(function() {
    'use strict';

    const state = {
        currentView: 'dictation',
        previousView: 'dictation',
        isRecording: false,
        isSubtitleActive: false,
        triggerMode: 'hold',
        dictationMode: 'batch',
        subtitleAlign: 'center',
        subtitleLayout: 'vertical',
        subtitlePreset: 'clean',
        subtitleAlign: 'center',
        subtitleContainerAlignX: 'center',
        subtitleContainerAlignY: 'bottom',
        customElements: [],
        elementOrder: ['speaker', 'original', 'original2', 'translation', 'timestamp'],
        transcriptSegments: [],
        scenes: [],
        currentSceneId: 'default',
        previewInterim: false,
        config: null,
        subtitleSettings: {
            fontSize: 32,
            opacity: 60,
            blur: 20,
            lines: 3
        },
        unlisteners: [],
        isMouseDown: false,
        settingsDirty: false,
        populatingSettings: false,
        tts: {
            voicePage: 1,
            voicePageSize: 20,
            voiceTotal: 0,
            lastAudioPath: null,
            lastText: '',
            savingTimer: null,
            voiceSearchTimer: null,
            loaded: false,
            synthesizing: false
        }
    };

    let invoke = null;
    let listen = null;
    let getCurrentWindow = null;

    if (window.__TAURI__) {
        const t = window.__TAURI__;
        invoke = (t.core && t.core.invoke) || t.invoke;
        listen = (t.event && t.event.listen) || t.listen;
        if (t.window) {
            getCurrentWindow = t.window.getCurrentWindow;
        }
    }

    const $ = (selector) => document.querySelector(selector);
    const $$ = (selector) => document.querySelectorAll(selector);

    // ====== 滑动指示器：为主题强调色背景块提供平滑过渡动画 ======

    /**
     * 将侧边栏滑动指示器移动到目标导航项。
     * 使用 offsetTop/offsetHeight（相对 offsetParent，已包含父级 padding），
     * 通过 transform + height 过渡实现平滑滑动。
     */
    function moveNavIndicator(targetNav) {
        const indicator = $('#nav-indicator');
        if (!indicator || !targetNav) return;
        const offsetY = targetNav.offsetTop;
        indicator.style.transform = `translateY(${offsetY}px)`;
        indicator.style.height = `${targetNav.offsetHeight}px`;
        indicator.classList.add('visible');
    }

    /**
     * 将分段控件/日志级别筛选器的滑动指示器移动到目标按钮。
     * 使用 offsetLeft/offsetTop/offsetWidth/offsetHeight（相对 offsetParent，已包含父级 padding），
     * 通过 transform + width/height 过渡实现平滑滑动。
     */
    function moveSegIndicator(targetBtn) {
        if (!targetBtn) return;
        const container = targetBtn.parentElement;
        const indicator = container ? container.querySelector('.seg-indicator') : null;
        if (!container || !indicator) return;
        indicator.style.transform = `translate(${targetBtn.offsetLeft}px, ${targetBtn.offsetTop}px)`;
        indicator.style.width = `${targetBtn.offsetWidth}px`;
        indicator.style.height = `${targetBtn.offsetHeight}px`;
        indicator.classList.add('visible');
    }

    /**
     * 刷新所有可见的滑动指示器位置（用于窗口尺寸变化、侧边栏切换、视图切换后）。
     */
    function refreshAllIndicators() {
        // 侧边栏导航
        const activeNav = $('.nav-item.active');
        if (activeNav) moveNavIndicator(activeNav);

        // 所有可见的 segmented control / log-level-filter
        $$('.segmented-control, .log-level-filter').forEach(container => {
            // 只刷新当前可见视图内的容器（避免为隐藏视图计算错误的尺寸）
            const view = container.closest('.view');
            if (view && !view.classList.contains('active')) return;
            const activeBtn = container.querySelector('.seg-btn.active, .log-level-btn.active');
            if (activeBtn) moveSegIndicator(activeBtn);
        });
    }

    function virtualKeyToName(vkCode) {
        const vkMap = {
            0x70: 'F1',
            0x71: 'F2',
            0x72: 'F3',
            0x73: 'F4',
            0x74: 'F5',
            0x75: 'F6',
            0x76: 'F7',
            0x77: 'F8',
            0x78: 'F9',
            0x79: 'F10',
            0x7a: 'F11',
            0x7b: 'F12',
            0x08: 'Backspace',
            0x09: 'Tab',
            0x0d: 'Enter',
            0x1b: 'Esc',
            0x20: 'Space',
            0x2e: 'Del'
        };
        return vkMap[vkCode] || String.fromCharCode(vkCode);
    }

    // 将按键名称转换为 Windows 虚拟键码（与后端 rdev::Key 的映射保持一致）
    // 仅支持后端 rdev 监听器能识别的按键（主要是 F1-F12）
    function nameToVirtualKey(name) {
        if (!name) return null;
        const keyName = String(name).trim();
        // F1-F12
        const fMatch = keyName.match(/^F(\d{1,2})$/i);
        if (fMatch) {
            const n = parseInt(fMatch[1], 10);
            if (n >= 1 && n <= 12) return 0x6f + n; // 0x70 = F1
        }
        // 其他常见按键（后端 rdev 目前只处理 F1-F12，但仍提供映射以便未来扩展）
        const map = {
            'Backspace': 0x08,
            'Tab': 0x09,
            'Enter': 0x0d,
            'Esc': 0x1b,
            'Escape': 0x1b,
            'Space': 0x20,
            'Del': 0x2e,
            'Delete': 0x2e
        };
        if (keyName in map) return map[keyName];
        // 单字符：转大写后取 ASCII
        if (keyName.length === 1) {
            const code = keyName.toUpperCase().charCodeAt(0);
            if (code >= 0x30 && code <= 0x5a) return code;
        }
        return null;
    }

    function setStatus(status, text) {
        const statusDot = $('#dictation-status .status-dot');
        const statusText = $('#dictation-status .status-text');
        const appStatusDot = $('#app-status-indicator .pulse-dot');
        const appStatusText = $('#app-status-text');

        const statusClasses = ['idle', 'recording', 'processing', 'ready', 'error'];
        statusClasses.forEach(cls => {
            if (statusDot) statusDot.classList.remove(cls);
        });

        if (statusDot) statusDot.classList.add(status);

        const dotColors = {
            idle: 'var(--text-tertiary)',
            recording: 'var(--accent-red)',
            processing: 'var(--accent-yellow)',
            ready: 'var(--accent-green)',
            error: 'var(--accent-red)'
        };

        if (appStatusDot && dotColors[status]) {
            appStatusDot.style.background = dotColors[status];
        }

        if (text) {
            if (statusText) statusText.textContent = text;
            if (appStatusText) appStatusText.textContent = text;
        }
    }

    async function switchView(viewName) {
        // 离开设置页/字幕页时检查未保存的更改
        const leavingSettingsView =
            (state.currentView === 'settings' && viewName !== 'settings') ||
            (state.currentView === 'subtitle' && viewName !== 'subtitle');
        if (leavingSettingsView && state.settingsDirty) {
            const result = await showUnsavedChangesDialog();
            if (result === 'cancel') return;
            if (result === 'save') {
                await saveSettings();
            }
            state.settingsDirty = false;
        }

        const views = $$('.view');
        views.forEach(v => v.classList.remove('active'));

        const navItems = $$('.nav-item');
        navItems.forEach(n => n.classList.remove('active'));

        const targetView = $(`#view-${viewName}`);
        if (targetView) {
            targetView.classList.add('active');
        }
        const targetNav = $(`.nav-item[data-view="${viewName}"]`);
        if (targetNav) {
            targetNav.classList.add('active');
            // 滑动指示器：平滑移动到新选中的导航项
            moveNavIndicator(targetNav);
        }
        state.currentView = viewName;

        if (viewName === 'history') {
            loadHistory();
        } else if (viewName === 'settings') {
            loadSettings();
            loadInputDevices();
            // 模型和引擎检测改为手动触发
        } else if (viewName === 'tts') {
            loadTtsView();
        }

        // 视图切换后刷新分段控件指示器（新视图变为可见后才能正确测量尺寸）
        requestAnimationFrame(refreshAllIndicators);
    }

    function showSettings() {
        switchView('settings');
    }

    function hideSettings() {
        switchView(state.previousView || 'dictation');
    }

    async function startRecording() {
        if (!invoke) {
            console.log('Tauri API not available');
            return;
        }

        const micBtn = $('#mic-btn');
        const micHint = $('.mic-hint');

        try {
            await invoke('start_recording');
            state.isRecording = true;
            if (micBtn) micBtn.classList.add('recording');
            if (micHint) micHint.textContent = state.triggerMode === 'hold' ? '松开停止录音' : '点击停止录音';
            setStatus('recording', '正在录音...');
        } catch (err) {
            console.error('Failed to start recording:', err);
            setStatus('error', '启动录音失败');
            setTimeout(() => setStatus('idle', '就绪'), 2000);
        }
    }

    async function stopRecording() {
        if (!invoke) {
            console.log('Tauri API not available');
            return;
        }

        const micBtn = $('#mic-btn');
        const micHint = $('.mic-hint');

        try {
            if (micBtn) micBtn.classList.remove('recording');
            if (micHint) micHint.textContent = '点击开始录音';
            setStatus('processing', '正在识别...');
            const result = await invoke('stop_recording');
            if (result && result.trim()) {
                appendToOutput(result);
            }
            state.isRecording = false;
        } catch (err) {
            console.error('Failed to stop recording:', err);
            setStatus('error', '识别失败');
            setTimeout(() => setStatus('idle', '就绪'), 2000);
            state.isRecording = false;
        }
    }

    function appendToOutput(text) {
        const output = $('#dictation-output');
        if (!output) return;

        const existingText = output.textContent.trim();
        if (existingText) {
            output.textContent = existingText + '\n' + text;
        } else {
            output.textContent = text;
        }
        output.scrollTop = output.scrollHeight;
    }

    async function toggleSubtitle() {
        if (!invoke) {
            console.log('Tauri API not available');
            return;
        }

        try {
            const isActive = await invoke('toggle_subtitle');
            state.isSubtitleActive = isActive;
            updateSubtitleButton();
        } catch (err) {
            console.error('Failed to toggle subtitle:', err);
        }
    }

    function updateSubtitleButton() {
        const btn = $('#toggle-subtitle-btn');
        if (!btn) return;

        if (state.isSubtitleActive) {
            btn.classList.add('active');
            btn.innerHTML = '<span class="btn-indicator"></span>停止字幕';
        } else {
            btn.classList.remove('active');
            btn.innerHTML = '<span class="btn-indicator"></span>开启实时字幕';
        }
    }

    function ensurePreviewBox() {
        const preview = $('.subtitle-preview');
        if (!preview) return null;
        let box = $('#subtitle-preview-box');
        if (!box) {
            const textEl = $('#subtitle-preview-text');
            box = document.createElement('div');
            box.id = 'subtitle-preview-box';
            box.className = 'subtitle-preview-box';
            if (textEl && textEl.parentNode === preview) {
                preview.insertBefore(box, textEl);
                box.appendChild(textEl);
            } else {
                preview.appendChild(box);
            }
        }
        return box;
    }

    function resolvePreviewPlaceholders(text) {
        if (!text) return '';
        const now = new Date();
        const h = String(now.getHours()).padStart(2, '0');
        const m = String(now.getMinutes()).padStart(2, '0');
        const s = String(now.getSeconds()).padStart(2, '0');
        const y = now.getFullYear();
        const mo = String(now.getMonth() + 1).padStart(2, '0');
        const d = String(now.getDate()).padStart(2, '0');
        return text
            .replace(/\{time\}/g, `${h}:${m}:${s}`)
            .replace(/\{date\}/g, `${y}-${mo}-${d}`)
            .replace(/\{datetime\}/g, `${y}-${mo}-${d} ${h}:${m}:${s}`);
    }

    function formatPreviewTimestamp(format) {
        const now = new Date();
        const h = String(now.getHours()).padStart(2, '0');
        const m = String(now.getMinutes()).padStart(2, '0');
        const s = String(now.getSeconds()).padStart(2, '0');
        switch (format) {
            case 'MM:SS': return `${m}:${s}`;
            case 'none': return '';
            case 'HH:MM:SS':
            default: return `${h}:${m}:${s}`;
        }
    }

    // 切换音源类型时更新设备下拉/提示的显隐
    function updateSubtitleSourceUI(source) {
        const micGroup = $('#subtitle-mic-device-group');
        const systemHint = $('#subtitle-system-hint');
        const dualHint = $('#subtitle-dual-hint');
        if (source === 'system') {
            if (micGroup) micGroup.style.display = 'none';
            if (systemHint) systemHint.style.display = '';
            if (dualHint) dualHint.style.display = 'none';
        } else if (source === 'dual') {
            // 同传模式：系统音为主源，麦克风为副字幕音源
            if (micGroup) micGroup.style.display = '';
            if (systemHint) systemHint.style.display = 'none';
            if (dualHint) dualHint.style.display = '';
        } else {
            if (micGroup) micGroup.style.display = '';
            if (systemHint) systemHint.style.display = 'none';
            if (dualHint) dualHint.style.display = 'none';
        }
    }

    function updateSubtitlePreview() {
        const preview = $('.subtitle-preview');
        const previewText = $('#subtitle-preview-text');
        if (!preview || !previewText) return;
        const box = ensurePreviewBox();

        // 原文预览包裹容器（历史行 + 当前行，与真实窗口 #sub-original 结构一致）
        let origWrap = $('#preview-original');
        if (!origWrap) {
            origWrap = document.createElement('div');
            origWrap.id = 'preview-original';
            origWrap.className = 'sub-preview-original';
            box.appendChild(origWrap);
            origWrap.appendChild(previewText);
        }

        // 同步 bold 开关状态：与后端 push_subtitle_config 一致，bold = font_weight >= 700
        const weightSelect = $('#subtitle-font-weight');
        const boldSwitch = $('#subtitle-bold');
        if (weightSelect && boldSwitch) {
            const shouldBold = parseInt(weightSelect.value) >= 700;
            boldSwitch.dataset.on = shouldBold ? 'true' : 'false';
        }

        const scene = collectSceneFromUI();
        if (!scene) return;
        const cfg = sceneToFlat(scene);

        const shadowColor = cfg.subtitle_text_shadow_color || '#000000';
        const shadowStrength = typeof cfg.subtitle_text_shadow_strength === 'number' ? cfg.subtitle_text_shadow_strength : 4;
        const textShadow = buildTextShadow(shadowColor, shadowStrength);

        // ===== 容器对齐锚点 + 卡片最大宽度（与真实窗口一致） =====
        const alignMap = { left: 'flex-start', center: 'center', right: 'flex-end', top: 'flex-start', bottom: 'flex-end' };
        preview.style.justifyContent = alignMap[cfg.subtitle_container_align_x] || 'center';
        preview.style.alignItems = alignMap[cfg.subtitle_container_align_y] || 'flex-end';
        box.style.maxWidth = cfg.subtitle_box_max_width + '%';

        // ===== 容器样式（背景/模糊/内边距/布局） =====
        const bEl = box.style;
        bEl.background = hexToRgba(cfg.subtitle_bg_color, cfg.subtitle_bg_opacity);
        bEl.backdropFilter = `blur(${cfg.subtitle_blur}px)`;
        bEl.webkitBackdropFilter = `blur(${cfg.subtitle_blur}px)`;
        bEl.padding = `${cfg.subtitle_padding_y}px ${cfg.subtitle_padding_x}px`;
        const layout = cfg.subtitle_layout || 'vertical';
        bEl.flexDirection = layout === 'horizontal' ? 'row' : 'column';
        bEl.alignItems = layout === 'horizontal' ? 'center' : 'stretch';
        bEl.gap = layout === 'horizontal' ? '16px' : '6px';

        // ===== 原文元素（样式作用于包裹容器，历史行继承） =====
        const oEl = origWrap.style;
        oEl.fontFamily = `"${cfg.subtitle_font_family}", sans-serif`;
        oEl.fontSize = cfg.subtitle_font_size + 'px';
        oEl.fontWeight = cfg.subtitle_bold ? '700' : String(cfg.subtitle_font_weight);
        oEl.fontStyle = cfg.subtitle_italic ? 'italic' : 'normal';
        oEl.textAlign = cfg.subtitle_text_align;
        oEl.letterSpacing = cfg.subtitle_letter_spacing + 'px';
        oEl.lineHeight = String(cfg.subtitle_line_height);
        oEl.textShadow = textShadow;
        oEl.display = cfg.subtitle_show_original !== false ? '' : 'none';

        // 当前行文本元素：颜色/透明度跟随 最终/临时 状态
        const tCur = previewText.style;
        tCur.fontFamily = '';
        tCur.fontSize = '';
        tCur.fontWeight = '';
        tCur.fontStyle = '';
        tCur.letterSpacing = '';
        tCur.lineHeight = '';
        tCur.padding = '';
        tCur.background = '';
        tCur.backdropFilter = '';
        tCur.webkitBackdropFilter = '';
        tCur.textShadow = '';
        tCur.webkitLineClamp = '';
        tCur.display = '';
        if (state.previewInterim) {
            tCur.color = cfg.subtitle_interim_color;
            tCur.opacity = String(cfg.subtitle_interim_opacity);
            previewText.textContent = '临时识别结果预览效果...';
        } else {
            tCur.color = cfg.subtitle_font_color;
            tCur.opacity = '1';
            previewText.textContent = '实时字幕预览效果';
        }

        // 历史行示例（反映「显示行数」设置：maxLines-1 条，越老越淡）
        const histCount = Math.max(0, Math.min(cfg.subtitle_max_lines, 6) - 1);
        let histNodes = origWrap.querySelectorAll('.sub-preview-history-line');
        while (histNodes.length < histCount) {
            const h = document.createElement('div');
            h.className = 'sub-preview-history-line';
            origWrap.insertBefore(h, previewText);
            histNodes = origWrap.querySelectorAll('.sub-preview-history-line');
        }
        while (histNodes.length > histCount) {
            histNodes[0].remove();
            histNodes = origWrap.querySelectorAll('.sub-preview-history-line');
        }
        origWrap.querySelectorAll('.sub-preview-history-line').forEach((h, i) => {
            h.style.opacity = String(Math.min(0.95, 0.5 + 0.15 * i));
            h.textContent = '历史字幕示例行';
        });

        // ===== 译文元素 =====
        let transEl = $('#preview-translation');
        if (cfg.subtitle_show_translation) {
            if (!transEl) {
                transEl = document.createElement('div');
                transEl.id = 'preview-translation';
                transEl.className = 'sub-preview-element';
                box.appendChild(transEl);
            }
            const tEl = transEl.style;
            tEl.display = '';
            tEl.fontFamily = `"${cfg.subtitle_font_family}", sans-serif`;
            tEl.fontSize = cfg.subtitle_translation_font_size + 'px';
            tEl.fontWeight = String(cfg.subtitle_translation_font_weight);
            tEl.color = cfg.subtitle_translation_font_color;
            tEl.opacity = String(cfg.subtitle_translation_opacity);
            tEl.textAlign = cfg.subtitle_text_align;
            tEl.letterSpacing = cfg.subtitle_letter_spacing + 'px';
            tEl.lineHeight = String(cfg.subtitle_line_height);
            tEl.textShadow = textShadow;
            if (cfg.subtitle_max_lines > 1) {
                transEl.innerHTML = '';
                const hline = document.createElement('div');
                hline.className = 'sub-preview-history-line';
                hline.style.opacity = '0.6';
                hline.textContent = '译文历史示例';
                const cline = document.createElement('div');
                cline.textContent = (cfg.subtitle_translation_prefix || '') + '译文预览效果';
                transEl.appendChild(hline);
                transEl.appendChild(cline);
            } else {
                transEl.textContent = (cfg.subtitle_translation_prefix || '') + '译文预览效果';
            }
        } else if (transEl) {
            transEl.style.display = 'none';
        }

        // ===== 副原文元素（双源同传模式的麦克风副字幕示例） =====
        let orig2El = $('#preview-original2');
        if (cfg.subtitle_show_original_secondary) {
            if (!orig2El) {
                orig2El = document.createElement('div');
                orig2El.id = 'preview-original2';
                orig2El.className = 'sub-preview-element';
                box.appendChild(orig2El);
            }
            const o2 = orig2El.style;
            o2.display = '';
            o2.fontFamily = `"${cfg.subtitle_font_family}", sans-serif`;
            o2.fontSize = Math.max(14, Math.round(cfg.subtitle_font_size * 0.8)) + 'px';
            o2.fontWeight = String(cfg.subtitle_font_weight);
            o2.color = '#7dd3fc';
            o2.opacity = '0.9';
            o2.textAlign = cfg.subtitle_text_align;
            o2.letterSpacing = cfg.subtitle_letter_spacing + 'px';
            o2.lineHeight = String(cfg.subtitle_line_height);
            o2.textShadow = textShadow;
            orig2El.textContent = '副字幕预览效果（麦克风）';
        } else if (orig2El) {
            orig2El.style.display = 'none';
        }

        // ===== 说话人元素 =====
        let spkEl = $('#preview-speaker');
        if (cfg.subtitle_show_speaker) {
            if (!spkEl) {
                spkEl = document.createElement('div');
                spkEl.id = 'preview-speaker';
                spkEl.className = 'sub-preview-element';
                box.appendChild(spkEl);
            }
            const sEl = spkEl.style;
            sEl.display = '';
            sEl.color = cfg.subtitle_speaker_color;
            sEl.fontSize = cfg.subtitle_speaker_font_size + 'px';
            sEl.fontWeight = '500';
            sEl.textAlign = cfg.subtitle_text_align;
            spkEl.textContent = (cfg.subtitle_speaker_prefix || '') + '说话人';
        } else if (spkEl) {
            spkEl.style.display = 'none';
        }

        // ===== 时间戳元素 =====
        let tsEl = $('#preview-timestamp');
        if (cfg.subtitle_show_timestamp && cfg.subtitle_timestamp_format !== 'none') {
            if (!tsEl) {
                tsEl = document.createElement('div');
                tsEl.id = 'preview-timestamp';
                tsEl.className = 'sub-preview-element';
                box.appendChild(tsEl);
            }
            const tse = tsEl.style;
            tse.display = '';
            tse.color = cfg.subtitle_timestamp_color;
            tse.fontSize = cfg.subtitle_timestamp_font_size + 'px';
            tse.fontWeight = '400';
            tse.fontFamily = '"Cascadia Code", "Consolas", monospace';
            tse.textAlign = cfg.subtitle_text_align;
            tsEl.textContent = formatPreviewTimestamp(cfg.subtitle_timestamp_format);
        } else if (tsEl) {
            tsEl.style.display = 'none';
        }

        // ===== 自定义元素预览 =====
        const customEls = state.customElements || [];
        const existingCustom = new Map();
        box.querySelectorAll('[data-preview-custom]').forEach(n => existingCustom.set(n.dataset.previewCustom, n));
        const seenCustom = new Set();
        customEls.forEach(ce => {
            const id = ce.id || 'unknown';
            seenCustom.add(id);
            let node = existingCustom.get(id);
            if (!node) {
                node = document.createElement('div');
                node.className = 'sub-preview-element';
                node.dataset.previewCustom = id;
                box.appendChild(node);
            }
            const et = ce.element_type || 'text';
            const vis = ce.visible !== false;
            const st = node.style;
            if (et === 'divider') {
                st.display = vis ? '' : 'none';
                st.height = '1px';
                st.width = '100%';
                st.background = ce.color || '#ffffff';
                st.opacity = String(typeof ce.opacity === 'number' ? ce.opacity : 0.3);
                st.margin = '4px 0';
                st.fontSize = '';
                node.textContent = '';
            } else if (et === 'spacer') {
                st.display = vis ? '' : 'none';
                st.height = (typeof ce.font_size === 'number' ? ce.font_size : 12) + 'px';
                st.background = '';
                st.opacity = '';
                st.margin = '';
                node.textContent = '';
            } else {
                st.display = vis ? '' : 'none';
                st.height = '';
                st.width = '';
                st.background = '';
                st.margin = '';
                st.color = ce.color || '#ffffff';
                if (typeof ce.font_size === 'number') st.fontSize = ce.font_size + 'px';
                if (typeof ce.font_weight === 'number') st.fontWeight = String(ce.font_weight);
                if (typeof ce.opacity === 'number') st.opacity = String(ce.opacity);
                st.textAlign = ce.align || 'center';
                st.textShadow = textShadow;
                st.fontFamily = `"${cfg.subtitle_font_family}", sans-serif`;
                node.textContent = (ce.prefix || '') + resolvePreviewPlaceholders(ce.content || '自定义文本');
            }
        });
        existingCustom.forEach((node, id) => { if (!seenCustom.has(id)) node.remove(); });

        // ===== 按 elementOrder 重排预览元素 =====
        const idMap = { speaker: 'preview-speaker', original: 'preview-original', original2: 'preview-original2', translation: 'preview-translation', timestamp: 'preview-timestamp' };
        const order = (state.elementOrder && state.elementOrder.length > 0) ? state.elementOrder : ['speaker', 'original', 'translation', 'timestamp'];
        order.forEach(key => {
            let node = null;
            if (idMap[key]) node = $('#' + idMap[key]);
            else node = box.querySelector(`[data-preview-custom="${CSS.escape(key)}"]`);
            if (node) box.appendChild(node);
        });

        // 更新所有 value-display
        updateValueDisplay('subtitle-font-size', `${cfg.subtitle_font_size}px`);
        updateValueDisplay('subtitle-opacity', `${Math.round(cfg.subtitle_bg_opacity * 100)}%`);
        updateValueDisplay('subtitle-blur', `${cfg.subtitle_blur}px`);
        updateValueDisplay('subtitle-lines', `${cfg.subtitle_max_lines} 行`);
        updateValueDisplay('subtitle-box-max-width', `${cfg.subtitle_box_max_width}%`);
        updateValueDisplay('subtitle-line-height', cfg.subtitle_line_height.toFixed(1));
        updateValueDisplay('subtitle-letter-spacing', `${cfg.subtitle_letter_spacing}px`);
        updateValueDisplay('subtitle-text-shadow-strength', String(cfg.subtitle_text_shadow_strength));
        updateValueDisplay('subtitle-padding-x', `${cfg.subtitle_padding_x}px`);
        updateValueDisplay('subtitle-padding-y', `${cfg.subtitle_padding_y}px`);
        updateValueDisplay('subtitle-interim-opacity', `${Math.round(cfg.subtitle_interim_opacity * 100)}%`);
    }

    function updateValueDisplay(id, text) {
        const slider = $(`#${id}`);
        if (slider && slider.nextElementSibling && slider.nextElementSibling.classList.contains('value-display')) {
            slider.nextElementSibling.textContent = text;
        }
        // 同步更新滑块的填充进度（用于 CSS 渐变背景）
        if (slider && slider.type === 'range') {
            const min = parseFloat(slider.min) || 0;
            const max = parseFloat(slider.max) || 100;
            const val = parseFloat(slider.value);
            const pct = max > min ? ((val - min) / (max - min)) * 100 : 0;
            slider.style.setProperty('--fill', pct + '%');
        }
    }

    // ===== 字幕预设模板 =====
    const SUBTITLE_PRESETS = {
        clean: { show_original: true, show_translation: false, show_speaker: false, show_timestamp: false, layout: 'vertical' },
        bilingual: { show_original: true, show_translation: true, show_speaker: false, show_timestamp: false, layout: 'vertical' },
        meeting: { show_original: true, show_translation: false, show_speaker: true, show_timestamp: true, layout: 'vertical' },
        live: { show_original: true, show_translation: true, show_speaker: false, show_timestamp: true, layout: 'horizontal' },
    };

    // ===== 字幕场景管理（多窗口） =====

    /// 场景样式字段列表（scene.style.* 与 flat 配置 subtitle_* 一一对应）
    const SCENE_STYLE_FIELDS = [
        'font_family', 'font_size', 'font_color', 'font_weight', 'italic', 'text_align',
        'letter_spacing', 'line_height', 'text_shadow_color', 'text_shadow_strength',
        'bg_color', 'bg_opacity', 'blur',
        'padding_x', 'padding_y', 'max_lines', 'interim_color', 'interim_opacity',
        'show_original', 'show_translation', 'show_speaker', 'show_timestamp', 'layout',
        'translation_font_size', 'translation_font_color', 'translation_font_weight',
        'translation_opacity', 'translation_prefix', 'speaker_color', 'speaker_font_size',
        'speaker_prefix', 'timestamp_color', 'timestamp_font_size', 'timestamp_format',
        'custom_elements', 'element_order', 'preset',
        'container_align_x', 'container_align_y', 'box_max_width', 'show_original_secondary'
    ];

    function getCurrentScene() {
        return (state.scenes || []).find(s => s.id === state.currentSceneId) || null;
    }

    /// 从旧扁平字段构建默认场景（兼容旧配置）
    function sceneFromLegacy(sub) {
        const flat = sub || {};
        return {
            id: 'default',
            name: '默认字幕',
            enabled: true,
            window: {
                x: flat.subtitle_window_x !== undefined && flat.subtitle_window_x !== null ? flat.subtitle_window_x : -1,
                y: flat.subtitle_window_y !== undefined && flat.subtitle_window_y !== null ? flat.subtitle_window_y : -1,
                width: flat.subtitle_window_width || 1200,
                height: flat.subtitle_window_height || 120,
                always_on_top: true,
                click_through: false,
                obs_mode: false,
                auto_fit: true,
            },
            style: {
                font_family: flat.subtitle_font_family || 'Microsoft YaHei',
                font_size: flat.subtitle_font_size || 32,
                font_color: flat.subtitle_font_color || '#ffffff',
                font_weight: flat.subtitle_font_weight || 400,
                italic: !!flat.subtitle_italic,
                text_align: flat.subtitle_text_align || 'center',
                letter_spacing: flat.subtitle_letter_spacing !== undefined ? flat.subtitle_letter_spacing : 0,
                line_height: flat.subtitle_line_height !== undefined ? flat.subtitle_line_height : 1.4,
                text_shadow_color: flat.subtitle_text_shadow_color || '#000000',
                text_shadow_strength: flat.subtitle_text_shadow_strength !== undefined ? flat.subtitle_text_shadow_strength : 4,
                bg_color: flat.subtitle_bg_color || '#000000',
                bg_opacity: flat.subtitle_bg_opacity !== undefined ? flat.subtitle_bg_opacity : 0.6,
                blur: flat.subtitle_blur || 20,
                padding_x: flat.subtitle_padding_x || 24,
                padding_y: flat.subtitle_padding_y || 12,
                max_lines: flat.subtitle_max_lines || 2,
                interim_color: flat.subtitle_interim_color || '#ffffff',
                interim_opacity: flat.subtitle_interim_opacity !== undefined ? flat.subtitle_interim_opacity : 0.7,
                show_original: flat.subtitle_show_original !== false,
                show_translation: flat.subtitle_show_translation === true,
                show_speaker: flat.subtitle_show_speaker === true,
                show_timestamp: flat.subtitle_show_timestamp === true,
                layout: flat.subtitle_layout || 'vertical',
                translation_font_size: flat.subtitle_translation_font_size || 24,
                translation_font_color: flat.subtitle_translation_font_color || '#ffffff',
                translation_font_weight: flat.subtitle_translation_font_weight || 400,
                translation_opacity: flat.subtitle_translation_opacity !== undefined ? flat.subtitle_translation_opacity : 0.85,
                translation_prefix: flat.subtitle_translation_prefix || '',
                speaker_color: flat.subtitle_speaker_color || '#818cf8',
                speaker_font_size: flat.subtitle_speaker_font_size || 16,
                speaker_prefix: flat.subtitle_speaker_prefix || '',
                timestamp_color: flat.subtitle_timestamp_color || '#a1a1aa',
                timestamp_font_size: flat.subtitle_timestamp_font_size || 14,
                timestamp_format: flat.subtitle_timestamp_format || 'HH:MM:SS',
                custom_elements: Array.isArray(flat.subtitle_custom_elements) ? flat.subtitle_custom_elements.map(e => ({ ...e })) : [],
                element_order: (Array.isArray(flat.subtitle_element_order) && flat.subtitle_element_order.length > 0)
                    ? [...flat.subtitle_element_order]
                    : ['speaker', 'original', 'original2', 'translation', 'timestamp'],
                preset: flat.subtitle_preset || 'clean',
                container_align_x: 'center',
                container_align_y: 'bottom',
                box_max_width: 100,
                show_original_secondary: false,
            },
            translation: {
                engine: flat.subtitle_translation_engine || 'none',
                target_lang: flat.subtitle_translation_target_lang || '英文',
                interim: true,
            },
        };
    }

    /// 场景 → 旧扁平 cfg（供 updateSubtitlePreview 复用）
    function sceneToFlat(scene) {
        const s = (scene && scene.style) || {};
        const flat = {};
        SCENE_STYLE_FIELDS.forEach(f => { flat['subtitle_' + f] = s[f]; });
        flat.subtitle_translation_enabled = !!s.show_translation;
        flat.subtitle_bold = (s.font_weight || 400) >= 700;
        return flat;
    }

    /// 旧扁平 cfg → 场景 style
    function flatToScene(scene, flat) {
        if (!scene.style) scene.style = {};
        SCENE_STYLE_FIELDS.forEach(f => {
            const k = 'subtitle_' + f;
            if (flat[k] !== undefined) scene.style[f] = flat[k];
        });
        return scene;
    }

    /// 从配置加载场景列表到 state（兼容旧配置自动迁移）
    function loadScenesIntoState(config) {
        let scenes = (config && config.subtitle && Array.isArray(config.subtitle.subtitle_scenes))
            ? config.subtitle.subtitle_scenes
            : [];
        scenes = scenes.map(sc => ({
            ...sc,
            style: { ...(sc.style || {}) },
            window: { ...(sc.window || {}) },
            translation: { ...(sc.translation || {}) },
        }));
        if (!scenes.length) {
            scenes = [sceneFromLegacy(config && config.subtitle)];
        }
        if (!scenes.find(s => s.id === 'default')) {
            scenes.unshift(sceneFromLegacy(config && config.subtitle));
        }
        state.scenes = scenes;
        if (!state.scenes.find(s => s.id === state.currentSceneId)) {
            state.currentSceneId = 'default';
        }
        renderSceneList();
    }

    /// 渲染场景选择器
    function renderSceneList() {
        const sel = $('#subtitle-scene-select');
        if (!sel) return;
        const current = state.currentSceneId;
        sel.innerHTML = (state.scenes || []).map(sc =>
            `<option value="${escapeHtml(sc.id)}"${sc.id === current ? ' selected' : ''}>${escapeHtml(sc.name || sc.id)}${sc.enabled ? '' : '（已停用）'}</option>`
        ).join('');
        const delBtn = $('#btn-remove-subtitle-scene');
        if (delBtn) delBtn.disabled = current === 'default';
    }

    /// 切换场景：当前 UI 草稿写回 state.scenes，再载入目标场景
    function switchSubtitleScene(newId) {
        if (!newId || newId === state.currentSceneId) return;
        const cur = collectSceneFromUI();
        if (cur) {
            const idx = state.scenes.findIndex(s => s.id === cur.id);
            if (idx >= 0) state.scenes[idx] = cur;
        }
        state.currentSceneId = newId;
        const scene = getCurrentScene();
        if (scene && state.config) {
            populateSubtitleUiFromScene(scene, state.config);
        }
        renderSceneList();
        refreshAllIndicators();
    }

    /// 同声传译设置区显隐
    function updateTranslationUI() {
        const engine = ($('#subtitle-translation-engine') || {}).value || 'none';
        const enabled = engine !== 'none';
        const langGroup = $('#subtitle-translation-lang-group');
        const interimGroup = $('#subtitle-translation-interim-group');
        const llmRows = $$('.translation-llm-row');
        if (langGroup) langGroup.style.display = enabled ? '' : 'none';
        if (interimGroup) interimGroup.style.display = enabled ? '' : 'none';
        llmRows.forEach(row => { row.style.display = engine === 'llm' ? '' : 'none'; });
    }

    /// 静默保存当前全部设置草稿（场景增删前调用，避免丢失未保存修改）
    async function saveCurrentSceneDraft() {
        if (!invoke || !state.config) return false;
        const newConfig = collectSettings();
        if (!newConfig) return false;
        try {
            await invoke('save_config', { newConfig });
            state.config = newConfig;
            state.settingsDirty = false;
            return true;
        } catch (err) {
            console.error('Failed to save draft:', err);
            return false;
        }
    }

    /// 后端场景变更后重新加载并选中指定场景
    async function reloadScenesAfterBackendChange(selectId) {
        if (!invoke) return;
        const cfg = await invoke('get_config');
        state.config = cfg;
        loadScenesIntoState(cfg);
        state.currentSceneId = selectId || 'default';
        const scene = getCurrentScene();
        if (scene) populateSubtitleUiFromScene(scene, cfg);
        renderSceneList();
        state.settingsDirty = false;
        updateSubtitlePreview();
        refreshAllIndicators();
    }

    async function addSubtitleScene() {
        if (!invoke) return;
        await saveCurrentSceneDraft();
        try {
            const id = await invoke('duplicate_subtitle_scene', { sceneId: state.currentSceneId || 'default' });
            await reloadScenesAfterBackendChange(id);
            addLog('info', `已添加字幕场景`, 'subtitle');
        } catch (err) {
            console.error('Failed to add subtitle scene:', err);
            alert('添加场景失败: ' + err);
        }
    }

    async function duplicateSubtitleScene() {
        if (!invoke) return;
        await saveCurrentSceneDraft();
        try {
            const id = await invoke('duplicate_subtitle_scene', { sceneId: state.currentSceneId || 'default' });
            await reloadScenesAfterBackendChange(id);
            addLog('info', '已复制字幕场景', 'subtitle');
        } catch (err) {
            console.error('Failed to duplicate subtitle scene:', err);
            alert('复制场景失败: ' + err);
        }
    }

    async function removeSubtitleScene() {
        if (!invoke || state.currentSceneId === 'default') return;
        const { confirmed } = await showConfirmDialog(
            '删除场景',
            '确定删除当前场景及其字幕窗口吗？此操作不可恢复。',
            '删除'
        );
        if (!confirmed) return;
        await saveCurrentSceneDraft();
        try {
            await invoke('remove_subtitle_scene', { sceneId: state.currentSceneId });
            await reloadScenesAfterBackendChange('default');
            addLog('info', '已删除字幕场景', 'subtitle');
        } catch (err) {
            console.error('Failed to remove subtitle scene:', err);
            alert('删除场景失败: ' + err);
        }
    }

    // ===== 实时转录面板（会议纪要） =====

    function formatTranscriptTime(ms) {
        const total = Math.floor((ms || 0) / 1000);
        const m = String(Math.floor(total / 60)).padStart(2, '0');
        const s = String(total % 60).padStart(2, '0');
        return `${m}:${s}`;
    }

    function renderTranscript(segments) {
        const list = $('#subtitle-transcript-list');
        if (!list) return;
        state.transcriptSegments = Array.isArray(segments) ? segments : [];
        const segs = state.transcriptSegments;
        const countEl = $('#transcript-status');
        if (countEl) countEl.textContent = segs.length ? `共 ${segs.length} 条` : '';
        if (!segs.length) {
            list.innerHTML = '<div class="transcript-empty">开启实时字幕后，定稿句段将显示在这里</div>';
            return;
        }
        const wasAtBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
        list.innerHTML = segs.map(seg => {
            const srcBadge = seg.source === 'B' ? '<span class="tr-source-b">麦克风</span>' : '';
            const speaker = seg.speaker ? `<span class="tr-speaker">${escapeHtml(seg.speaker)}:</span>` : '';
            const trans = seg.translation ? `<span class="tr-translation">译: ${escapeHtml(seg.translation)}</span>` : '';
            return `<div class="transcript-item"><span class="tr-time">[${formatTranscriptTime(seg.start_ms)}]</span>${srcBadge}${speaker}${escapeHtml(seg.text)}${trans}</div>`;
        }).join('');
        if (wasAtBottom) list.scrollTop = list.scrollHeight;
    }

    function setTranscriptStatus(text, isError) {
        const el = $('#transcript-status');
        if (!el) return;
        el.textContent = text || '';
        el.style.color = isError ? 'var(--accent-red)' : '';
    }

    async function exportTranscript(format) {
        if (!invoke) return;
        const label = { txt: 'TXT', srt: 'SRT', md: 'Markdown' }[format] || 'TXT';
        setTranscriptStatus(`导出 ${label} 中...`);
        try {
            const path = await invoke('export_subtitle_transcript', { format });
            setTranscriptStatus(`已导出: ${path}`);
            setTimeout(() => setTranscriptStatus(''), 8000);
        } catch (err) {
            console.error('Failed to export transcript:', err);
            setTranscriptStatus(`导出失败: ${err}`, true);
            setTimeout(() => setTranscriptStatus(''), 8000);
        }
    }

    async function clearTranscript() {
        if (!invoke) return;
        try {
            await invoke('clear_subtitle_transcript');
            renderTranscript([]);
            setTranscriptStatus('已清空');
            setTimeout(() => setTranscriptStatus(''), 2000);
        } catch (err) {
            console.error('Failed to clear transcript:', err);
        }
    }


    function escapeHtmlSimple(str) {
        return String(str == null ? '' : str).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }

    function updatePresetUI() {
        $$('#subtitle-preset .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn.dataset.preset === state.subtitlePreset);
        });
        const activeBtn = $(`#subtitle-preset .seg-btn[data-preset="${state.subtitlePreset}"]`);
        if (activeBtn) moveSegIndicator(activeBtn);
    }

    function markPresetCustom() {
        if (state.subtitlePreset !== 'custom') {
            state.subtitlePreset = 'custom';
            updatePresetUI();
        }
    }

    function applySubtitlePreset(preset) {
        if (preset === 'custom') {
            state.subtitlePreset = 'custom';
            updatePresetUI();
            updateSubtitlePreview();
            return;
        }
        const p = SUBTITLE_PRESETS[preset];
        if (!p) return;
        const setSwitch = (id, on) => {
            const sw = $(`#${id}`);
            if (sw) sw.dataset.on = on ? 'true' : 'false';
        };
        setSwitch('subtitle-show-original', p.show_original);
        setSwitch('subtitle-show-translation', p.show_translation);
        setSwitch('subtitle-show-speaker', p.show_speaker);
        setSwitch('subtitle-show-timestamp', p.show_timestamp);
        state.subtitleLayout = p.layout;
        $$('#subtitle-layout .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn.dataset.mode === p.layout);
        });
        const activeBtn = $(`#subtitle-layout .seg-btn[data-mode="${p.layout}"]`);
        if (activeBtn) moveSegIndicator(activeBtn);
        state.subtitlePreset = preset;
        updatePresetUI();
        // 预设开启了译文但翻译引擎为关闭时，自动切到 LLM 同声传译
        const engineSel = $('#subtitle-translation-engine');
        const showTransSw = $('#subtitle-show-translation');
        if (engineSel && showTransSw && showTransSw.dataset.on === 'true' && engineSel.value === 'none') {
            engineSel.value = 'llm';
            updateTranslationUI();
        }
        updateSubtitlePreview();
    }

    // ===== 自定义元素管理 =====
    function genCustomElementId() {
        return 'ce_' + Date.now().toString(36) + '_' + Math.random().toString(36).slice(2, 7);
    }

    function initSliderFillsInList() {
        $$('#custom-element-list input[type="range"]').forEach(slider => {
            const min = parseFloat(slider.min) || 0;
            const max = parseFloat(slider.max) || 100;
            const val = parseFloat(slider.value);
            const pct = max > min ? ((val - min) / (max - min)) * 100 : 0;
            slider.style.setProperty('--fill', pct + '%');
        });
    }

    function renderCustomElementList() {
        const list = $('#custom-element-list');
        if (!list) return;
        list.innerHTML = '';
        const els = state.customElements || [];
        if (els.length === 0) {
            list.innerHTML = '<div class="custom-element-empty">暂无自定义元素，点击下方按钮添加</div>';
            return;
        }
        const weightOpts = [100, 200, 300, 400, 500, 600, 700, 800, 900];
        els.forEach(el => {
            const typeLabel = el.element_type === 'divider' ? '分隔线' : (el.element_type === 'spacer' ? '间距' : '文本');
            const typeIcon = el.element_type === 'divider' ? '—' : (el.element_type === 'spacer' ? '↕' : 'T');
            let bodyHtml = '';
            if (el.element_type === 'text') {
                bodyHtml =
                    `<input type="text" class="text-input ce-field" data-field="content" placeholder="内容（支持 {time} {date}）" value="${escapeHtmlSimple(el.content || '')}">` +
                    `<div class="ce-style-row">` +
                    `<input type="color" class="color-input ce-field" data-field="color" value="${el.color || '#ffffff'}" title="颜色">` +
                    `<input type="range" min="8" max="96" class="slider ce-field" data-field="font_size" value="${el.font_size || 18}" title="字号">` +
                    `<select class="text-input ce-field" data-field="font_weight">` +
                    weightOpts.map(w => `<option value="${w}" ${el.font_weight === w ? 'selected' : ''}>${w}</option>`).join('') +
                    `</select>` +
                    `<input type="range" min="0" max="100" class="slider ce-field" data-field="opacity_pct" value="${Math.round((el.opacity ?? 0.9) * 100)}" title="透明度">` +
                    `<input type="text" class="text-input ce-field ce-prefix-input" data-field="prefix" placeholder="前缀" value="${escapeHtmlSimple(el.prefix || '')}">` +
                    `<select class="text-input ce-field" data-field="align">` +
                    `<option value="left" ${el.align === 'left' ? 'selected' : ''}>左</option>` +
                    `<option value="center" ${(el.align === 'center' || !el.align) ? 'selected' : ''}>中</option>` +
                    `<option value="right" ${el.align === 'right' ? 'selected' : ''}>右</option>` +
                    `</select>` +
                    `</div>`;
            } else if (el.element_type === 'divider') {
                bodyHtml =
                    `<div class="ce-style-row">` +
                    `<input type="color" class="color-input ce-field" data-field="color" value="${el.color || '#ffffff'}" title="颜色">` +
                    `<input type="range" min="0" max="100" class="slider ce-field" data-field="opacity_pct" value="${Math.round((el.opacity ?? 0.3) * 100)}" title="透明度">` +
                    `</div>`;
            } else {
                bodyHtml =
                    `<div class="ce-style-row">` +
                    `<input type="range" min="4" max="48" class="slider ce-field" data-field="font_size" value="${el.font_size || 12}" title="高度">` +
                    `</div>`;
            }
            const item = document.createElement('div');
            item.className = 'custom-element-item';
            item.dataset.id = el.id;
            item.innerHTML =
                `<div class="ce-header">` +
                `<span class="ce-type-badge" title="${typeLabel}">${typeIcon} ${typeLabel}</span>` +
                `<input type="text" class="text-input ce-field ce-label-input" data-field="label" placeholder="名称" value="${escapeHtmlSimple(el.label || '')}">` +
                `<label class="ce-visible-toggle" title="显示/隐藏"><input type="checkbox" class="ce-field" data-field="visible" ${el.visible !== false ? 'checked' : ''}><span>显示</span></label>` +
                `<div class="ce-move-btns">` +
                `<button class="icon-btn ce-up" type="button" title="上移">▲</button>` +
                `<button class="icon-btn ce-down" type="button" title="下移">▼</button>` +
                `</div>` +
                `<button class="icon-btn ce-delete" type="button" title="删除">✕</button>` +
                `</div>` +
                `<div class="ce-body">${bodyHtml}</div>`;
            list.appendChild(item);
        });
        // 绑定事件
        list.querySelectorAll('.custom-element-item').forEach(item => {
            const id = item.dataset.id;
            item.querySelectorAll('.ce-field').forEach(input => {
                const field = input.dataset.field;
                const evt = (input.type === 'checkbox' || input.tagName === 'SELECT') ? 'change' : 'input';
                input.addEventListener(evt, () => {
                    updateCustomElementField(id, field, input);
                    updateSubtitlePreview();
                });
            });
            const upBtn = item.querySelector('.ce-up');
            if (upBtn) upBtn.addEventListener('click', () => moveCustomElement(id, -1));
            const downBtn = item.querySelector('.ce-down');
            if (downBtn) downBtn.addEventListener('click', () => moveCustomElement(id, 1));
            const delBtn = item.querySelector('.ce-delete');
            if (delBtn) delBtn.addEventListener('click', () => removeCustomElement(id));
        });
        initSliderFillsInList();
    }

    function updateCustomElementField(id, field, input) {
        const el = (state.customElements || []).find(e => e.id === id);
        if (!el) return;
        let val;
        if (input.type === 'checkbox') {
            val = input.checked;
        } else if (input.type === 'range' || input.type === 'number') {
            val = parseFloat(input.value);
        } else {
            val = input.value;
        }
        if (field === 'opacity_pct') {
            el.opacity = val / 100;
        } else if (field === 'font_size' || field === 'font_weight') {
            el[field] = parseInt(val) || 0;
        } else {
            el[field] = val;
        }
        markPresetCustom();
    }

    function addCustomElement(type) {
        const id = genCustomElementId();
        const defaults = {
            text: { label: '自定义文本', content: '自定义文本', font_size: 18, font_weight: 400, opacity: 0.9, align: 'center' },
            divider: { label: '分隔线', opacity: 0.3 },
            spacer: { label: '间距', font_size: 12 },
        };
        const d = defaults[type] || defaults.text;
        const el = {
            id, element_type: type, label: d.label, content: d.content || '',
            visible: true, color: '#ffffff', font_size: d.font_size || 18,
            font_weight: d.font_weight || 400, opacity: d.opacity ?? 0.9,
            prefix: '', align: d.align || 'center',
        };
        state.customElements.push(el);
        state.elementOrder.push(id);
        renderCustomElementList();
        renderElementOrderList();
        markPresetCustom();
        updateSubtitlePreview();
    }

    function removeCustomElement(id) {
        state.customElements = (state.customElements || []).filter(e => e.id !== id);
        state.elementOrder = (state.elementOrder || []).filter(k => k !== id);
        renderCustomElementList();
        renderElementOrderList();
        markPresetCustom();
        updateSubtitlePreview();
    }

    function syncCustomElementsOrder() {
        const customOrder = state.elementOrder.filter(k =>
            !(k === 'speaker' || k === 'original' || k === 'original2' || k === 'translation' || k === 'timestamp'));
        state.customElements.sort((a, b) => {
            const ia = customOrder.indexOf(a.id);
            const ib = customOrder.indexOf(b.id);
            return (ia < 0 ? 999 : ia) - (ib < 0 ? 999 : ib);
        });
    }

    function moveCustomElement(id, dir) {
        // 在 elementOrder 中移动
        const idx = state.elementOrder.indexOf(id);
        if (idx < 0) return;
        const newIdx = idx + dir;
        if (newIdx < 0 || newIdx >= state.elementOrder.length) return;
        const [item] = state.elementOrder.splice(idx, 1);
        state.elementOrder.splice(newIdx, 0, item);
        syncCustomElementsOrder();
        renderCustomElementList();
        renderElementOrderList();
        markPresetCustom();
        updateSubtitlePreview();
    }

    // ===== 元素排序列表 =====
    function renderElementOrderList() {
        const list = $('#element-order-list');
        if (!list) return;
        list.innerHTML = '';
        const labels = { speaker: '说话人', original: '原文', original2: '副原文', translation: '译文', timestamp: '时间戳' };
        const order = state.elementOrder || ['speaker', 'original', 'translation', 'timestamp'];
        order.forEach(key => {
            let label, isCustom = false;
            if (labels[key]) {
                label = labels[key];
            } else {
                const ce = (state.customElements || []).find(e => e.id === key);
                label = ce ? (ce.label || '自定义') : key;
                isCustom = true;
            }
            const item = document.createElement('div');
            item.className = 'element-order-item' + (isCustom ? ' is-custom' : '');
            item.dataset.key = key;
            item.draggable = true;
            item.innerHTML =
                `<span class="eo-drag-handle" title="拖拽排序">⋮⋮</span>` +
                `<span class="eo-label">${escapeHtmlSimple(label)}</span>` +
                `<div class="eo-move-btns">` +
                `<button class="icon-btn eo-up" type="button" title="上移">▲</button>` +
                `<button class="icon-btn eo-down" type="button" title="下移">▼</button>` +
                `</div>`;
            list.appendChild(item);
        });
        bindElementOrderEvents();
    }

    function bindElementOrderEvents() {
        const list = $('#element-order-list');
        if (!list) return;
        list.querySelectorAll('.element-order-item').forEach(item => {
            const key = item.dataset.key;
            const upBtn = item.querySelector('.eo-up');
            if (upBtn) upBtn.addEventListener('click', () => moveElementOrder(key, -1));
            const downBtn = item.querySelector('.eo-down');
            if (downBtn) downBtn.addEventListener('click', () => moveElementOrder(key, 1));
        });
        // 拖拽排序
        let dragKey = null;
        list.querySelectorAll('.element-order-item').forEach(item => {
            item.addEventListener('dragstart', (e) => {
                dragKey = item.dataset.key;
                item.classList.add('dragging');
                if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
            });
            item.addEventListener('dragend', () => {
                item.classList.remove('dragging');
                list.querySelectorAll('.element-order-item').forEach(n =>
                    n.classList.remove('drop-above', 'drop-below'));
                dragKey = null;
            });
            item.addEventListener('dragover', (e) => {
                e.preventDefault();
                if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
                const after = (e.clientY - item.getBoundingClientRect().top) > item.offsetHeight / 2;
                item.classList.toggle('drop-above', !after);
                item.classList.toggle('drop-below', after);
            });
            item.addEventListener('dragleave', () => {
                item.classList.remove('drop-above', 'drop-below');
            });
            item.addEventListener('drop', (e) => {
                e.preventDefault();
                if (!dragKey || dragKey === item.dataset.key) return;
                const after = (e.clientY - item.getBoundingClientRect().top) > item.offsetHeight / 2;
                reorderElementOrder(dragKey, item.dataset.key, after);
                item.classList.remove('drop-above', 'drop-below');
            });
        });
    }

    function moveElementOrder(key, dir) {
        const idx = state.elementOrder.indexOf(key);
        if (idx < 0) return;
        const newIdx = idx + dir;
        if (newIdx < 0 || newIdx >= state.elementOrder.length) return;
        const [item] = state.elementOrder.splice(idx, 1);
        state.elementOrder.splice(newIdx, 0, item);
        syncCustomElementsOrder();
        renderElementOrderList();
        renderCustomElementList();
        markPresetCustom();
        updateSubtitlePreview();
    }

    function reorderElementOrder(srcKey, dstKey, after) {
        const srcIdx = state.elementOrder.indexOf(srcKey);
        if (srcIdx < 0) return;
        const [item] = state.elementOrder.splice(srcIdx, 1);
        let dstIdx = state.elementOrder.indexOf(dstKey);
        if (dstIdx < 0) {
            state.elementOrder.push(item);
        } else {
            if (after) dstIdx += 1;
            state.elementOrder.splice(dstIdx, 0, item);
        }
        syncCustomElementsOrder();
        renderElementOrderList();
        renderCustomElementList();
        markPresetCustom();
        updateSubtitlePreview();
    }

    // 初始化所有滑块的填充进度
    function initSliderFills() {
        $$('input[type="range"]').forEach(slider => {
            const update = () => {
                const min = parseFloat(slider.min) || 0;
                const max = parseFloat(slider.max) || 100;
                const val = parseFloat(slider.value);
                const pct = max > min ? ((val - min) / (max - min)) * 100 : 0;
                slider.style.setProperty('--fill', pct + '%');
            };
            update();
            slider.addEventListener('input', update);
        });
    }

    function hexToRgba(hex, alpha) {
        const h = (hex || '#000000').replace('#', '');
        const r = parseInt(h.substring(0, 2), 16) || 0;
        const g = parseInt(h.substring(2, 4), 16) || 0;
        const b = parseInt(h.substring(4, 6), 16) || 0;
        return `rgba(${r}, ${g}, ${b}, ${alpha})`;
    }

    function buildTextShadow(color, strength) {
        if (!strength || strength === 0) return 'none';
        const offsets = [[0, 1], [0, -1], [1, 0], [-1, 0], [1, 1], [-1, 1], [1, -1], [-1, -1]];
        const intensity = Math.min(strength / 4, 2.5);
        return offsets.map(([x, y]) => `${x * intensity}px ${y * intensity}px ${intensity}px ${color}`).join(', ');
    }

    function collectSubtitleSettings() {
        const cfg = {};
        cfg.subtitle_font_family = ($('#subtitle-font-family') || {}).value || 'Microsoft YaHei';
        cfg.subtitle_font_size = parseInt(($('#subtitle-font-size') || {}).value || 32);
        cfg.subtitle_font_weight = parseInt(($('#subtitle-font-weight') || {}).value || 400);
        cfg.subtitle_bold = ($('#subtitle-bold') || {}).dataset.on === 'true';
        cfg.subtitle_italic = ($('#subtitle-italic') || {}).dataset.on === 'true';
        cfg.subtitle_font_color = ($('#subtitle-font-color') || {}).value || '#ffffff';
        cfg.subtitle_text_align = state.subtitleAlign || 'center';
        cfg.subtitle_letter_spacing = parseFloat(($('#subtitle-letter-spacing') || {}).value || 0);
        cfg.subtitle_line_height = parseFloat(($('#subtitle-line-height') || {}).value || 1.4);
        cfg.subtitle_text_shadow_color = ($('#subtitle-text-shadow-color') || {}).value || '#000000';
        cfg.subtitle_text_shadow_strength = parseInt(($('#subtitle-text-shadow-strength') || {}).value || 4);
        cfg.subtitle_bg_color = ($('#subtitle-bg-color') || {}).value || '#000000';
        cfg.subtitle_bg_opacity = parseInt(($('#subtitle-opacity') || {}).value || 60) / 100;
        cfg.subtitle_blur = parseInt(($('#subtitle-blur') || {}).value || 20);
        cfg.subtitle_padding_x = parseInt(($('#subtitle-padding-x') || {}).value || 24);
        cfg.subtitle_padding_y = parseInt(($('#subtitle-padding-y') || {}).value || 12);
        cfg.subtitle_max_lines = parseInt(($('#subtitle-lines') || {}).value || 3);
        cfg.subtitle_interim_color = ($('#subtitle-interim-color') || {}).value || '#ffffff';
        cfg.subtitle_interim_opacity = parseInt(($('#subtitle-interim-opacity') || {}).value || 70) / 100;
        // 元素级显示控制
        cfg.subtitle_show_original = ($('#subtitle-show-original') || {}).dataset.on !== 'false';
        cfg.subtitle_show_translation = ($('#subtitle-show-translation') || {}).dataset.on === 'true';
        cfg.subtitle_show_speaker = ($('#subtitle-show-speaker') || {}).dataset.on === 'true';
        cfg.subtitle_show_timestamp = ($('#subtitle-show-timestamp') || {}).dataset.on === 'true';
        cfg.subtitle_show_original_secondary = ($('#subtitle-show-original2') || {}).dataset.on === 'true';
        cfg.subtitle_layout = state.subtitleLayout || 'vertical';
        // 容器对齐锚点 + 卡片最大宽度
        cfg.subtitle_container_align_x = state.subtitleContainerAlignX || 'center';
        cfg.subtitle_container_align_y = state.subtitleContainerAlignY || 'bottom';
        cfg.subtitle_box_max_width = parseInt(($('#subtitle-box-max-width') || {}).value || 100);
        // 译文样式
        cfg.subtitle_translation_font_size = parseInt(($('#subtitle-translation-size') || {}).value || 24);
        cfg.subtitle_translation_font_color = ($('#subtitle-translation-color') || {}).value || '#ffffff';
        cfg.subtitle_translation_font_weight = parseInt(($('#subtitle-translation-weight') || {}).value || 400);
        cfg.subtitle_translation_opacity = parseInt(($('#subtitle-translation-opacity') || {}).value || 85) / 100;
        cfg.subtitle_translation_prefix = ($('#subtitle-translation-prefix') || {}).value || '';
        // 说话人样式
        cfg.subtitle_speaker_color = ($('#subtitle-speaker-color') || {}).value || '#818cf8';
        cfg.subtitle_speaker_font_size = parseInt(($('#subtitle-speaker-size') || {}).value || 16);
        cfg.subtitle_speaker_prefix = ($('#subtitle-speaker-prefix') || {}).value || '';
        // 时间戳样式
        cfg.subtitle_timestamp_color = ($('#subtitle-timestamp-color') || {}).value || '#a1a1aa';
        cfg.subtitle_timestamp_font_size = parseInt(($('#subtitle-timestamp-size') || {}).value || 14);
        cfg.subtitle_timestamp_format = ($('#subtitle-timestamp-format') || {}).value || 'HH:MM:SS';
        // 翻译配置（同声传译）
        const engineSel = $('#subtitle-translation-engine');
        cfg.subtitle_translation_enabled = cfg.subtitle_show_translation && (engineSel ? engineSel.value !== 'none' : false);
        cfg.subtitle_translation_target_lang = ($('#subtitle-translation-lang') || {}).value || '英文';
        cfg.subtitle_translation_engine = (engineSel || {}).value || 'none';
        // 自定义元素系统
        cfg.subtitle_custom_elements = (state.customElements || []).map(e => ({ ...e }));
        cfg.subtitle_element_order = (state.elementOrder && state.elementOrder.length > 0)
            ? [...state.elementOrder]
            : ['speaker', 'original', 'original2', 'translation', 'timestamp'];
        cfg.subtitle_preset = state.subtitlePreset || 'custom';
        return cfg;
    }

    /// 从当前 UI 收集完整场景对象（含样式/窗口/翻译配置）
    function collectSceneFromUI() {
        const flat = collectSubtitleSettings();
        if (!flat) return null;
        const scene = getCurrentScene();
        if (!scene) return null;
        flatToScene(scene, flat);

        // 同声传译配置
        if (!scene.translation) scene.translation = {};
        const engineSel = $('#subtitle-translation-engine');
        const langSel = $('#subtitle-translation-lang');
        const interimSw = $('#subtitle-translation-interim');
        if (engineSel) scene.translation.engine = engineSel.value || 'none';
        if (langSel) scene.translation.target_lang = langSel.value || '英文';
        if (interimSw) scene.translation.interim = interimSw.dataset.on === 'true';

        // 窗口控制（镜像实时开关状态）
        if (!scene.window) scene.window = {};
        const onTopSw = $('#subtitle-always-on-top');
        const clickSw = $('#subtitle-click-through');
        const obsSw = $('#subtitle-obs-mode');
        const autoFitSw = $('#subtitle-auto-fit');
        if (onTopSw) scene.window.always_on_top = onTopSw.dataset.on === 'true';
        if (clickSw) scene.window.click_through = clickSw.dataset.on === 'true';
        if (obsSw) scene.window.obs_mode = obsSw.dataset.on === 'true';
        if (autoFitSw) scene.window.auto_fit = autoFitSw.dataset.on === 'true';

        // 场景名称
        const nameInput = $('#subtitle-scene-name');
        if (nameInput && nameInput.value.trim()) scene.name = nameInput.value.trim();

        return scene;
    }

    async function loadSettings() {
        if (!invoke) {
            console.log('Tauri API not available, using defaults');
            setupDefaultSettings();
            updateSubtitlePreview();
            return;
        }

        try {
            const config = await invoke('get_config');
            state.config = config;
            populateSettings(config);
            applyTheme(config.theme || 'auto', false);
        } catch (err) {
            console.error('Failed to load config:', err);
            // 初始化默认配置，确保用户操作（如切换模式）能被保存
            state.config = { basic: { dictation_mode: state.dictationMode } };
            setupDefaultSettings();
        }

        // 初始渲染（未下载状态）
        renderModelCards();
        // 异步检测已下载状态并刷新
        loadModelStatus();

        try {
            const dir = await invoke('get_models_dir');
            const dirEl = $('#models-dir-path');
            if (dirEl) dirEl.textContent = dir;
        } catch (err) {
            console.error('Failed to get models dir:', err);
            const dirEl = $('#models-dir-path');
            if (dirEl) dirEl.textContent = '无法加载';
        }
    }

    /// 加载系统音频输入设备列表，填充到两个下拉：
    /// - #setting-input-device：语音输入设备（整段+流式）
    /// - #setting-subtitle-input-device：字幕识别音源
    /// 保留当前选中值，若设备已不存在则回退到"系统默认"
    async function loadInputDevices() {
        if (!invoke) return;
        try {
            const [devices, defaultName] = await invoke('list_input_devices');
            const fills = [
                { id: 'setting-input-device', current: state.config?.basic?.input_device || '' },
                { id: 'setting-subtitle-input-device', current: state.config?.subtitle?.subtitle_input_device || '' },
            ];
            for (const { id, current } of fills) {
                const sel = $('#' + id);
                if (!sel) continue;
                const stillExists = !current || devices.includes(current);
                sel.innerHTML = '<option value="">系统默认</option>' +
                    devices.map(name => `<option value="${escapeHtml(name)}">${escapeHtml(name)}</option>`).join('');
                sel.value = stillExists ? current : '';
            }
            // 缓存默认设备名，便于 UI 提示
            state.defaultInputDevice = defaultName || '';
        } catch (err) {
            console.error('Failed to load input devices:', err);
        }
    }

    /// 检查 whisper.cpp 引擎健康状态并更新 UI 徽章
    async function checkEngineStatus() {
        const badge = $('#engine-status-badge');
        const hint = $('#engine-detail-hint');
        const dlBtn = $('#btn-download-engine');
        if (!badge) return;

        badge.textContent = '检测中...';
        badge.className = 'mm-engine-badge';
        if (hint) hint.textContent = '';

        try {
            const health = await invoke('check_whisper_binary_health');
            if (health.status === 'ok') {
                badge.textContent = '引擎就绪';
                badge.className = 'mm-engine-badge status-ok';
                if (hint) hint.textContent = '';
                if (dlBtn) dlBtn.textContent = '重新下载';
            } else if (health.status === 'missing') {
                badge.textContent = '未下载';
                badge.className = 'mm-engine-badge status-missing';
                if (hint) hint.textContent = '点击"下载引擎"自动下载，或手动下载 whisper-bin-x64.zip 解压所有文件到 whisper-bin 文件夹';
                if (dlBtn) dlBtn.textContent = '下载引擎';
            } else if (health.status === 'corrupt') {
                badge.textContent = '引擎损坏';
                badge.className = 'mm-engine-badge status-corrupt';
                if (hint) hint.textContent = health.message + '。请点击"重新下载"';
                if (dlBtn) dlBtn.textContent = '重新下载';
            }
        } catch (err) {
            badge.textContent = '检测失败';
            badge.className = 'mm-engine-badge status-corrupt';
            if (hint) hint.textContent = String(err);
        }
    }

    // 模型元数据：key、文件名、大小、速度等级(1-4)、速度文案、推理耗时、推荐场景
    const WHISPER_MODELS_META = [
        { key: 'tiny',   file: 'ggml-tiny.bin',   sizeMB: 75,   speedLevel: 1, speed: '极快', desc: '2秒音频≈1秒',   recommend: '追求速度可选' },
        { key: 'base',   file: 'ggml-base.bin',   sizeMB: 142,  speedLevel: 2, speed: '快',   desc: '2秒音频≈2-3秒', recommend: '推荐 · 精度速度均衡' },
        { key: 'small',  file: 'ggml-small.bin',  sizeMB: 466,  speedLevel: 3, speed: '慢',   desc: '2秒音频≈9秒',   recommend: '需高精度且硬件强' },
        { key: 'medium', file: 'ggml-medium.bin', sizeMB: 1530, speedLevel: 4, speed: '很慢', desc: '2秒音频≈20秒+',  recommend: '仅高配机' },
    ];

    // 模型状态缓存（由后端 list_available_models + 当前配置决定）
    // { 'ggml-tiny.bin': { downloaded: true, available: true, size: 77852928 } }
    let modelStatusMap = {};
    let currentModelFile = '';
    let downloadingModel = null;
    let modelProgressUnlisten = null;

    /// 统一渲染模型卡片网格
    function renderModelCards() {
        const grid = $('#model-store');
        if (!grid) return;

        grid.innerHTML = WHISPER_MODELS_META.map(m => {
            const status = modelStatusMap[m.file];
            const isDownloaded = status?.downloaded === true;
            const isAvailable = status?.available !== false;
            const isCorrupt = isDownloaded && !isAvailable;
            const isCurrent = m.file === currentModelFile;
            const isDownloading = downloadingModel === m.key;

            // 卡片状态 class
            const stateClass = isCurrent ? 'is-current'
                : isCorrupt ? 'is-corrupt'
                : isDownloaded ? 'is-downloaded'
                : 'is-empty';

            // 主操作按钮（智能切换）
            let primaryBtn;
            if (isDownloading) {
                primaryBtn = `<div class="mm-download-row">
                    <button class="mm-card-btn mm-btn-progress" disabled>
                        <span class="mm-progress-text">0%</span>
                    </button>
                    <button class="mm-card-btn mm-btn-cancel" data-action="cancel" data-key="${m.key}">取消</button>
                </div>
                <div class="mm-progress-track"><div class="mm-progress-fill" style="width:0%"></div></div>`;
            } else if (isCurrent) {
                primaryBtn = `<button class="mm-card-btn mm-btn-current" disabled>使用中</button>`;
            } else if (isCorrupt) {
                primaryBtn = `<button class="mm-card-btn mm-btn-repair" data-action="redownload" data-key="${m.key}">重新下载</button>`;
            } else if (isDownloaded) {
                primaryBtn = `<button class="mm-card-btn mm-btn-use" data-action="use" data-file="${m.file}">使用</button>`;
            } else {
                primaryBtn = `<button class="mm-card-btn mm-btn-download" data-action="download" data-key="${m.key}">下载</button>`;
            }

            // 删除按钮（仅已下载且非使用中）
            const deleteBtn = (isDownloaded && !isCurrent && !isDownloading)
                ? `<button class="mm-card-delete" data-action="delete" data-file="${m.file}" title="删除">×</button>`
                : '';

            // 状态标签
            let statusBadge;
            if (isDownloading) statusBadge = '<span class="mm-card-status status-downloading">下载中</span>';
            else if (isCurrent) statusBadge = '<span class="mm-card-status status-current">使用中</span>';
            else if (isCorrupt) statusBadge = '<span class="mm-card-status status-corrupt">校验失败</span>';
            else if (isDownloaded) statusBadge = '<span class="mm-card-status status-ready">已就绪</span>';
            else statusBadge = '<span class="mm-card-status status-empty">未下载</span>';

            // 实际大小（已下载时显示真实大小）
            const sizeText = isDownloaded && status?.size
                ? `${(status.size / 1048576).toFixed(1)} MB`
                : `${m.sizeMB} MB`;

            // 速度等级（1-4 个点）
            const speedDots = Array.from({ length: 4 }, (_, i) =>
                `<span class="mm-speed-dot${i < m.speedLevel ? ' active' : ''}"></span>`
            ).join('');

            return `
                <div class="mm-card ${stateClass}" data-model-key="${m.key}" data-file="${m.file}">
                    <div class="mm-card-top">
                        <div class="mm-card-name">${m.key}</div>
                        ${deleteBtn}
                    </div>
                    <div class="mm-card-speed">
                        <span class="mm-speed-label">${m.speed}</span>
                        <span class="mm-speed-dots">${speedDots}</span>
                    </div>
                    <div class="mm-card-desc">${m.desc}</div>
                    <div class="mm-card-recommend">${m.recommend}</div>
                    <div class="mm-card-footer">
                        <div class="mm-card-meta">
                            ${statusBadge}
                            <span class="mm-card-size">${sizeText}</span>
                        </div>
                    </div>
                    ${primaryBtn}
                </div>
            `;
        }).join('');

        // 绑定主操作按钮
        $$('.mm-card-btn[data-action]').forEach(btn => {
            btn.addEventListener('click', () => {
                const action = btn.dataset.action;
                if (action === 'download' || action === 'redownload') {
                    downloadModel(btn.dataset.key);
                } else if (action === 'cancel') {
                    cancelModelDownload(btn.dataset.key);
                } else if (action === 'use') {
                    setModelAsCurrent(btn.dataset.file);
                }
            });
        });

        // 绑定删除按钮
        $$('.mm-card-delete').forEach(btn => {
            btn.addEventListener('click', () => deleteModel(btn.dataset.file));
        });
    }

    /// 加载已下载模型状态 + 当前使用模型，然后渲染
    async function loadModelStatus() {
        if (!invoke) return;
        try {
            // 并行：已下载模型列表 + 当前配置
            const [models, cfg] = await Promise.all([
                invoke('list_available_models'),
                invoke('get_config').catch(() => null),
            ]);

            // 构建状态 map
            const map = {};
            (models || []).forEach(m => {
                map[m.name] = {
                    downloaded: true,
                    available: m.available !== false,
                    size: m.size || 0,
                };
            });
            modelStatusMap = map;
            currentModelFile = cfg?.model?.local_whisper_model || '';

            renderModelCards();
        } catch (err) {
            console.error('[loadModelStatus] 失败:', err);
            const grid = $('#model-store');
            if (grid) grid.innerHTML = `<div class="mm-empty">加载失败: ${err}</div>`;
        }
    }

    /// 触发模型下载（带确认）
    async function downloadModel(modelKey) {
        if (!invoke || downloadingModel) return;

        // 下载前确认（含源选择器）
        const meta = WHISPER_MODELS_META.find(m => m.key === modelKey);
        const sizeText = meta ? `约 ${meta.sizeMB} MB` : '';
        const { confirmed, selected: source } = await showConfirmDialog(
            '下载模型',
            `确定下载 ${modelKey} 模型（${sizeText}）吗？下载期间可随时取消。`,
            '开始下载',
            [
                { label: 'HuggingFace', value: 'hf' },
                { label: '直链', value: 'custom' },
            ]
        );
        if (!confirmed) return;

        downloadingModel = modelKey;

        // 监听进度
        if (listen && !modelProgressUnlisten) {
            modelProgressUnlisten = await listen('model-download-progress', (event) => {
                const p = event.payload;
                if (!p || !p.model) return;
                const card = $(`.mm-card[data-model-key="${p.model}"]`);
                if (!card) return;
                const text = card.querySelector('.mm-progress-text');
                const fill = card.querySelector('.mm-progress-fill');
                const sizeEl = card.querySelector('.mm-card-size');

                // total 已知：显示百分比；未知：显示已下载 MB
                if (p.total && p.total > 0) {
                    if (text) text.textContent = `${p.progress || 0}%`;
                    if (fill) fill.style.width = `${p.progress || 0}%`;
                } else {
                    const dlMB = (p.downloaded || 0) / 1048576;
                    if (text) text.textContent = `${dlMB.toFixed(1)} MB`;
                    if (fill) fill.style.width = '0%';
                }
                // 实时显示速度 + 剩余时间
                if (sizeEl && (p.progress || 0) < 100 && p.speed) {
                    sizeEl.textContent = `${(p.speed || 0).toFixed(1)} MB/s${p.eta ? ` · ${p.eta}s` : ''}`;
                }
            });
        }

        renderModelCards();

        try {
            await invoke('download_whisper_model', { modelName: modelKey, source: source || 'hf' });
        } catch (err) {
            console.error('[downloadModel] 失败:', err);
            if (String(err).includes('取消')) {
                addLog('info', `已取消下载: ${modelKey}`, 'settings');
            } else {
                alert('模型下载失败: ' + err);
            }
        } finally {
            downloadingModel = null;
            await loadModelStatus();
        }
    }

    /// 取消当前模型下载
    async function cancelModelDownload(modelKey) {
        if (!invoke || !downloadingModel) return;
        try {
            await invoke('cancel_download');
        } catch (err) {
            console.error('[cancelModelDownload] 失败:', err);
        }
        // 立即清空下载态并重渲染：用户可马上重新下载
        downloadingModel = null;
        addLog('info', `已取消下载: ${modelKey}`, 'settings');
        renderModelCards();
        // 后端收到取消后很快返回，finally 里会再次 loadModelStatus 刷新真实状态
    }

    /// 设为当前使用
    async function setModelAsCurrent(fileName) {
        if (!invoke) return;
        try {
            const cfg = await invoke('get_config');
            cfg.model.local_whisper_model = fileName;
            await invoke('save_config', { newConfig: cfg });
            currentModelFile = fileName;
            renderModelCards();
            addLog('info', `已切换本地模型: ${fileName}`, 'settings');
        } catch (err) {
            console.error('[setModelAsCurrent] 失败:', err);
            alert('设为当前失败: ' + err);
        }
    }

    /// 删除模型
    async function deleteModel(fileName) {
        if (!invoke) return;
        const { confirmed } = await showConfirmDialog(
            '删除模型',
            `确定删除 ${fileName} 吗？此操作不可恢复。`,
            '删除'
        );
        if (!confirmed) return;
        try {
            await invoke('delete_whisper_model', { modelName: fileName });
            await loadModelStatus();
            addLog('info', `已删除模型: ${fileName}`, 'settings');
        } catch (err) {
            console.error('[deleteModel] 失败:', err);
            alert('删除失败: ' + err);
        }
    }

    /// 打开模型目录（优先使用后端 open_directory 命令，回退到 shell.open）
    async function openModelsDirectory() {
        if (!invoke) return;
        try {
            const dir = await invoke('get_models_dir');
            try {
                await invoke('open_directory', { path: dir });
            } catch (backendErr) {
                // 后端命令不可用时回退到 shell 插件
                if (window.__TAURI__?.shell?.open) {
                    await window.__TAURI__.shell.open(dir);
                } else {
                    throw new Error('无法打开目录：后端命令和 shell 插件均不可用');
                }
            }
        } catch (err) {
            console.error('[openModelsDirectory] 失败:', err);
            alert('打开目录失败: ' + err);
        }
    }

    /// 显示未保存更改确认对话框，返回 'save' | 'discard' | 'cancel'
    function showUnsavedChangesDialog() {
        return new Promise((resolve) => {
            const overlay = document.createElement('div');
            overlay.className = 'modal-overlay';
            overlay.style.display = 'flex';
            overlay.innerHTML = `
                <div class="modal-content confirm-modal">
                    <div class="modal-header">
                        <h2>未保存的更改</h2>
                    </div>
                    <div class="modal-body">
                        <p class="confirm-message">设置已修改但尚未保存，是否保存更改？</p>
                    </div>
                    <div class="modal-footer">
                        <button class="secondary-btn" data-action="cancel">取消</button>
                        <button class="secondary-btn" data-action="discard">不保存</button>
                        <button class="solid-btn" data-action="save">保存</button>
                    </div>
                </div>
            `;
            document.body.appendChild(overlay);
            const close = (result) => { overlay.remove(); resolve(result); };
            overlay.querySelector('[data-action="save"]').addEventListener('click', () => close('save'));
            overlay.querySelector('[data-action="discard"]').addEventListener('click', () => close('discard'));
            overlay.querySelector('[data-action="cancel"]').addEventListener('click', () => close('cancel'));
            overlay.addEventListener('click', (e) => { if (e.target === overlay) close('cancel'); });
            const onKey = (e) => {
                if (e.key === 'Escape') {
                    document.removeEventListener('keydown', onKey);
                    close('cancel');
                }
            };
            document.addEventListener('keydown', onKey);
        });
    }

    /// 显示本地模型使用教程模态框
    function showHelpModal() {
        const modal = $('#help-modal');
        if (modal) modal.style.display = 'flex';
    }

    /// 关闭本地模型使用教程模态框
    function closeHelpModal() {
        const modal = $('#help-modal');
        if (modal) modal.style.display = 'none';
    }

    /// 显示通用确认对话框，返回 {confirmed, selected}
    /// confirmText 为确认按钮文字（默认"确认"）
    /// selectOptions 可选：在 footer 左侧渲染下拉选择器，selected 为选中值
    function showConfirmDialog(title, message, confirmText, selectOptions) {
        return new Promise((resolve) => {
            const overlay = document.createElement('div');
            overlay.className = 'modal-overlay';
            overlay.style.display = 'flex';

            // 源选择器 HTML（仅当传入 selectOptions 时渲染）
            let selectHtml = '';
            if (selectOptions && selectOptions.length > 0) {
                const optionsHtml = selectOptions.map((opt, i) =>
                    `<option value="${escapeHtml(String(opt.value))}"${i === 0 ? ' selected' : ''}>${escapeHtml(opt.label)}</option>`
                ).join('');
                selectHtml = `
                    <div class="confirm-source-select-wrap">
                        <span class="confirm-source-label">源</span>
                        <select class="confirm-source-select">${optionsHtml}</select>
                    </div>
                `;
            }

            const safeTitle = escapeHtml(title);
            const safeMessage = escapeHtml(message).replace(/\n/g, '<br>');
            const safeConfirmText = escapeHtml(confirmText || '确认');
            overlay.innerHTML = `
                <div class="modal-content confirm-modal">
                    <div class="modal-header">
                        <h2>${safeTitle}</h2>
                    </div>
                    <div class="modal-body">
                        <p class="confirm-message">${safeMessage}</p>
                    </div>
                    <div class="modal-footer">
                        ${selectHtml}
                        <button class="secondary-btn" data-action="cancel">取消</button>
                        <button class="solid-btn" data-action="confirm">${safeConfirmText}</button>
                    </div>
                </div>
            `;
            document.body.appendChild(overlay);

            const close = (result) => {
                overlay.remove();
                resolve(result);
            };
            overlay.querySelector('[data-action="confirm"]').addEventListener('click', () => {
                const sel = overlay.querySelector('.confirm-source-select');
                close({ confirmed: true, selected: sel ? sel.value : undefined });
            });
            overlay.querySelector('[data-action="cancel"]').addEventListener('click', () => close({ confirmed: false }));
            overlay.addEventListener('click', (e) => {
                if (e.target === overlay) close({ confirmed: false });
            });
            const onKey = (e) => {
                if (e.key === 'Escape') {
                    document.removeEventListener('keydown', onKey);
                    close({ confirmed: false });
                }
            };
            document.addEventListener('keydown', onKey);
        });
    }

    function setupDefaultSettings() {
        const dirEl = $('#models-dir-path');
        if (dirEl) dirEl.textContent = '运行时加载';
        // 浏览器模式兜底：生成默认场景
        if (!state.scenes.length) {
            state.scenes = [sceneFromLegacy({})];
            state.currentSceneId = 'default';
        }
    }

    /// 将当前场景的配置填充到字幕设置 UI
    function populateSubtitleUiFromScene(scene, config) {
        if (!scene) return;
        const s = scene.style || {};
        const setVal = (id, value) => {
            const el = $(`#${id}`);
            if (el && value !== undefined && value !== null) el.value = value;
        };

        // ===== 样式 =====
        setVal('subtitle-font-family', s.font_family);
        setVal('subtitle-font-size', s.font_size);
        setVal('subtitle-font-weight', s.font_weight);
        setVal('subtitle-font-color', s.font_color);
        setVal('subtitle-letter-spacing', s.letter_spacing);
        setVal('subtitle-line-height', s.line_height);
        setVal('subtitle-text-shadow-color', s.text_shadow_color);
        setVal('subtitle-text-shadow-strength', s.text_shadow_strength);
        setVal('subtitle-bg-color', s.bg_color);
        setVal('subtitle-opacity', Math.round((s.bg_opacity ?? 0.6) * 100));
        setVal('subtitle-blur', s.blur);
        setVal('subtitle-padding-x', s.padding_x);
        setVal('subtitle-padding-y', s.padding_y);
        setVal('subtitle-lines', s.max_lines);
        setVal('subtitle-interim-color', s.interim_color);
        setVal('subtitle-interim-opacity', Math.round((s.interim_opacity ?? 0.7) * 100));

        // 元素级显示控制
        const showOrigSw = $('#subtitle-show-original');
        if (showOrigSw) showOrigSw.dataset.on = s.show_original !== false ? 'true' : 'false';
        const showTransSw = $('#subtitle-show-translation');
        if (showTransSw) showTransSw.dataset.on = s.show_translation === true ? 'true' : 'false';
        const showSpeakerSw = $('#subtitle-show-speaker');
        if (showSpeakerSw) showSpeakerSw.dataset.on = s.show_speaker === true ? 'true' : 'false';
        const showTsSw = $('#subtitle-show-timestamp');
        if (showTsSw) showTsSw.dataset.on = s.show_timestamp === true ? 'true' : 'false';
        const showOrig2Sw = $('#subtitle-show-original2');
        if (showOrig2Sw) showOrig2Sw.dataset.on = s.show_original_secondary === true ? 'true' : 'false';

        // 译文样式
        setVal('subtitle-translation-color', s.translation_font_color);
        setVal('subtitle-translation-size', s.translation_font_size);
        setVal('subtitle-translation-weight', s.translation_font_weight);
        setVal('subtitle-translation-opacity', Math.round((s.translation_opacity ?? 0.85) * 100));
        setVal('subtitle-translation-prefix', s.translation_prefix);

        // 说话人样式
        setVal('subtitle-speaker-color', s.speaker_color);
        setVal('subtitle-speaker-size', s.speaker_font_size);
        setVal('subtitle-speaker-prefix', s.speaker_prefix);

        // 时间戳样式
        setVal('subtitle-timestamp-color', s.timestamp_color);
        setVal('subtitle-timestamp-size', s.timestamp_font_size);
        setVal('subtitle-timestamp-format', s.timestamp_format);

        // ===== 识别音源（会话级，所有场景共享） =====
        const sub = (config && config.subtitle) || {};
        const subDeviceSel = $('#setting-subtitle-input-device');
        if (subDeviceSel) subDeviceSel.value = sub.subtitle_input_device || '';

        // 字幕开关热键
        const subHotkeyInput = $('#subtitle-hotkey');
        if (subHotkeyInput) subHotkeyInput.value = virtualKeyToName(sub.subtitle_hotkey || 0x76);

        const audioSource = sub.subtitle_audio_source || 'microphone';
        const sourceActiveBtn = $(`#subtitle-audio-source .seg-btn[data-source="${audioSource}"]`);
        $$('#subtitle-audio-source .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === sourceActiveBtn);
        });
        if (sourceActiveBtn) moveSegIndicator(sourceActiveBtn);
        updateSubtitleSourceUI(audioSource);

        // 开关：加粗 / 斜体（bold 通过 font_weight>=700 体现）
        const boldSwitch = $('#subtitle-bold');
        const isBold = (s.font_weight || 400) >= 700;
        if (boldSwitch) boldSwitch.dataset.on = isBold ? 'true' : 'false';
        const italicSwitch = $('#subtitle-italic');
        if (italicSwitch) italicSwitch.dataset.on = s.italic === true ? 'true' : 'false';

        // 对齐
        const align = s.text_align || 'center';
        state.subtitleAlign = align;
        const alignActiveBtn = $(`#subtitle-text-align .seg-btn[data-mode="${align}"]`);
        $$('#subtitle-text-align .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === alignActiveBtn);
        });
        if (alignActiveBtn) moveSegIndicator(alignActiveBtn);

        // 布局方向
        const layout = s.layout || 'vertical';
        state.subtitleLayout = layout;
        const layoutActiveBtn = $(`#subtitle-layout .seg-btn[data-mode="${layout}"]`);
        $$('#subtitle-layout .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === layoutActiveBtn);
        });
        if (layoutActiveBtn) moveSegIndicator(layoutActiveBtn);

        // 容器对齐锚点 + 卡片最大宽度
        const alignX = s.container_align_x || 'center';
        state.subtitleContainerAlignX = alignX;
        const axBtn = $(`#subtitle-container-align-x .seg-btn[data-mode="${alignX}"]`);
        $$('#subtitle-container-align-x .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === axBtn);
        });
        if (axBtn) moveSegIndicator(axBtn);

        const alignY = s.container_align_y || 'bottom';
        state.subtitleContainerAlignY = alignY;
        const ayBtn = $(`#subtitle-container-align-y .seg-btn[data-mode="${alignY}"]`);
        $$('#subtitle-container-align-y .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === ayBtn);
        });
        if (ayBtn) moveSegIndicator(ayBtn);

        setVal('subtitle-box-max-width', s.box_max_width || 100);

        // 预设模板
        state.subtitlePreset = s.preset || 'custom';
        const presetActiveBtn = $(`#subtitle-preset .seg-btn[data-preset="${state.subtitlePreset}"]`);
        $$('#subtitle-preset .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === presetActiveBtn);
        });
        if (presetActiveBtn) moveSegIndicator(presetActiveBtn);

        // 自定义元素系统
        state.customElements = Array.isArray(s.custom_elements)
            ? s.custom_elements.map(e => ({ ...e }))
            : [];
        state.elementOrder = (Array.isArray(s.element_order) && s.element_order.length > 0)
            ? [...s.element_order]
            : ['speaker', 'original', 'translation', 'timestamp'];
        renderCustomElementList();
        renderElementOrderList();

        // ===== 同声传译设置 =====
        const trans = scene.translation || {};
        setVal('subtitle-translation-engine', trans.engine || 'none');
        setVal('subtitle-translation-lang', trans.target_lang || '英文');
        const interimSw = $('#subtitle-translation-interim');
        if (interimSw) interimSw.dataset.on = trans.interim !== false ? 'true' : 'false';

        // LLM 共享接口配置
        setVal('subtitle-translation-llm-url', sub.subtitle_translation_llm_api_url || '');
        setVal('subtitle-translation-llm-key', sub.subtitle_translation_llm_api_key || '');
        setVal('subtitle-translation-llm-model', sub.subtitle_translation_llm_model || '');

        // ===== 窗口控制（镜像 scene.window） =====
        const w = scene.window || {};
        const onTopSw = $('#subtitle-always-on-top');
        if (onTopSw) onTopSw.dataset.on = w.always_on_top !== false ? 'true' : 'false';
        const clickSw = $('#subtitle-click-through');
        if (clickSw) clickSw.dataset.on = w.click_through === true ? 'true' : 'false';
        const obsSw = $('#subtitle-obs-mode');
        if (obsSw) obsSw.dataset.on = w.obs_mode === true ? 'true' : 'false';

        const autoFitSw = $('#subtitle-auto-fit');
        if (autoFitSw) autoFitSw.dataset.on = w.auto_fit !== false ? 'true' : 'false';

        // 场景名称
        const nameInput = $('#subtitle-scene-name');
        if (nameInput) nameInput.value = scene.name || '';

        updateTranslationUI();
        updateSubtitlePreview();
    }

    function populateSettings(config) {
        state.populatingSettings = true;
        _doPopulateSettings(config);
        state.populatingSettings = false;
        state.settingsDirty = false;
    }

    function _doPopulateSettings(config) {
        if (config.model_selection) {
            const batchSel = $('#setting-batch-model');
            const streamSel = $('#setting-stream-model');
            const subSel = $('#setting-subtitle-model');
            if (batchSel && config.model_selection.batch_model) batchSel.value = config.model_selection.batch_model;
            if (streamSel && config.model_selection.stream_model) streamSel.value = config.model_selection.stream_model;
            if (subSel && config.model_selection.subtitle_model) subSel.value = config.model_selection.subtitle_model;
        }

        if (config.model) {
            const sfKey = $('#setting-sf-key');
            const groqKey = $('#setting-groq-key');
            const doubaoKey = $('#setting-doubao-key');
            if (sfKey) sfKey.value = config.model.siliconflow_api_key || '';
            if (groqKey) groqKey.value = config.model.groq_api_key || '';
            if (doubaoKey) doubaoKey.value = config.model.doubao_api_key || '';

            // 自定义模型提供商配置
            const customUrl = $('#setting-custom-api-url');
            const customModel = $('#setting-custom-model-name');
            const customKey = $('#setting-custom-key');
            if (customUrl) customUrl.value = config.model.custom_api_url || '';
            if (customModel) customModel.value = config.model.custom_model_name || '';
            if (customKey) customKey.value = config.model.custom_api_key || '';

            // 本地 Whisper 性能调优字段
            const wThreads = $('#setting-whisper-threads');
            const wGreedy = $('#setting-whisper-greedy');
            const wNoFallback = $('#setting-whisper-no-fallback');
            if (wThreads) wThreads.value = config.model.local_whisper_threads ?? 0;
            if (wGreedy) wGreedy.checked = !!config.model.local_whisper_greedy;
            if (wNoFallback) wNoFallback.checked = !!config.model.local_whisper_no_fallback;
        }

        if (config.basic) {
            const hotkeyInput = $('#setting-hotkey-batch');
            const outputMode = $('#setting-output-mode');
            const outputLang = $('#setting-output-lang');
            if (hotkeyInput) hotkeyInput.value = virtualKeyToName(config.basic.hotkey || 0x71);
            if (outputMode) outputMode.value = config.basic.output_mode || 'clipboard';
            if (outputLang) outputLang.value = config.basic.output_language || 'auto';

            // 语音输入设备：options 由 loadInputDevices 异步填充，这里只设选中值
            const inputDeviceSel = $('#setting-input-device');
            if (inputDeviceSel) inputDeviceSel.value = config.basic.input_device || '';

            // 音频采集质量偏好
            const audioProfile = $('#setting-audio-profile');
            const audioDownmix = $('#setting-audio-downmix');
            const audioSampleFmt = $('#setting-audio-sample-format');
            const audioSampleRate = $('#setting-audio-sample-rate');
            const audioChannels = $('#setting-audio-channels');
            if (audioProfile) audioProfile.value = config.basic.audio_profile || 'standard';
            if (audioDownmix) audioDownmix.value = config.basic.audio_downmix || 'strongest';
            if (audioSampleFmt) audioSampleFmt.value = config.basic.audio_sample_format || 'auto';
            if (audioSampleRate) audioSampleRate.value = config.basic.audio_sample_rate || 'auto';
            if (audioChannels) audioChannels.value = config.basic.audio_channels || 'auto';

            const dictationMode = config.basic.dictation_mode || 'batch';
            state.dictationMode = dictationMode;
            updateHotkeyHint();
        }

        // 无论 config.basic 是否存在，都同步识别模式 UI
        const dictMode = (config.basic && config.basic.dictation_mode) || 'batch';
        state.dictationMode = dictMode;
        const dictActiveBtn = $(`#dictation-mode .seg-btn[data-mode="${dictMode}"]`);
        $$('#dictation-mode .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === dictActiveBtn);
        });
        if (dictActiveBtn) moveSegIndicator(dictActiveBtn);

        if (config.streaming) {
            // 流式配置仍然保留（资源 ID、模型名等），但不再有独立热键输入
        }

        if (config.subtitle) {
            loadScenesIntoState(config);
            const scene = getCurrentScene();
            populateSubtitleUiFromScene(scene, config);
        }

        if (config.features) {
            const punctuation = $('#setting-punctuation');
            const emoji = $('#setting-emoji');
            const indicator = $('#setting-indicator');
            const postProcessor = $('#setting-post-processor');
            if (punctuation) punctuation.checked = config.features.allow_punctuation !== false;
            if (emoji) emoji.checked = config.features.allow_emoji !== false;
            if (indicator) indicator.checked = config.features.enable_indicator !== false;
            if (postProcessor) postProcessor.checked = config.features.enable_post_processor === true;
        }

        if (config.llm_post) {
            const llmEnable = $('#setting-llm-post-enable');
            const llmUrl = $('#setting-llm-api-url');
            const llmKey = $('#setting-llm-api-key');
            const llmModel = $('#setting-llm-model');
            const llmPrompt = $('#setting-llm-system-prompt');
            if (llmEnable) llmEnable.checked = config.llm_post.enable === true;
            if (llmUrl) llmUrl.value = config.llm_post.api_url || '';
            if (llmKey) llmKey.value = config.llm_post.api_key || '';
            if (llmModel) llmModel.value = config.llm_post.model || '';
            if (llmPrompt) llmPrompt.value = config.llm_post.system_prompt || '';
            updateLlmPostConfigVisibility();
        }

        if (config.advanced) {
            const triggerMode = config.advanced.trigger_mode || 'hold';
            state.triggerMode = triggerMode;
            // 仅作用于触发模式选择组，避免覆盖其他 seg-btn（如识别模式）的选中状态
            $$('#trigger-mode .seg-btn').forEach(btn => {
                btn.classList.toggle('active', btn.dataset.mode === triggerMode);
            });
            updateMicHint();
        }

        if (config.vad) {
            const sensSlider = $('#setting-vad-sensitivity');
            const silenceSlider = $('#setting-vad-silence');
            if (sensSlider && config.vad.vad_sensitivity !== undefined) {
                sensSlider.value = Math.round(config.vad.vad_sensitivity * 100);
                if (sensSlider.nextElementSibling) {
                    sensSlider.nextElementSibling.textContent = `${Math.round(config.vad.vad_sensitivity * 100)}%`;
                }
            }
            if (silenceSlider && config.vad.vad_silence_duration_ms) {
                silenceSlider.value = config.vad.vad_silence_duration_ms;
                if (silenceSlider.nextElementSibling) {
                    silenceSlider.nextElementSibling.textContent = `${config.vad.vad_silence_duration_ms}ms`;
                }
            }
        }

        // 主题选择器按钮状态
        const theme = config.theme || 'auto';
        $$('.theme-selector .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn.dataset.theme === theme);
        });

        updateModelBadge();
    }

    /// 应用主题到 documentElement，并缓存到 localStorage（启动时避免闪烁）
    function applyTheme(theme, persist) {
        const resolved = theme || 'auto';
        document.documentElement.setAttribute('data-theme', resolved);
        try { localStorage.setItem('v2t-theme', resolved); } catch (e) {}
        if (persist && state.config) {
            state.config.theme = resolved;
            if (invoke) {
                invoke('save_config', { newConfig: state.config }).catch(err =>
                    console.warn('Failed to persist theme:', err)
                );
            }
        }
    }

    /// 初始化可折叠设置分组
    function initCollapsibleSections() {
        const headers = $$('.settings-section .section-header');
        headers.forEach(header => {
            if (header.dataset.bound) return;
            header.dataset.bound = '1';
            header.addEventListener('click', () => {
                const section = header.closest('.settings-section');
                if (!section) return;
                section.classList.toggle('collapsed');
                const collapsed = section.classList.contains('collapsed');
                const title = (section.querySelector('h3') || {}).textContent || section.id || 'section';
                try {
                    const store = JSON.parse(localStorage.getItem('v2t-collapsed-sections') || '{}');
                    store[title] = collapsed;
                    localStorage.setItem('v2t-collapsed-sections', JSON.stringify(store));
                } catch (e) {}
            });
        });

        // 字幕设置卡片折叠
        const cardTitles = $$('.subtitle-card-title');
        cardTitles.forEach(title => {
            if (title.dataset.bound) return;
            title.dataset.bound = '1';
            title.addEventListener('click', () => {
                const card = title.closest('.subtitle-card');
                if (!card) return;
                card.classList.toggle('collapsed');
            });
        });

        // 恢复折叠状态
        try {
            const store = JSON.parse(localStorage.getItem('v2t-collapsed-sections') || '{}');
            $$('.settings-section').forEach(section => {
                const title = (section.querySelector('h3') || {}).textContent || section.id || 'section';
                if (store[title]) section.classList.add('collapsed');
            });
        } catch (e) {}
    }

    /// 根据 LLM 校对开关状态显示/隐藏配置区
    function updateLlmPostConfigVisibility() {
        const enable = $('#setting-llm-post-enable');
        const config = $('#llm-post-config');
        if (!enable || !config) return;
        config.style.display = enable.checked ? 'block' : 'none';
    }

    /// 初始化 LLM 智能校对开关联动
    function initLlmPostToggle() {
        const enable = $('#setting-llm-post-enable');
        if (!enable || enable.dataset.bound) return;
        enable.dataset.bound = '1';
        enable.addEventListener('change', updateLlmPostConfigVisibility);
    }

    /// 设置页脏状态跟踪：用户改动任何设置控件时标记为未保存
    function initSettingsDirtyTracking() {
        const handler = () => {
            if (!state.populatingSettings) {
                state.settingsDirty = true;
            }
        };
        // 设置页与字幕页都包含可保存的设置控件
        ['#view-settings', '#view-subtitle'].forEach(viewId => {
            const view = $(viewId);
            if (view) {
                view.addEventListener('change', handler);
                view.addEventListener('input', handler);
            }
        });
    }

    function initAudioQualityInteractions() {
        const audioProfile = $('#setting-audio-profile');
        const audioDownmix = $('#setting-audio-downmix');
        const audioSampleFmt = $('#setting-audio-sample-format');
        const audioSampleRate = $('#setting-audio-sample-rate');
        const audioChannels = $('#setting-audio-channels');
        if (!audioProfile) return;

        const presetVals = {
            standard:  { audio_downmix: 'strongest', audio_sample_format: 'auto',  audio_sample_rate: 'auto',  audio_channels: 'auto' },
            array_mic: { audio_downmix: 'strongest', audio_sample_format: 'f32',   audio_sample_rate: '48000', audio_channels: 'auto' },
        };

        // 选择预设后：如果非 custom，自动同步其余参数
        audioProfile.addEventListener('change', () => {
            if (state.populatingSettings) return;
            const prof = audioProfile.value;
            const presets = presetVals[prof];
            if (!presets) return; // custom 时不干涉
            const before = state.populatingSettings;
            state.populatingSettings = true;
            try {
                if (audioDownmix) audioDownmix.value = presets.audio_downmix;
                if (audioSampleFmt) audioSampleFmt.value = presets.audio_sample_format;
                if (audioSampleRate) audioSampleRate.value = presets.audio_sample_rate;
                if (audioChannels) audioChannels.value = presets.audio_channels;
            } finally {
                state.populatingSettings = before;
                state.settingsDirty = true;
            }
        });

        // 用户手动改动任何细项 → 自动切到自定义 Profile，避免视觉/逻辑不一致
        const manualControls = [audioDownmix, audioSampleFmt, audioSampleRate, audioChannels];
        manualControls.forEach(el => {
            if (!el) return;
            el.addEventListener('change', () => {
                if (state.populatingSettings) return;
                if (audioProfile.value !== 'custom') {
                    state.populatingSettings = true;
                    try {
                        audioProfile.value = 'custom';
                    } finally {
                        state.populatingSettings = false;
                    }
                }
                state.settingsDirty = true;
            });
        });
    }

    /// 初始化主题切换器
    function initThemeSwitcher() {
        const btns = $$('.theme-selector .seg-btn');
        btns.forEach(btn => {
            if (btn.dataset.bound) return;
            btn.dataset.bound = '1';
            btn.addEventListener('click', (e) => {
                e.stopPropagation();
                const theme = btn.dataset.theme;
                btns.forEach(b => b.classList.toggle('active', b === btn));
                applyTheme(theme, true);
            });
        });
    }

    function updateMicHint() {
        const micHint = $('.mic-hint');
        if (!micHint) return;
        if (state.isRecording) {
            micHint.textContent = state.triggerMode === 'hold' ? '松开停止录音' : '点击停止录音';
        } else {
            micHint.textContent = state.triggerMode === 'hold' ? '按住开始录音' : '点击开始录音';
        }
    }

    function updateHotkeyHint() {
        const hint = $('.hotkey-hint');
        if (!hint) return;
        const modeLabel = state.dictationMode === 'stream' ? '流式' : '整段';
        // 从配置或设置输入框读取实际快捷键，避免硬编码 F2
        let keyName = 'F2';
        if (state.config && state.config.basic && state.config.basic.hotkey) {
            const name = virtualKeyToName(state.config.basic.hotkey);
            if (name) keyName = name;
        }
        const hotkeyInput = $('#setting-hotkey-batch');
        if (hotkeyInput && hotkeyInput.value && hotkeyInput.dataset.listening !== 'true') {
            keyName = hotkeyInput.value;
        }
        hint.textContent = `快捷键: ${keyName} (${modeLabel})`;
    }

    function updateModelBadge() {
        const badge = $('#current-model-badge');
        if (!badge || !state.config) return;

        let modelName = 'SenseVoiceSmall';
        if (state.config.model_selection && state.config.model_selection.batch_model) {
            const m = state.config.model_selection.batch_model;
            if (m.includes('SenseVoice')) modelName = 'SenseVoiceSmall';
            else if (m.includes('TeleSpeech')) modelName = 'TeleSpeech';
            else if (m.includes('whisper')) modelName = 'Whisper';
            else if (m.includes('local')) modelName = '本地 Whisper';
        }
        badge.textContent = modelName;
    }

    function collectSettings() {
        if (!state.config) return null;

        const newConfig = JSON.parse(JSON.stringify(state.config));

        const batchSel = $('#setting-batch-model');
        const streamSel = $('#setting-stream-model');
        const subSel = $('#setting-subtitle-model');
        if (batchSel) newConfig.model_selection.batch_model = batchSel.value;
        if (streamSel) newConfig.model_selection.stream_model = streamSel.value;
        if (subSel) newConfig.model_selection.subtitle_model = subSel.value;

        const sfKey = $('#setting-sf-key');
        const groqKey = $('#setting-groq-key');
        const doubaoKey = $('#setting-doubao-key');
        if (sfKey) newConfig.model.siliconflow_api_key = sfKey.value;
        if (groqKey) newConfig.model.groq_api_key = groqKey.value;
        if (doubaoKey) newConfig.model.doubao_api_key = doubaoKey.value;

        // 自定义模型提供商配置
        const customUrl = $('#setting-custom-api-url');
        const customModel = $('#setting-custom-model-name');
        const customKey = $('#setting-custom-key');
        if (customUrl) newConfig.model.custom_api_url = customUrl.value;
        if (customModel) newConfig.model.custom_model_name = customModel.value;
        if (customKey) newConfig.model.custom_api_key = customKey.value;

        // 本地 Whisper 性能调优字段（0=自动线程；贪婪/关闭回退默认关）
        const wThreads = $('#setting-whisper-threads');
        const wGreedy = $('#setting-whisper-greedy');
        const wNoFallback = $('#setting-whisper-no-fallback');
        if (wThreads) {
            const t = parseInt(wThreads.value, 10);
            newConfig.model.local_whisper_threads = (Number.isFinite(t) && t >= 0 && t <= 8) ? t : 0;
        }
        if (wGreedy) newConfig.model.local_whisper_greedy = wGreedy.checked;
        if (wNoFallback) newConfig.model.local_whisper_no_fallback = wNoFallback.checked;

        const outputMode = $('#setting-output-mode');
        const outputLang = $('#setting-output-lang');
        if (outputMode) newConfig.basic.output_mode = outputMode.value;
        if (outputLang) newConfig.basic.output_language = outputLang.value;

        // 语音输入设备（整段+流式共用）
        const inputDeviceSel = $('#setting-input-device');
        if (inputDeviceSel) newConfig.basic.input_device = inputDeviceSel.value;

        // 音频采集质量偏好
        const audioProfile = $('#setting-audio-profile');
        const audioDownmix = $('#setting-audio-downmix');
        const audioSampleFmt = $('#setting-audio-sample-format');
        const audioSampleRate = $('#setting-audio-sample-rate');
        const audioChannels = $('#setting-audio-channels');
        if (audioProfile) newConfig.basic.audio_profile = audioProfile.value;
        if (audioDownmix) newConfig.basic.audio_downmix = audioDownmix.value;
        if (audioSampleFmt) newConfig.basic.audio_sample_format = audioSampleFmt.value;
        if (audioSampleRate) newConfig.basic.audio_sample_rate = audioSampleRate.value;
        if (audioChannels) newConfig.basic.audio_channels = audioChannels.value;

        // 快捷键：将按键名称转换为虚拟键码后保存
        // 后端 rdev 监听器在每次按键时都会读取 config.basic.hotkey，所以保存后立即生效
        const hotkeyInput = $('#setting-hotkey-batch');
        if (hotkeyInput) {
            const vk = nameToVirtualKey(hotkeyInput.value);
            if (vk) {
                newConfig.basic.hotkey = vk;
                // 同步更新 hotkey 提示文本
                const hint = $('.hotkey-hint');
                if (hint) {
                    const modeLabel = state.dictationMode === 'stream' ? '流式' : '整段';
                    hint.textContent = `快捷键: ${hotkeyInput.value} (${modeLabel})`;
                }
            }
        }

        const punctuation = $('#setting-punctuation');
        const emoji = $('#setting-emoji');
        const indicator = $('#setting-indicator');
        const postProcessor = $('#setting-post-processor');
        if (punctuation) newConfig.features.allow_punctuation = punctuation.checked;
        if (emoji) newConfig.features.allow_emoji = emoji.checked;
        if (indicator) newConfig.features.enable_indicator = indicator.checked;
        if (postProcessor) newConfig.features.enable_post_processor = postProcessor.checked;

        const llmEnable = $('#setting-llm-post-enable');
        const llmUrl = $('#setting-llm-api-url');
        const llmKey = $('#setting-llm-api-key');
        const llmModel = $('#setting-llm-model');
        const llmPrompt = $('#setting-llm-system-prompt');
        if (!newConfig.llm_post) newConfig.llm_post = {};
        if (llmEnable) newConfig.llm_post.enable = llmEnable.checked;
        if (llmUrl) newConfig.llm_post.api_url = llmUrl.value.trim();
        if (llmKey) newConfig.llm_post.api_key = llmKey.value.trim();
        if (llmModel) newConfig.llm_post.model = llmModel.value.trim();
        if (llmPrompt) newConfig.llm_post.system_prompt = llmPrompt.value;

        newConfig.advanced.trigger_mode = state.triggerMode;
        newConfig.basic.dictation_mode = state.dictationMode;

        const sensSlider = $('#setting-vad-sensitivity');
        const silenceSlider = $('#setting-vad-silence');
        if (sensSlider) newConfig.vad.vad_sensitivity = parseInt(sensSlider.value) / 100;
        if (silenceSlider) newConfig.vad.vad_silence_duration_ms = parseInt(silenceSlider.value);

        // 字幕配置：场景化收集（当前 UI 场景 + 其它场景草稿全部写入）
        const scene = collectSceneFromUI();
        if (scene) {
            if (!newConfig.subtitle) newConfig.subtitle = {};
            const idx = state.scenes.findIndex(sc => sc.id === scene.id);
            if (idx >= 0) state.scenes[idx] = scene;
            else state.scenes.push(scene);
            newConfig.subtitle.subtitle_scenes = state.scenes.map(sc => JSON.parse(JSON.stringify(sc)));
        }

        // 字幕识别音源（会话级，独立于语音输入设备）
        const subDeviceSel = $('#setting-subtitle-input-device');
        if (subDeviceSel) newConfig.subtitle.subtitle_input_device = subDeviceSel.value;

        // 音源类型（麦克风 / 系统扬声器）
        const sourceActiveBtn = $('#subtitle-audio-source .seg-btn.active');
        newConfig.subtitle.subtitle_audio_source = sourceActiveBtn
            ? sourceActiveBtn.dataset.source
            : 'microphone';

        // 同声传译 LLM 共享接口配置
        const transLlmUrl = $('#subtitle-translation-llm-url');
        const transLlmKey = $('#subtitle-translation-llm-key');
        const transLlmModel = $('#subtitle-translation-llm-model');
        if (transLlmUrl) newConfig.subtitle.subtitle_translation_llm_api_url = transLlmUrl.value.trim();
        if (transLlmKey) newConfig.subtitle.subtitle_translation_llm_api_key = transLlmKey.value.trim();
        if (transLlmModel) newConfig.subtitle.subtitle_translation_llm_model = transLlmModel.value.trim();

        // 字幕开关热键
        const subHotkeyInput = $('#subtitle-hotkey');
        if (subHotkeyInput) {
            const vk = nameToVirtualKey(subHotkeyInput.value);
            if (vk) newConfig.subtitle.subtitle_hotkey = vk;
        }

        // 主题
        const activeThemeBtn = $('.theme-selector .seg-btn.active');
        if (activeThemeBtn) {
            newConfig.theme = activeThemeBtn.dataset.theme;
        }

        newConfig.basic.model_name = newConfig.model_selection.batch_model;

        return newConfig;
    }

    async function saveSettings() {
        const newConfig = collectSettings();
        if (!newConfig) return;

        if (!invoke) {
            console.log('Would save config:', newConfig);
            state.config = newConfig;
            state.settingsDirty = false;
            return;
        }

        try {
            await invoke('save_config', { newConfig: newConfig });
            state.config = newConfig;
            state.settingsDirty = false;
            updateModelBadge();
            updateMicHint();
            // 如果字幕窗口正在显示，热推送新配置让样式立即生效
            try {
                await invoke('push_subtitle_config');
            } catch (e) {
                console.warn('Failed to push subtitle config:', e);
            }
            setStatus('ready', '设置已保存');
            setTimeout(() => setStatus('idle', '就绪'), 2000);
        } catch (err) {
            console.error('Failed to save config:', err);
            setStatus('error', '保存失败');
            setTimeout(() => setStatus('idle', '就绪'), 2000);
        }
    }

    async function loadHistory() {
        if (!invoke) return;

        const historyList = $('#history-list');
        if (!historyList) return;

        try {
            const history = await invoke('get_history');
            if (!history || history.length === 0) {
                historyList.innerHTML = `
                    <div class="empty-state">
                        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.3"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>
                        <p>暂无历史记录</p>
                    </div>
                `;
                return;
            }

            historyList.innerHTML = history.map((text, i) => `
                <div class="history-item" data-index="${i}" data-text="${escapeHtml(text)}">
                    <span class="history-item-index">${String(i + 1).padStart(2, '0')}</span>
                    <span class="history-item-text">${escapeHtml(text)}</span>
                </div>
            `).join('');

            // 右键菜单
            historyList.querySelectorAll('.history-item').forEach(item => {
                item.addEventListener('contextmenu', (e) => {
                    e.preventDefault();
                    const text = item.dataset.text;
                    const idx = parseInt(item.dataset.index);
                    showContextMenu(e.clientX, e.clientY, [
                        {
                            label: '复制',
                            onClick: () => copyToClipboard(text)
                        },
                        { divider: true },
                        {
                            label: '删除此项',
                            danger: true,
                            onClick: async () => {
                                if (!invoke) return;
                                try {
                                    await invoke('remove_history', { index: idx });
                                    loadHistory();
                                } catch (err) {
                                    console.error('Failed to remove history:', err);
                                }
                            }
                        }
                    ]);
                });
            });
        } catch (err) {
            console.error('Failed to load history:', err);
        }
    }

    // 全局右键菜单
    function showContextMenu(x, y, items) {
        const menu = $('#context-menu');
        if (!menu) return;
        menu.innerHTML = items.map((it, i) => {
            if (it.divider) return `<div class="context-menu-divider" data-i="${i}"></div>`;
            return `<div class="context-menu-item${it.danger ? ' danger' : ''}" data-i="${i}">${escapeHtml(it.label)}</div>`;
        }).join('');
        menu.hidden = false;
        // 防止溢出窗口
        const rect = menu.getBoundingClientRect();
        const winW = window.innerWidth;
        const winH = window.innerHeight;
        const px = x + rect.width > winW ? winW - rect.width - 4 : x;
        const py = y + rect.height > winH ? winH - rect.height - 4 : y;
        menu.style.left = px + 'px';
        menu.style.top = py + 'px';

        const close = () => {
            menu.hidden = true;
            document.removeEventListener('click', onDocClick);
            document.removeEventListener('contextmenu', onDocContext);
        };
        const onDocClick = () => close();
        const onDocContext = () => close();
        menu.querySelectorAll('.context-menu-item').forEach(el => {
            el.addEventListener('click', () => {
                const i = parseInt(el.dataset.i);
                const item = items[i];
                close();
                if (item && typeof item.onClick === 'function') {
                    item.onClick();
                }
            });
        });
        setTimeout(() => {
            document.addEventListener('click', onDocClick);
            document.addEventListener('contextmenu', onDocContext);
        }, 0);
    }

    async function copyToClipboard(text) {
        if (!invoke) return;
        try {
            await invoke('copy_to_clipboard', { text });
        } catch (err) {
            console.error('Failed to copy:', err);
        }
    }

    function escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }

    /// 转义 HTML 属性值（额外转义引号，防止属性注入型 XSS）
    function escapeAttr(text) {
        return String(text == null ? '' : text)
            .replace(/&/g, '&amp;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#39;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;');
    }

    function initWindowControls() {
        const closeBtn = $('#btn-close');
        const minimizeBtn = $('#btn-minimize');

        if (closeBtn) {
            closeBtn.addEventListener('click', async () => {
                // 关闭窗口时若有未保存设置则提示
                if ((state.currentView === 'settings' || state.currentView === 'subtitle') && state.settingsDirty) {
                    const result = await showUnsavedChangesDialog();
                    if (result === 'cancel') return;
                    if (result === 'save') {
                        await saveSettings();
                    }
                    state.settingsDirty = false;
                }
                if (getCurrentWindow) {
                    try {
                        await getCurrentWindow().close();
                    } catch (err) {
                        console.log('Window close:', err);
                    }
                } else {
                    console.log('Window close');
                    window.close();
                }
            });
        }

        if (minimizeBtn) {
            minimizeBtn.addEventListener('click', async () => {
                if (getCurrentWindow) {
                    try {
                        await getCurrentWindow().minimize();
                    } catch (err) {
                        console.log('Window minimize:', err);
                    }
                } else {
                    console.log('Window minimize');
                }
            });
        }
    }

    function initNavigation() {
        $$('.nav-item').forEach(item => {
            item.addEventListener('click', async () => {
                const view = item.dataset.view;
                if (view) {
                    await switchView(view);
                }
            });
        });

        const sidebarToggle = $('#sidebar-toggle');
        const sidebar = $('#sidebar');
        if (sidebarToggle && sidebar) {
            const savedState = localStorage.getItem('sidebar-collapsed');
            if (savedState === 'true') {
                sidebar.classList.remove('expanded');
                sidebar.classList.add('collapsed');
            }
            sidebarToggle.addEventListener('click', () => {
                const isCollapsed = sidebar.classList.contains('collapsed');
                if (isCollapsed) {
                    sidebar.classList.remove('collapsed');
                    sidebar.classList.add('expanded');
                    localStorage.setItem('sidebar-collapsed', 'false');
                } else {
                    sidebar.classList.remove('expanded');
                    sidebar.classList.add('collapsed');
                    localStorage.setItem('sidebar-collapsed', 'true');
                }
                // 侧边栏宽度过渡期间持续刷新指示器位置，保证平滑跟随
                requestAnimationFrame(refreshAllIndicators);
                setTimeout(refreshAllIndicators, 180);
                setTimeout(refreshAllIndicators, 320);
            });
        }
    }

    function initDictation() {
        const micBtn = $('#mic-btn');
        if (micBtn) {
            // 仅左键 (button === 0) 触发录音；右键、中键忽略
            micBtn.addEventListener('mousedown', (e) => {
                if (e.button !== 0) return;
                if (state.triggerMode === 'hold' && !state.isRecording) {
                    state.isMouseDown = true;
                    startRecording();
                }
            });

            micBtn.addEventListener('mouseup', (e) => {
                if (e.button !== 0) return;
                if (state.triggerMode === 'hold' && state.isRecording) {
                    state.isMouseDown = false;
                    stopRecording();
                }
            });

            micBtn.addEventListener('mouseleave', () => {
                if (state.triggerMode === 'hold' && state.isMouseDown && state.isRecording) {
                    state.isMouseDown = false;
                    stopRecording();
                }
            });

            micBtn.addEventListener('contextmenu', (e) => {
                // 阻止右键菜单弹出，避免与录音逻辑冲突
                e.preventDefault();
            });

            micBtn.addEventListener('click', (e) => {
                e.preventDefault();
                if (e.button !== 0) return;
                if (state.triggerMode === 'toggle') {
                    if (state.isRecording) {
                        stopRecording();
                    } else {
                        startRecording();
                    }
                }
            });
        }

        $$('#trigger-mode .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('#trigger-mode .seg-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                state.triggerMode = btn.dataset.mode;
                updateMicHint();
                moveSegIndicator(btn);
            });
        });

        $$('#dictation-mode .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('#dictation-mode .seg-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                state.dictationMode = btn.dataset.mode;
                updateHotkeyHint();
                moveSegIndicator(btn);
                // 立即保存到后端，便于 F2 切换后即时生效
                if (invoke && state.config) {
                    const cfg = JSON.parse(JSON.stringify(state.config));
                    cfg.basic.dictation_mode = state.dictationMode;
                    invoke('save_config', { newConfig: cfg }).then(() => {
                        state.config = cfg;
                    }).catch(err => console.error('Failed to save dictation mode:', err));
                }
            });
        });

        // 启动时根据 state.dictationMode 同步按钮选中状态
        const activeDictBtn = $(`#dictation-mode .seg-btn[data-mode="${state.dictationMode}"]`);
        $$('#dictation-mode .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === activeDictBtn);
        });
        if (activeDictBtn) moveSegIndicator(activeDictBtn);
    }

    function initSubtitle() {
        const toggleBtn = $('#toggle-subtitle-btn');
        if (toggleBtn) {
            toggleBtn.addEventListener('click', toggleSubtitle);
        }

        // 所有可触发预览更新的控件
        const previewIds = [
            'subtitle-font-family', 'subtitle-font-size', 'subtitle-font-weight',
            'subtitle-font-color', 'subtitle-letter-spacing', 'subtitle-line-height',
            'subtitle-text-shadow-color', 'subtitle-text-shadow-strength',
            'subtitle-bg-color', 'subtitle-opacity', 'subtitle-blur',
            'subtitle-padding-x', 'subtitle-padding-y', 'subtitle-lines',
            'subtitle-box-max-width',
            'subtitle-interim-color', 'subtitle-interim-opacity'
        ];
        previewIds.forEach(id => {
            const el = $(`#${id}`);
            if (el) {
                el.addEventListener('input', updateSubtitlePreview);
                el.addEventListener('change', updateSubtitlePreview);
            }
        });

        // 预览文本点击切换 最终/临时(interim) 状态，方便预览 interim 样式
        const previewText = $('#subtitle-preview-text');
        if (previewText) {
            previewText.addEventListener('click', () => {
                state.previewInterim = !state.previewInterim;
                updateSubtitlePreview();
            });
            previewText.title = '点击切换 最终/临时 状态预览';
        }

        // 开关：加粗 / 斜体
        ['subtitle-bold', 'subtitle-italic'].forEach(id => {
            const sw = $(`#${id}`);
            if (sw) {
                sw.addEventListener('click', () => {
                    sw.dataset.on = sw.dataset.on === 'true' ? 'false' : 'true';
                    // bold 开关与 font_weight 同步：后端无单独 bold 字段，通过 font_weight>=700 体现
                    if (id === 'subtitle-bold') {
                        const weightSelect = $('#subtitle-font-weight');
                        if (weightSelect) {
                            if (sw.dataset.on === 'true') {
                                weightSelect.value = '700';
                            } else if (parseInt(weightSelect.value) >= 700) {
                                weightSelect.value = '400';
                            }
                        }
                    }
                    updateSubtitlePreview();
                });
            }
        });

        // 对齐方式 segmented control
        $$('#subtitle-text-align .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('#subtitle-text-align .seg-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                state.subtitleAlign = btn.dataset.mode;
                updateSubtitlePreview();
                moveSegIndicator(btn);
            });
        });

        // 元素显示开关：原文/译文/说话人/时间戳
        ['subtitle-show-original', 'subtitle-show-translation', 'subtitle-show-speaker', 'subtitle-show-timestamp'].forEach(id => {
            const sw = $(`#${id}`);
            if (sw) {
                sw.addEventListener('click', () => {
                    sw.dataset.on = sw.dataset.on === 'true' ? 'false' : 'true';
                    markPresetCustom();
                    updateSubtitlePreview();
                });
            }
        });

        // 布局方向 segmented control
        $$('#subtitle-layout .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('#subtitle-layout .seg-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                state.subtitleLayout = btn.dataset.mode;
                markPresetCustom();
                updateSubtitlePreview();
                moveSegIndicator(btn);
            });
        });

        // 容器水平对齐锚点 segmented control
        $$('#subtitle-container-align-x .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('#subtitle-container-align-x .seg-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                state.subtitleContainerAlignX = btn.dataset.mode;
                updateSubtitlePreview();
                moveSegIndicator(btn);
            });
        });

        // 容器垂直对齐锚点 segmented control
        $$('#subtitle-container-align-y .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('#subtitle-container-align-y .seg-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                state.subtitleContainerAlignY = btn.dataset.mode;
                updateSubtitlePreview();
                moveSegIndicator(btn);
            });
        });

        // 音源类型 segmented control（麦克风 / 系统扬声器）
        $$('#subtitle-audio-source .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('#subtitle-audio-source .seg-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                updateSubtitleSourceUI(btn.dataset.source);
                moveSegIndicator(btn);
            });
        });

        // 预设模板 segmented control
        $$('#subtitle-preset .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                applySubtitlePreset(btn.dataset.preset);
                moveSegIndicator(btn);
            });
        });

        // 自定义元素：添加按钮
        [
            { id: 'btn-add-custom-text', type: 'text' },
            { id: 'btn-add-custom-divider', type: 'divider' },
            { id: 'btn-add-custom-spacer', type: 'spacer' },
        ].forEach(({ id, type }) => {
            const btn = $(`#${id}`);
            if (btn) {
                btn.addEventListener('click', () => addCustomElement(type));
            }
        });

        // 应用到字幕窗口按钮：保存设置并推送配置
        const applyBtn = $('#btn-apply-subtitle');
        if (applyBtn) {
            applyBtn.addEventListener('click', async () => {
                if (!invoke) return;
                applyBtn.disabled = true;
                applyBtn.textContent = '应用中...';
                try {
                    const newConfig = collectSettings();
                    if (newConfig) {
                        await invoke('save_config', { newConfig: newConfig });
                        state.config = newConfig;
                        state.settingsDirty = false;
                    }
                    await invoke('push_subtitle_config');
                    setStatus('ready', '字幕配置已应用');
                    setTimeout(() => setStatus('idle', '就绪'), 2000);
                } catch (err) {
                    console.error('Failed to apply subtitle config:', err);
                    setStatus('error', '应用失败');
                    setTimeout(() => setStatus('idle', '就绪'), 2000);
                } finally {
                    applyBtn.disabled = false;
                    applyBtn.textContent = '应用到字幕窗口';
                }
            });
        }

        // 字幕窗口控制开关（按当前场景生效）
        const alwaysOnTopSw = $('#subtitle-always-on-top');
        if (alwaysOnTopSw) {
            alwaysOnTopSw.addEventListener('click', async () => {
                const on = alwaysOnTopSw.dataset.on === 'true';
                alwaysOnTopSw.dataset.on = on ? 'false' : 'true';
                const sceneObj = getCurrentScene();
                if (sceneObj && sceneObj.window) sceneObj.window.always_on_top = !on;
                if (invoke) {
                    try {
                        await invoke('set_subtitle_always_on_top', { sceneId: state.currentSceneId, onTop: !on });
                    } catch (err) {
                        console.error('Failed to set always on top:', err);
                    }
                }
            });
        }

        const clickThroughSw = $('#subtitle-click-through');
        if (clickThroughSw) {
            clickThroughSw.addEventListener('click', async () => {
                const on = clickThroughSw.dataset.on === 'true';
                clickThroughSw.dataset.on = on ? 'false' : 'true';
                const sceneObj = getCurrentScene();
                if (sceneObj && sceneObj.window) sceneObj.window.click_through = !on;
                if (invoke) {
                    try {
                        await invoke('set_subtitle_click_through', { sceneId: state.currentSceneId, clickThrough: !on });
                    } catch (err) {
                        console.error('Failed to set click through:', err);
                    }
                }
            });
        }

        const obsModeSw = $('#subtitle-obs-mode');
        if (obsModeSw) {
            obsModeSw.addEventListener('click', async () => {
                const on = obsModeSw.dataset.on === 'true';
                obsModeSw.dataset.on = on ? 'false' : 'true';
                const sceneObj = getCurrentScene();
                if (sceneObj && sceneObj.window) sceneObj.window.obs_mode = !on;
                if (invoke) {
                    try {
                        await invoke('set_subtitle_obs_mode', { sceneId: state.currentSceneId, obsMode: !on });
                    } catch (err) {
                        console.error('Failed to set OBS mode:', err);
                    }
                }
            });
        }

        const autoFitSw = $('#subtitle-auto-fit');
        if (autoFitSw) {
            autoFitSw.addEventListener('click', () => {
                autoFitSw.dataset.on = autoFitSw.dataset.on === 'true' ? 'false' : 'true';
                const sceneObj = getCurrentScene();
                if (sceneObj && sceneObj.window) sceneObj.window.auto_fit = autoFitSw.dataset.on === 'true';
            });
        }

        // 显示/隐藏字幕窗口（当前场景）
        const showBtn = $('#btn-show-subtitle-window');
        if (showBtn) {
            showBtn.addEventListener('click', async () => {
                if (!invoke) return;
                try {
                    await invoke('show_subtitle_window', { sceneId: state.currentSceneId, show: true });
                    await invoke('push_subtitle_config');
                    // 显示窗口会重新启用被关闭停用的场景
                    const sceneObj = getCurrentScene();
                    if (sceneObj) sceneObj.enabled = true;
                    renderSceneList();
                } catch (err) {
                    console.error('Failed to show subtitle window:', err);
                }
            });
        }
        const hideBtn = $('#btn-hide-subtitle-window');
        if (hideBtn) {
            hideBtn.addEventListener('click', async () => {
                if (!invoke) return;
                try {
                    await invoke('show_subtitle_window', { sceneId: state.currentSceneId, show: false });
                } catch (err) {
                    console.error('Failed to hide subtitle window:', err);
                }
            });
        }

        // ===== 场景管理 =====
        const sceneSelect = $('#subtitle-scene-select');
        if (sceneSelect) {
            sceneSelect.addEventListener('change', () => switchSubtitleScene(sceneSelect.value));
        }

        const sceneNameInput = $('#subtitle-scene-name');
        if (sceneNameInput) {
            sceneNameInput.addEventListener('input', () => {
                const sceneObj = getCurrentScene();
                if (sceneObj) sceneObj.name = sceneNameInput.value.trim() || sceneObj.name;
            });
        }

        const addSceneBtn = $('#btn-add-subtitle-scene');
        if (addSceneBtn) addSceneBtn.addEventListener('click', addSubtitleScene);

        const dupSceneBtn = $('#btn-duplicate-subtitle-scene');
        if (dupSceneBtn) dupSceneBtn.addEventListener('click', duplicateSubtitleScene);

        const rmSceneBtn = $('#btn-remove-subtitle-scene');
        if (rmSceneBtn) rmSceneBtn.addEventListener('click', removeSubtitleScene);

        // ===== 同声传译设置 =====
        const transEngineSel = $('#subtitle-translation-engine');
        if (transEngineSel) {
            transEngineSel.addEventListener('change', () => {
                updateTranslationUI();
                updateSubtitlePreview();
            });
        }
        const transLangSel = $('#subtitle-translation-lang');
        if (transLangSel) transLangSel.addEventListener('change', () => updateSubtitlePreview());
        const transInterimSw = $('#subtitle-translation-interim');
        if (transInterimSw) {
            transInterimSw.addEventListener('click', () => {
                transInterimSw.dataset.on = transInterimSw.dataset.on === 'true' ? 'false' : 'true';
            });
        }

        // ===== 实时转录面板 =====
        const exportTxtBtn = $('#btn-export-transcript-txt');
        if (exportTxtBtn) exportTxtBtn.addEventListener('click', () => exportTranscript('txt'));
        const exportSrtBtn = $('#btn-export-transcript-srt');
        if (exportSrtBtn) exportSrtBtn.addEventListener('click', () => exportTranscript('srt'));
        const exportMdBtn = $('#btn-export-transcript-md');
        if (exportMdBtn) exportMdBtn.addEventListener('click', () => exportTranscript('md'));
        const clearTrBtn = $('#btn-clear-transcript');
        if (clearTrBtn) clearTrBtn.addEventListener('click', clearTranscript);
    }

    function initHistory() {
        const clearBtn = $('#btn-clear-history');
        if (clearBtn) {
            clearBtn.addEventListener('click', async () => {
                if (!invoke) return;
                try {
                    await invoke('clear_history');
                    loadHistory();
                } catch (err) {
                    console.error('Failed to clear history:', err);
                }
            });
        }
    }

    function initSettingsSliders() {
        const sensSlider = $('#setting-vad-sensitivity');
        const silenceSlider = $('#setting-vad-silence');

        if (sensSlider) {
            sensSlider.addEventListener('input', () => {
                if (sensSlider.nextElementSibling) {
                    sensSlider.nextElementSibling.textContent = `${sensSlider.value}%`;
                }
            });
        }
        if (silenceSlider) {
            silenceSlider.addEventListener('input', () => {
                if (silenceSlider.nextElementSibling) {
                    silenceSlider.nextElementSibling.textContent = `${silenceSlider.value}ms`;
                }
            });
        }

        const saveBtn = $('#btn-save-settings');
        if (saveBtn) {
            saveBtn.addEventListener('click', saveSettings);
        }

        // 本地模型帮助按钮
        const helpBtn = $('#btn-local-model-help');
        if (helpBtn) {
            helpBtn.addEventListener('click', showHelpModal);
        }
        const closeHelpBtn = $('#btn-close-help');
        if (closeHelpBtn) {
            closeHelpBtn.addEventListener('click', closeHelpModal);
        }
        // 点击模态框遮罩关闭
        const helpModal = $('#help-modal');
        if (helpModal) {
            helpModal.addEventListener('click', (e) => {
                if (e.target === helpModal) closeHelpModal();
            });
        }
        // 模态框内的链接按钮也通过 shell.open 打开
        $$('.help-link').forEach(link => {
            link.addEventListener('click', async (e) => {
                e.preventDefault();
                const url = link.dataset.url;
                if (!url) return;
                try {
                    if (window.__TAURI__ && window.__TAURI__.shell && window.__TAURI__.shell.open) {
                        await window.__TAURI__.shell.open(url);
                    } else {
                        window.open(url, '_blank');
                    }
                } catch (err) {
                    window.open(url, '_blank');
                }
            });
        });

        // 刷新已下载模型列表
        const refreshBtn = $('#btn-refresh-models');
        if (refreshBtn) {
            refreshBtn.addEventListener('click', async () => {
                refreshBtn.classList.add('spinning');
                await loadModelStatus();
                setTimeout(() => refreshBtn.classList.remove('spinning'), 600);
            });
        }

        // 打开模型目录
        const openDirBtn = $('#btn-open-models-dir');
        if (openDirBtn) openDirBtn.addEventListener('click', openModelsDirectory);

        // 更改模型目录
        const changeDirBtn = $('#btn-change-models-dir');
        if (changeDirBtn) {
            changeDirBtn.addEventListener('click', changeModelsDirectory);
        }

        // 恢复默认模型目录
        const resetDirBtn = $('#btn-reset-models-dir');
        if (resetDirBtn) {
            resetDirBtn.addEventListener('click', resetModelsDirectory);
        }

        // 检测引擎状态
        const checkEngineBtn = $('#btn-check-engine');
        if (checkEngineBtn) {
            checkEngineBtn.addEventListener('click', () => checkEngineStatus());
        }

        // 下载/重新下载 whisper.cpp 引擎
        const dlEngineBtn = $('#btn-download-engine');
        if (dlEngineBtn) {
            dlEngineBtn.addEventListener('click', async () => {
                if (!invoke) return;
                dlEngineBtn.disabled = true;
                dlEngineBtn.textContent = '下载中...';

                // 监听下载进度
                let unlisten = null;
                if (listen) {
                    unlisten = await listen('binary-download-progress', (event) => {
                        const p = event.payload;
                        if (p && typeof p.progress !== 'undefined') {
                            dlEngineBtn.textContent = `下载中 ${p.progress}%`;
                        }
                    });
                }

                try {
                    const result = await invoke('download_whisper_binary', { force: true });
                    console.log('引擎下载成功:', result);
                    await checkEngineStatus();
                } catch (err) {
                    console.error('引擎下载失败:', err);
                    alert('引擎下载失败: ' + err);
                    await checkEngineStatus();
                } finally {
                    if (unlisten) unlisten();
                    dlEngineBtn.disabled = false;
                }
            });
        }
    }

    /// 打开文件夹选择器，让用户选择新的模型下载目录
    async function changeModelsDirectory() {
        if (!invoke) return;

        const btn = $('#btn-change-models-dir');
        const originalText = btn ? btn.textContent : '';
        if (btn) {
            btn.disabled = true;
            btn.textContent = '选择中...';
        }

        try {
            const result = await invoke('pick_models_directory');
            if (result) {
                // 用户选择了新目录
                const dirEl = $('#models-dir-path');
                if (dirEl) dirEl.textContent = result;
                // 重新加载（新目录可能已有模型）
                await loadModelStatus();
                addLog('info', `模型目录已更改为: ${result}`, 'settings');
            }
        } catch (err) {
            console.error('Failed to pick models directory:', err);
            addLog('error', `更改模型目录失败: ${err}`, 'settings');
        } finally {
            if (btn) {
                btn.disabled = false;
                btn.textContent = originalText;
            }
        }
    }

    /// 恢复默认模型目录
    async function resetModelsDirectory() {
        if (!invoke) return;

        const btn = $('#btn-reset-models-dir');
        const originalText = btn ? btn.textContent : '';
        if (btn) {
            btn.disabled = true;
            btn.textContent = '恢复中...';
        }

        try {
            const newDir = await invoke('reset_models_directory');
            const dirEl = $('#models-dir-path');
            if (dirEl) dirEl.textContent = newDir;
            // 重新加载
            await loadModelStatus();
            addLog('info', `模型目录已恢复为默认: ${newDir}`, 'settings');
        } catch (err) {
            console.error('Failed to reset models directory:', err);
            addLog('error', `恢复默认模型目录失败: ${err}`, 'settings');
        } finally {
            if (btn) {
                btn.disabled = false;
                btn.textContent = originalText;
            }
        }
    }

    function initApiKeyToggles() {
        $$('.toggle-eye-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                const targetId = btn.dataset.target;
                const input = document.getElementById(targetId);
                if (!input) return;
                if (input.type === 'password') {
                    input.type = 'text';
                } else {
                    input.type = 'password';
                }
            });
        });

        // 申请链接按钮：点击通过 shell 插件打开系统浏览器
        $$('.apply-link-btn, .mm-link-btn[data-url]').forEach(btn => {
            btn.addEventListener('click', async () => {
                const url = btn.dataset.url;
                if (!url) return;

                try {
                    if (window.__TAURI__ && window.__TAURI__.shell && window.__TAURI__.shell.open) {
                        await window.__TAURI__.shell.open(url);
                    } else {
                        window.open(url, '_blank');
                    }
                } catch (e) {
                    console.error('打开链接失败:', e);
                    window.open(url, '_blank');
                }
            });
        });
    }

    const logs = [];
    let currentLogFilter = 'all';
    // 防止渲染风暴：批量更新时只渲染一次
    let renderScheduled = false;

    function addLog(level, message, source, time) {
        let timeStr;
        if (time) {
            // 后端提供的精确时间戳
            timeStr = time;
        } else {
            const now = new Date();
            const dateStr = `${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
            timeStr = `${dateStr} ${now.toLocaleTimeString('zh-CN', { hour12: false })}.${String(now.getMilliseconds()).padStart(3, '0')}`;
        }
        logs.push({
            level,
            message: String(message),
            source: source || '',
            time: timeStr
        });
        if (logs.length > 2000) {
            logs.shift();
        }
        scheduleRender();
    }

    function scheduleRender() {
        if (renderScheduled) return;
        renderScheduled = true;
        requestAnimationFrame(() => {
            renderScheduled = false;
            renderLogs();
        });
    }

    function getFilteredLogs() {
        if (currentLogFilter === 'all') return logs;
        return logs.filter(l => l.level === currentLogFilter);
    }

    function renderLogs() {
        const container = $('#logs-container');
        if (!container) return;

        const filtered = getFilteredLogs();

        // 更新计数
        const countEl = $('#logs-count');
        if (countEl) {
            const total = logs.length;
            const shown = filtered.length;
            countEl.textContent = currentLogFilter === 'all'
                ? `${total} 条`
                : `${shown}/${total} 条`;
        }

        if (filtered.length === 0) {
            container.innerHTML = `
                <div class="log-empty">
                    <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.2" opacity="0.3"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
                    <p>${logs.length === 0 ? '暂无日志' : '当前筛选下无日志'}</p>
                </div>
            `;
            return;
        }

        // 自动滚动判断：仅在用户处于底部时跟随
        const wasAtBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 40;

        const html = filtered.map(log => {
            const sourceHtml = log.source
                ? `<span class="log-source">[${escapeHtml(log.source)}]</span>`
                : '';
            return `<div class="log-line ${log.level}">
                <span class="log-time">${escapeHtml(log.time)}</span>
                <span class="log-level-tag ${log.level}">${log.level}</span>
                ${sourceHtml}
                <span class="log-message">${escapeHtml(log.message)}</span>
            </div>`;
        }).join('');
        container.innerHTML = html;

        if (wasAtBottom) {
            container.scrollTop = container.scrollHeight;
        }
    }

    function formatLogLine(log) {
        const source = log.source ? ` [${log.source}]` : '';
        return `${log.time} [${log.level.toUpperCase()}]${source} ${log.message}`;
    }

    function initLogs() {
        const clearBtn = $('#btn-clear-logs');
        if (clearBtn) {
            clearBtn.addEventListener('click', () => {
                logs.length = 0;
                renderLogs();
            });
        }

        const copyAllBtn = $('#btn-copy-logs');
        if (copyAllBtn) {
            copyAllBtn.addEventListener('click', () => {
                const filtered = getFilteredLogs();
                if (filtered.length === 0) return;
                const text = filtered.map(formatLogLine).join('\n');
                copyToClipboard(text);
            });
        }

        $$('.log-level-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('.log-level-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                currentLogFilter = btn.dataset.level;
                renderLogs();
                moveSegIndicator(btn);
            });
        });

        const origError = console.error;
        const origWarn = console.warn;
        const origInfo = console.info;
        const origLog = console.log;

        console.error = function(...args) {
            addLog('error', args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' '), 'console');
            origError.apply(console, args);
        };
        console.warn = function(...args) {
            addLog('warn', args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' '), 'console');
            origWarn.apply(console, args);
        };
        console.info = function(...args) {
            addLog('info', args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' '), 'console');
            origInfo.apply(console, args);
        };
        console.log = function(...args) {
            addLog('info', args.map(a => typeof a === 'object' ? JSON.stringify(a) : String(a)).join(' '));
            origLog.apply(console, args);
        };
    }

    function initHotkeyInputs() {
        $$('.hotkey-input').forEach(input => {
            input.addEventListener('click', () => {
                input.value = '按下按键...';
                input.dataset.listening = 'true';

                const onKeyDown = (e) => {
                    e.preventDefault();
                    if (input.dataset.listening !== 'true') return;

                    let keyName = '';
                    if (e.key.startsWith('F') && e.key.length <= 3) {
                        keyName = e.key;
                    } else if (e.key.length === 1) {
                        keyName = e.key.toUpperCase();
                    } else {
                        const specialKeys = {
                            'Escape': 'Esc',
                            'Backspace': 'Backspace',
                            'Tab': 'Tab',
                            'Enter': 'Enter',
                            ' ': 'Space'
                        };
                        keyName = specialKeys[e.key] || e.key;
                    }

                    input.value = keyName;
                    input.dataset.listening = 'false';
                    document.removeEventListener('keydown', onKeyDown);
                    // 立即更新主界面快捷键提示
                    updateHotkeyHint();
                };

                document.addEventListener('keydown', onKeyDown);
            });
        });
    }

    // ===== 自动更新 =====
    const updateState = {
        checking: false,
        hasUpdate: false,
        info: null,
        downloaded: false
    };

    async function loadAppVersion() {
        if (!invoke) return;
        try {
            const v = await invoke('get_app_version');
            const el = $('#app-version-text');
            if (el) el.textContent = 'v' + v;
        } catch (err) {
            console.error('Failed to get app version:', err);
        }
    }

    async function checkForUpdate() {
        if (!invoke || updateState.checking) return;
        updateState.checking = true;
        const btn = $('#btn-check-update');
        if (btn) { btn.disabled = true; btn.textContent = '检查中...'; }
        const statusRow = $('#update-status-row');
        const statusText = $('#update-status-text');
        if (statusRow) statusRow.style.display = '';
        if (statusText) statusText.textContent = '正在检查更新...';

        try {
            const result = await invoke('check_update');
            const latestEl = $('#latest-version-text');
            if (latestEl) latestEl.textContent = result.version || '-';

            if (result.has_update) {
                updateState.hasUpdate = true;
                updateState.info = result;
                if (statusText) statusText.textContent = '发现新版本 ' + result.version;
                const dlBtn = $('#btn-download-update');
                if (dlBtn) dlBtn.style.display = '';
                if (btn) btn.textContent = '重新检查';
                // 显示更新内容
                const notesRow = $('#update-notes-row');
                const notesContent = $('#update-notes-content');
                if (notesRow && notesContent && result.body) {
                    notesContent.textContent = result.body;
                    notesRow.style.display = '';
                }
            } else {
                if (statusText) statusText.textContent = '已是最新版本';
                if (btn) btn.textContent = '重新检查';
                const dlBtn = $('#btn-download-update');
                if (dlBtn) dlBtn.style.display = 'none';
                const notesRow = $('#update-notes-row');
                if (notesRow) notesRow.style.display = 'none';
            }
        } catch (err) {
            console.error('Failed to check update:', err);
            if (statusText) statusText.textContent = '检查失败: ' + err;
        } finally {
            updateState.checking = false;
            if (btn) { btn.disabled = false; }
        }
    }

    async function downloadAndInstallUpdate() {
        if (!invoke || !updateState.info) return;
        const dlBtn = $('#btn-download-update');
        if (dlBtn) { dlBtn.disabled = true; dlBtn.textContent = '下载中...'; }
        const progressRow = $('#update-progress-row');
        if (progressRow) progressRow.style.display = '';

        try {
            await invoke('download_and_install_update', {
                url: updateState.info.download_url,
                filename: updateState.info.filename
            });
            updateState.downloaded = true;
            const statusText = $('#update-status-text');
            if (statusText) statusText.textContent = '下载完成，点击重启应用以完成安装';
            if (dlBtn) dlBtn.style.display = 'none';
            const restartBtn = $('#btn-restart-app');
            if (restartBtn) restartBtn.style.display = '';
        } catch (err) {
            console.error('Failed to download update:', err);
            const statusText = $('#update-status-text');
            if (statusText) statusText.textContent = '下载失败: ' + err;
            if (dlBtn) { dlBtn.disabled = false; dlBtn.textContent = '下载并安装'; }
        }
    }

    function restartApp() {
        if (!invoke) return;
        invoke('restart_app').catch(err => console.error('Failed to restart:', err));
    }

    function initUpdateChecker() {
        const btn = $('#btn-check-update');
        if (btn) btn.addEventListener('click', checkForUpdate);
        const dlBtn = $('#btn-download-update');
        if (dlBtn) dlBtn.addEventListener('click', downloadAndInstallUpdate);
        const restartBtn = $('#btn-restart-app');
        if (restartBtn) restartBtn.addEventListener('click', restartApp);

        // 监听下载进度
        if (listen) {
            listen('update-download-progress', (event) => {
                const { downloaded, total } = event.payload;
                const bar = $('#update-progress-bar');
                const text = $('#update-progress-text');
                const pct = total > 0 ? (downloaded / total * 100) : 0;
                if (bar) bar.style.width = pct + '%';
                if (text) {
                    const mbDone = (downloaded / 1024 / 1024).toFixed(1);
                    const mbTotal = (total / 1024 / 1024).toFixed(1);
                    text.textContent = pct.toFixed(0) + '% (' + mbDone + '/' + mbTotal + ' MB)';
                }
            });
        }

        loadAppVersion();
    }

    function initOutput() {
        const output = $('#dictation-output');
        if (output) {
            output.addEventListener('focus', () => {
            });
        }
    }

    function setupTauriEventListeners() {
        if (!listen) {
            console.log('Tauri event listener not available');
            return;
        }

        listen('recording-started', () => {
            state.isRecording = true;
            const micBtn = $('#mic-btn');
            const micHint = $('.mic-hint');
            if (micBtn) micBtn.classList.add('recording');
            if (micHint) micHint.textContent = state.triggerMode === 'hold' ? '松开停止录音' : '点击停止录音';
            setStatus('recording', '正在录音...');
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        listen('recording-result', (event) => {
            state.isRecording = false;
            const micBtn = $('#mic-btn');
            const micHint = $('.mic-hint');
            if (micBtn) micBtn.classList.remove('recording');
            if (micHint) micHint.textContent = state.triggerMode === 'hold' ? '按住开始录音' : '点击开始录音';

            const text = event.payload;
            if (text && text.trim()) {
                appendToOutput(text);
            }
            setStatus('idle', '就绪');
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        listen('force-cancelled', () => {
            state.isRecording = false;
            state.isMouseDown = false;
            const micBtn = $('#mic-btn');
            const micHint = $('.mic-hint');
            if (micBtn) micBtn.classList.remove('recording');
            if (micHint) micHint.textContent = state.triggerMode === 'hold' ? '按住开始录音' : '点击开始录音';
            setStatus('idle', '就绪');
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        listen('app-status', (event) => {
            const status = event.payload;
            let statusKey = 'idle';
            let statusText = '就绪';

            if (status === 'recording') {
                statusKey = 'recording';
                statusText = '正在录音...';
                state.isRecording = true;
            } else if (status === 'processing') {
                statusKey = 'processing';
                statusText = '正在识别...';
            } else if (status && status.startsWith('error:')) {
                statusKey = 'error';
                statusText = '错误: ' + status.substring(6);
                state.isRecording = false;
            } else {
                statusKey = 'idle';
                statusText = '就绪';
                state.isRecording = false;
            }

            setStatus(statusKey, statusText);

            const micBtn = $('#mic-btn');
            const micHint = $('.mic-hint');
            if (state.isRecording) {
                if (micBtn) micBtn.classList.add('recording');
                if (micHint) micHint.textContent = state.triggerMode === 'hold' ? '松开停止录音' : '点击停止录音';
            } else {
                if (micBtn) micBtn.classList.remove('recording');
                if (micHint) micHint.textContent = state.triggerMode === 'hold' ? '按住开始录音' : '点击开始录音';
            }
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        listen('app-ready', () => {
            setStatus('idle', '就绪');
            updateSubtitlePreview();

            if (invoke) {
                invoke('is_subtitle_running').then(running => {
                    state.isSubtitleActive = running;
                    updateSubtitleButton();
                }).catch(() => {});
                // 加载上一次会话遗留的转录（服务保留到下次会话启动）
                invoke('get_subtitle_transcript').then(segments => {
                    if (Array.isArray(segments) && segments.length) renderTranscript(segments);
                }).catch(() => {});
            }
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        // 转录更新（句段定稿）
        listen('subtitle-transcript-updated', (event) => {
            const p = event.payload || {};
            renderTranscript(p.segments || []);
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        // 会话生命周期
        listen('subtitle-session-started', () => {
            state.isSubtitleActive = true;
            updateSubtitleButton();
            renderTranscript([]);
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        listen('subtitle-session-stopped', () => {
            state.isSubtitleActive = false;
            updateSubtitleButton();
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        listen('subtitle-text', () => {
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        // 监听后端日志事件，将后端 log::* 输出同步到前端日志视图
        listen('backend-log', (event) => {
            const p = event.payload;
            if (p && p.message) {
                addLog(p.level || 'info', p.message, p.source || 'backend', p.time);
            }
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });
    }

    // ====== 语音合成（TTS / Fish Audio）======

    /// 将本地文件路径转为可在 <audio>/<img> 中加载的 URL（Tauri asset 协议）
    function toAssetUrl(path) {
        try {
            const t = window.__TAURI__;
            const conv = t && t.core && t.core.convertFileSrc;
            if (conv) return conv(path);
        } catch (e) {}
        // 兜底：手动构造 asset 协议 URL（Windows: http://asset.localhost/...）
        return 'http://asset.localhost/' + encodeURIComponent(path);
    }

    /// 加载 TTS 视图：确保配置就绪并填充 UI
    async function loadTtsView() {
        if (!invoke) return;
        if (!state.config) {
            try {
                state.config = await invoke('get_config');
            } catch (e) {
                console.error('Failed to load config for TTS:', e);
                return;
            }
        }
        populateTtsUi(state.config.tts || {});
        if (!state.tts.loaded) {
            state.tts.loaded = true;
        }
    }

    /// 将 TTS 配置填充到 UI 控件
    function populateTtsUi(tts) {
        const set = (id, val) => { const el = $('#' + id); if (el && val !== undefined && val !== null) el.value = val; };
        const setChk = (id, val) => { const el = $('#' + id); if (el) el.checked = !!val; };

        set('tts-api-key', tts.fish_api_key);
        set('tts-model', tts.model || 's2.1-pro-free');
        set('tts-format', tts.format || 'mp3');
        set('tts-mp3-bitrate', String(tts.mp3_bitrate || 128));
        set('tts-latency', tts.latency || 'normal');
        set('tts-speed', tts.speed ?? 1);
        set('tts-volume', tts.volume ?? 0);
        set('tts-temperature', tts.temperature ?? 0.7);
        set('tts-top-p', tts.top_p ?? 0.7);
        set('tts-chunk-length', tts.chunk_length || 200);
        setChk('tts-normalize', tts.normalize !== false);

        // 回显选中的音色
        const titleEl = $('#tts-voice-title');
        const subEl = $('#tts-voice-sub');
        const clearBtn = $('#btn-tts-clear-voice');
        if (tts.reference_id) {
            if (titleEl) titleEl.textContent = tts.reference_title || '已选音色';
            if (subEl) subEl.textContent = tts.reference_id;
            if (clearBtn) clearBtn.style.display = '';
        } else {
            if (titleEl) titleEl.textContent = '默认音色';
            if (subEl) subEl.textContent = '使用模型默认音色';
            if (clearBtn) clearBtn.style.display = 'none';
        }

        updateTtsSliderLabels();
        updateTtsModelBadge();
        updateTtsBitrateVisibility();
    }

    function updateTtsModelBadge() {
        const el = $('#tts-model-badge');
        const sel = $('#tts-model');
        if (el && sel) el.textContent = sel.value;
    }

    function updateTtsBitrateVisibility() {
        const fmt = $('#tts-format');
        const row = $('#tts-bitrate-row');
        if (row && fmt) row.style.display = fmt.value === 'mp3' ? '' : 'none';
    }

    function updateTtsSliderLabels() {
        const speed = $('#tts-speed');
        const vol = $('#tts-volume');
        const temp = $('#tts-temperature');
        const topp = $('#tts-top-p');
        const chunk = $('#tts-chunk-length');
        if (speed) $('#tts-speed-val').textContent = parseFloat(speed.value).toFixed(2) + 'x';
        if (vol) $('#tts-volume-val').textContent = (vol.value >= 0 ? '+' : '') + vol.value + ' dB';
        if (temp) $('#tts-temp-val').textContent = parseFloat(temp.value).toFixed(2);
        if (topp) $('#tts-topp-val').textContent = parseFloat(topp.value).toFixed(2);
        if (chunk) $('#tts-chunk-val').textContent = chunk.value;
    }

    /// 从 UI 收集 TTS 配置并写入 state.config.tts
    function collectTtsConfig() {
        if (!state.config) state.config = {};
        if (!state.config.tts) state.config.tts = {};
        const t = state.config.tts;
        const get = (id) => { const el = $('#' + id); return el ? el.value : undefined; };
        const getNum = (id, dflt) => { const el = $('#' + id); return el ? parseFloat(el.value) : dflt; };
        const getChk = (id) => { const el = $('#' + id); return el ? el.checked : false; };

        t.fish_api_key = get('tts-api-key') ?? t.fish_api_key ?? '';
        t.model = get('tts-model') || 's2.1-pro-free';
        t.format = get('tts-format') || 'mp3';
        t.mp3_bitrate = parseInt(get('tts-mp3-bitrate') || '128', 10);
        t.latency = get('tts-latency') || 'normal';
        t.speed = getNum('tts-speed', 1);
        t.volume = getNum('tts-volume', 0);
        t.temperature = getNum('tts-temperature', 0.7);
        t.top_p = getNum('tts-top-p', 0.7);
        t.chunk_length = parseInt(get('tts-chunk-length') || '200', 10);
        t.normalize = getChk('tts-normalize');
        // sample_rate 保持已有值（UI 不暴露）
        return t;
    }

    /// 防抖保存 TTS 配置
    function saveTtsConfigDebounced() {
        if (!invoke || !state.config) return;
        if (state.tts.savingTimer) clearTimeout(state.tts.savingTimer);
        state.tts.savingTimer = setTimeout(async () => {
            try {
                await invoke('save_config', { newConfig: state.config });
            } catch (e) {
                console.warn('Failed to save TTS config:', e);
            }
        }, 500);
    }

    function setTtsStatus(cls, text) {
        const el = $('#tts-status');
        if (!el) return;
        el.className = 'tts-status' + (cls ? ' ' + cls : '');
        el.textContent = text;
    }

    /// 生成语音
    async function synthesizeTts() {
        if (!invoke) return;
        if (state.tts.synthesizing) return; // 防止重入

        const text = ($('#tts-text') || {}).value || '';
        if (!text.trim()) {
            setTtsStatus('error', '请输入文本');
            setTimeout(() => setTtsStatus('', '就绪'), 2000);
            return;
        }
        const tts = collectTtsConfig();
        if (!tts.fish_api_key) {
            setTtsStatus('error', '请先填写 API Key');
            return;
        }

        // 设置重入标志并禁用按钮（在弹窗前，防止弹窗期间再次点击触发并发请求）
        state.tts.synthesizing = true;
        const btn = $('#btn-tts-synthesize');
        const dlBtn = $('#btn-tts-download');
        if (btn) btn.disabled = true;

        // 若文本有改动且已有生成记录，弹窗确认是否重新生成
        const hasExistingAudio = !!state.tts.lastAudioPath;
        const textChanged = hasExistingAudio && state.tts.lastText !== text;
        if (textChanged) {
            const { confirmed } = await showConfirmDialog(
                '重新生成确认',
                '文本已修改，是否使用新文本重新生成语音？',
                '重新生成'
            );
            if (!confirmed) {
                state.tts.synthesizing = false;
                if (btn) btn.disabled = false;
                return;
            }
        }

        saveTtsConfigDebounced();

        const audio = $('#tts-audio');
        const wrap = $('#tts-player-wrap');
        const empty = $('#tts-empty-hint');
        // 保存旧音频路径，失败时可用于恢复
        const prevPath = state.tts.lastAudioPath;

        // 生成期间：暂停旧音频、隐藏播放器和下载按钮，下方音频区域消失直至生成完成
        if (audio) {
            audio.pause();
            audio.removeAttribute('src');
            audio.load();
        }
        if (wrap) wrap.style.display = 'none';
        if (dlBtn) dlBtn.disabled = true;
        if (empty) empty.style.display = 'none';
        setTtsStatus('busy', '生成中...');

        try {
            const path = await invoke('tts_synthesize', { text });
            state.tts.lastAudioPath = path;
            state.tts.lastText = text;

            // 生成完成：显示新音频（不自动播放，由用户手动点击播放）
            if (audio) {
                audio.src = toAssetUrl(path);
                audio.load();
            }
            if (wrap) wrap.style.display = '';
            if (empty) empty.style.display = 'none';
            if (dlBtn) dlBtn.disabled = false;
            setTtsStatus('ok', '生成成功');
        } catch (e) {
            console.error('TTS synthesize failed:', e);
            setTtsStatus('error', '生成失败：' + (e || '未知错误'));
            // 失败时恢复旧音频（若有），否则显示空提示
            if (prevPath) {
                if (audio) {
                    audio.src = toAssetUrl(prevPath);
                    audio.load();
                }
                if (wrap) wrap.style.display = '';
                if (dlBtn) dlBtn.disabled = false;
            } else {
                if (empty) empty.style.display = '';
            }
        } finally {
            state.tts.synthesizing = false;
            if (btn) btn.disabled = false;
        }
    }

    /// 下载（保存为文件）
    async function downloadTts() {
        if (!invoke || !state.tts.lastAudioPath) return;
        const dlBtn = $('#btn-tts-download');
        if (dlBtn) dlBtn.disabled = true;
        setTtsStatus('busy', '保存中...');
        try {
            const name = (state.tts.lastText || 'tts_output').slice(0, 20).replace(/[\\/:*?"<>|\n\r]/g, '').trim() || 'tts_output';
            const result = await invoke('tts_export', { srcPath: state.tts.lastAudioPath, fileName: name });
            if (result) {
                setTtsStatus('ok', '已保存');
            } else {
                setTtsStatus('', '就绪');
            }
        } catch (e) {
            console.error('TTS export failed:', e);
            setTtsStatus('error', '保存失败：' + (e || '未知错误'));
        } finally {
            if (dlBtn) dlBtn.disabled = false;
        }
    }

    // ---- 音色库浏览器 ----

    function openVoiceLib() {
        const modal = $('#tts-voice-modal');
        if (!modal) return;
        modal.style.display = 'flex';
        state.tts.voicePage = 1;
        loadVoices();
    }

    function closeVoiceLib() {
        const modal = $('#tts-voice-modal');
        if (modal) modal.style.display = 'none';
    }

    async function loadVoices() {
        if (!invoke) return;
        const tts = collectTtsConfig();
        if (!tts.fish_api_key) {
            setTtsStatus('error', '请先填写 API Key');
            return;
        }
        const grid = $('#tts-voice-grid');
        if (grid) grid.innerHTML = '<div class="tts-voice-loading">加载中...</div>';

        const title = ($('#tts-voice-search-input') || {}).value || '';
        const language = ($('#tts-voice-lang-filter') || {}).value || '';
        const sortBy = ($('#tts-voice-sort') || {}).value || 'score';
        const selfOnly = ($('#tts-voice-self') || {}).checked || false;

        try {
            const res = await invoke('tts_list_voices', {
                pageSize: state.tts.voicePageSize,
                pageNumber: state.tts.voicePage,
                title: title || null,
                language: language || null,
                sortBy: sortBy || null,
                selfOnly: selfOnly || null
            });
            const items = (res && res.items) || [];
            state.tts.voiceTotal = (res && res.total) ? res.total : items.length;
            renderVoices(items);
            updateVoicePager();
        } catch (e) {
            console.error('Failed to load voices:', e);
            if (grid) grid.innerHTML = '<div class="tts-voice-loading">加载失败：' + escapeHtml(String(e)) + '</div>';
        }
    }

    function renderVoices(items) {
        const grid = $('#tts-voice-grid');
        if (!grid) return;
        if (!items.length) {
            grid.innerHTML = '<div class="tts-voice-loading">未找到音色</div>';
            return;
        }
        const currentRef = (state.config && state.config.tts && state.config.tts.reference_id) || '';
        grid.innerHTML = items.map(v => {
            // 兼容新旧 API 响应：voiceId (新) / _id (旧)
            const id = v.voiceId || v._id || '';
            const title = escapeHtml(v.title || '未命名');
            const desc = escapeHtml((v.languages && v.languages.length) ? v.languages.join(', ') : (v.description || ''));
            const author = v.author ? escapeHtml(v.author.nickname || '') : (v.isPersonal ? '自建' : '');
            const cover = v.cover_image ? `style="background-image:url('${escapeAttr(v.cover_image)}')"` : '';
            const tags = (v.tags || []).slice(0, 3).map(tg => `<span class="vv-tag">${escapeHtml(tg)}</span>`).join('');
            const sel = id === currentRef ? ' selected' : '';
            return `<div class="tts-voice-item${sel}" data-id="${escapeAttr(id)}" data-title="${escapeAttr(v.title || '')}">
                <div class="vv-avatar" ${cover}></div>
                <div class="vv-body">
                    <div class="vv-title">${title}</div>
                    <div class="vv-desc">${desc}${author ? ' · ' + author : ''}</div>
                    <div class="vv-tags">${tags}</div>
                </div>
            </div>`;
        }).join('');

        grid.querySelectorAll('.tts-voice-item').forEach(item => {
            item.addEventListener('click', () => {
                selectVoice(item.dataset.id, item.dataset.title);
                closeVoiceLib();
            });
        });
    }

    function selectVoice(id, title) {
        if (!state.config) state.config = {};
        if (!state.config.tts) state.config.tts = {};
        state.config.tts.reference_id = id || '';
        state.config.tts.reference_title = title || '';
        populateTtsUi(state.config.tts);
        saveTtsConfigDebounced();
        setTtsStatus('ok', '已选择音色');
        setTimeout(() => setTtsStatus('', '就绪'), 1500);
    }

    function clearVoice() {
        if (!state.config) state.config = {};
        if (!state.config.tts) state.config.tts = {};
        state.config.tts.reference_id = '';
        state.config.tts.reference_title = '';
        populateTtsUi(state.config.tts);
        saveTtsConfigDebounced();
    }

    function updateVoicePager() {
        const info = $('#tts-voice-page-info');
        const prev = $('#btn-tts-voice-prev');
        const next = $('#btn-tts-voice-next');
        const pageSize = state.tts.voicePageSize;
        const totalPages = Math.max(1, Math.ceil(state.tts.voiceTotal / pageSize));
        if (info) info.textContent = `${state.tts.voicePage} / ${totalPages}（共 ${state.tts.voiceTotal}）`;
        if (prev) prev.disabled = state.tts.voicePage <= 1;
        if (next) next.disabled = state.tts.voicePage >= totalPages;
    }

    /// 初始化 TTS 视图事件
    function initTts() {
        const text = $('#tts-text');
        if (text) {
            text.addEventListener('input', () => {
                const cc = $('#tts-char-count');
                if (cc) cc.textContent = text.value.length + ' 字';
            });
        }

        // 滑块/选择器：实时更新标签 + 防抖保存
        const liveIds = ['tts-speed', 'tts-volume', 'tts-temperature', 'tts-top-p', 'tts-chunk-length'];
        liveIds.forEach(id => {
            const el = $('#' + id);
            if (el) el.addEventListener('input', () => { updateTtsSliderLabels(); collectTtsConfig(); saveTtsConfigDebounced(); });
        });
        const changeIds = ['tts-model', 'tts-format', 'tts-mp3-bitrate', 'tts-latency', 'tts-normalize', 'tts-api-key'];
        changeIds.forEach(id => {
            const el = $('#' + id);
            if (!el) return;
            const evt = el.type === 'checkbox' ? 'change' : 'change';
            el.addEventListener(evt, () => {
                collectTtsConfig();
                saveTtsConfigDebounced();
                updateTtsModelBadge();
                updateTtsBitrateVisibility();
            });
        });

        const synthBtn = $('#btn-tts-synthesize');
        if (synthBtn) synthBtn.addEventListener('click', synthesizeTts);
        const dlBtn = $('#btn-tts-download');
        if (dlBtn) dlBtn.addEventListener('click', downloadTts);

        // 音色库
        const libBtn = $('#btn-tts-voice-lib');
        if (libBtn) libBtn.addEventListener('click', openVoiceLib);
        const closeBtn = $('#btn-tts-voice-close');
        if (closeBtn) closeBtn.addEventListener('click', closeVoiceLib);
        const clearBtn = $('#btn-tts-clear-voice');
        if (clearBtn) clearBtn.addEventListener('click', clearVoice);

        // 点击模态背景关闭
        const modal = $('#tts-voice-modal');
        if (modal) {
            modal.addEventListener('click', (e) => { if (e.target === modal) closeVoiceLib(); });
        }

        // 搜索 / 筛选（防抖）
        const searchInput = $('#tts-voice-search-input');
        if (searchInput) {
            searchInput.addEventListener('input', () => {
                if (state.tts.voiceSearchTimer) clearTimeout(state.tts.voiceSearchTimer);
                state.tts.voiceSearchTimer = setTimeout(() => {
                    state.tts.voicePage = 1;
                    loadVoices();
                }, 400);
            });
        }
        ['tts-voice-lang-filter', 'tts-voice-sort', 'tts-voice-self'].forEach(id => {
            const el = $('#' + id);
            if (el) el.addEventListener('change', () => { state.tts.voicePage = 1; loadVoices(); });
        });

        // 分页
        const prevBtn = $('#btn-tts-voice-prev');
        if (prevBtn) prevBtn.addEventListener('click', () => {
            if (state.tts.voicePage > 1) { state.tts.voicePage--; loadVoices(); }
        });
        const nextBtn = $('#btn-tts-voice-next');
        if (nextBtn) nextBtn.addEventListener('click', () => {
            state.tts.voicePage++;
            loadVoices();
        });

        // API Key 链接
        const apiLink = $('#tts-api-link');
        if (apiLink) {
            apiLink.addEventListener('click', async (e) => {
                e.preventDefault();
                if (window.__TAURI__ && window.__TAURI__.shell && window.__TAURI__.shell.open) {
                    await window.__TAURI__.shell.open('https://fish.audio/app/api-keys');
                }
            });
        }
    }

    function init() {
        // 早期应用主题（从 localStorage 缓存），避免主题闪烁
        try {
            const cached = localStorage.getItem('v2t-theme');
            if (cached) document.documentElement.setAttribute('data-theme', cached);
        } catch (e) {}

        initLogs();
        initWindowControls();
        initNavigation();
        initDictation();
        initSubtitle();
        initHistory();
        initSettingsSliders();
        initApiKeyToggles();
        initHotkeyInputs();
        initOutput();
        initUpdateChecker();
        initSliderFills();
        initCollapsibleSections();
        initThemeSwitcher();
        initLlmPostToggle();
        initSettingsDirtyTracking();
        initAudioQualityInteractions();
        initTts();

        // 禁用 WebView 默认右键菜单（刷新、检查、另存为等），
        // 历史记录项的 contextmenu 监听器已自行处理 preventDefault，不受影响。
        document.addEventListener('contextmenu', (e) => {
            if (!e.target.closest('.history-item')) {
                e.preventDefault();
            }
        });

        // 禁用常用浏览器快捷键，避免干扰应用使用
        // F1-F12、F5 刷新、Ctrl+R、Ctrl+Shift+I/J/C 开发者工具、Ctrl+ +/- 缩放、Ctrl+0、Ctrl+P、Ctrl+S、Ctrl+F、Alt+方向键
        document.addEventListener('keydown', (e) => {
            // F1-F12：全部阻止默认行为（F1 帮助、F3 搜索、F5 刷新、F11 全屏、F12 开发者工具等）
            if (/^F\d{1,2}$/.test(e.key)) {
                e.preventDefault();
                return;
            }
            const k = e.key.toLowerCase();
            // Ctrl+R / Ctrl+Shift+R 刷新
            if (k === 'r' && e.ctrlKey) {
                e.preventDefault();
                return;
            }
            // Ctrl+Shift+I / J / C 开发者工具
            if (e.ctrlKey && e.shiftKey && (k === 'i' || k === 'j' || k === 'c')) {
                e.preventDefault();
                return;
            }
            // Ctrl++ / Ctrl+- / Ctrl+0 页面缩放
            if (e.ctrlKey && (k === '+' || k === '-' || k === '0' || k === '=')) {
                e.preventDefault();
                return;
            }
            // Ctrl+P 打印、Ctrl+S 保存、Ctrl+F 查找
            if (e.ctrlKey && (k === 'p' || k === 's' || k === 'f')) {
                e.preventDefault();
                return;
            }
            // Ctrl+G / F3 查找下一个（已在 F1-F12 覆盖 F3）
            if (e.ctrlKey && k === 'g') {
                e.preventDefault();
                return;
            }
            // Alt+方向键 浏览器历史导航
            if (e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
                e.preventDefault();
                return;
            }
            // Backspace 浏览器后退（非输入元素时）
            if (e.key === 'Backspace' && !isEditableTarget(e.target)) {
                e.preventDefault();
                return;
            }
        });

        function isEditableTarget(target) {
            if (!target) return false;
            const tag = target.tagName;
            if (tag === 'INPUT' || tag === 'TEXTAREA') return true;
            if (tag === 'SELECT') return true;
            if (target.isContentEditable) return true;
            return false;
        }

        setupTauriEventListeners();

        if (invoke) {
            loadSettings();
            loadInputDevices();
            updateSubtitlePreview();
        } else {
            console.log('Running in browser mode - Tauri API not available');
            setupDefaultSettings();
            updateSubtitlePreview();
        }

        setStatus('idle', '就绪');
        updateMicHint();
        updateHotkeyHint();

        // 初始化所有滑动指示器位置（等 DOM 完成布局后再计算）
        requestAnimationFrame(() => {
            refreshAllIndicators();
            // 字体渲染/异步布局可能导致首次测量偏差，再补一次刷新
            setTimeout(refreshAllIndicators, 60);
        });

        // 窗口尺寸变化时同步刷新指示器位置（防抖）
        let resizeTimer = null;
        window.addEventListener('resize', () => {
            if (resizeTimer) clearTimeout(resizeTimer);
            resizeTimer = setTimeout(refreshAllIndicators, 80);
        });

        console.log('Voice2Type UI initialized');
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();
