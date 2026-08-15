(function() {
    'use strict';

    const state = {
        currentView: 'dictation',
        previousView: 'dictation',
        isRecording: false,
        isSubtitleActive: false,
        triggerMode: 'hold',
        dictationMode: 'batch',
        transcriptSegments: [],
        subtitleWindows: [],
        currentWindowId: 'primary',
        selectedElementId: null,
        previewInterim: false,
        config: null,
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

    /// 解析预览占位符（{time} {date} {datetime} {text} {translation} {speaker}）
    function resolvePreviewPlaceholders(text, samples) {
        if (!text) return '';
        const now = new Date();
        const s = samples || {};
        const h = String(now.getHours()).padStart(2, '0');
        const m = String(now.getMinutes()).padStart(2, '0');
        const sec = String(now.getSeconds()).padStart(2, '0');
        const y = now.getFullYear();
        const mo = String(now.getMonth() + 1).padStart(2, '0');
        const d = String(now.getDate()).padStart(2, '0');
        return text
            .replace(/\{time\}/g, `${h}:${m}:${sec}`)
            .replace(/\{date\}/g, `${y}-${mo}-${d}`)
            .replace(/\{datetime\}/g, `${y}-${mo}-${d} ${h}:${m}:${sec}`)
            .replace(/\{text\}/g, s.text || '实时字幕预览效果')
            .replace(/\{translation\}/g, s.translation || '译文预览效果')
            .replace(/\{speaker\}/g, s.speaker || '说话人1');
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

    /// 为文本类预览元素应用基础排版（字体/对齐/间距/行高/阴影）
    function applyPreviewTextBase(st, theme, textShadow) {
        st.fontFamily = `"${theme.fontFamily || 'SimHei'}", sans-serif`;
        st.textAlign = theme.textAlign || 'center';
        st.letterSpacing = (theme.letterSpacing || 0) + 'px';
        st.lineHeight = String(theme.lineHeight != null ? theme.lineHeight : 1.4);
        st.textShadow = textShadow;
        st.wordWrap = 'break-word';
        st.wordBreak = 'break-word';
    }

    /// 渲染单个预览元素节点（按 kind），返回 DOM 节点或 null（不显示时）
    function renderPreviewElement(el, theme, textShadow) {
        const kind = el.kind || 'text';

        if (kind === 'divider') {
            const node = document.createElement('div');
            node.className = 'sub-preview-divider';
            node.dataset.previewKind = kind;
            const st = node.style;
            st.height = '1px';
            st.width = '100%';
            st.background = el.color || '#ffffff';
            st.opacity = String(typeof el.opacity === 'number' ? el.opacity : 0.3);
            st.margin = '4px 0';
            return node;
        }
        if (kind === 'spacer') {
            const node = document.createElement('div');
            node.className = 'sub-preview-spacer';
            node.dataset.previewKind = kind;
            node.style.height = (typeof el.fontSize === 'number' && el.fontSize > 0 ? el.fontSize : 12) + 'px';
            return node;
        }

        // 原文元素：历史行 + 当前行（最终/临时两种状态）
        if (kind === 'original') {
            const wrap = document.createElement('div');
            wrap.className = 'sub-preview-element sub-preview-original';
            wrap.dataset.previewKind = kind;
            applyPreviewTextBase(wrap.style, theme, textShadow);
            wrap.style.fontSize = (theme.fontSize || 32) + 'px';
            wrap.style.fontWeight = String(theme.fontWeight || 400);
            wrap.style.fontStyle = theme.italic ? 'italic' : 'normal';

            const cur = document.createElement('div');
            cur.style.color = state.previewInterim
                ? (theme.interimColor || '#ffffff')
                : (theme.fontColor || '#ffffff');
            cur.style.opacity = state.previewInterim
                ? String(theme.interimOpacity != null ? theme.interimOpacity : 0.7)
                : '1';
            cur.textContent = state.previewInterim ? '临时识别结果预览效果...' : '实时字幕预览效果';

            // 层级：历史行在上，当前行（定稿/临时）永远在最下
            const histCount = Math.max(0, Math.min(theme.maxLines || 3, 6) - 1);
            for (let i = 0; i < histCount; i++) {
                const hline = document.createElement('div');
                hline.className = 'sub-preview-history-line';
                hline.style.opacity = String(Math.min(0.95, 0.5 + 0.15 * i));
                hline.textContent = '历史字幕示例行';
                wrap.appendChild(hline);
            }
            wrap.appendChild(cur);
            return wrap;
        }

        // 文本类元素（speaker / translation / secondary / timestamp / 自定义 text）
        const node = document.createElement('div');
        node.className = 'sub-preview-element';
        node.dataset.previewKind = kind;
        const st = node.style;

        if (kind === 'speaker') {
            const spk = theme.speaker || {};
            st.color = spk.color || '#818cf8';
            st.fontSize = (spk.size || 16) + 'px';
            st.fontWeight = '500';
            st.textAlign = theme.textAlign || 'center';
            node.textContent = (spk.prefix || '') + '说话人1';
        } else if (kind === 'translation') {
            const tr = theme.translation || {};
            applyPreviewTextBase(st, theme, textShadow);
            st.fontSize = (tr.size || 24) + 'px';
            st.fontWeight = String(tr.weight || 400);
            st.color = tr.color || '#ffffff';
            st.opacity = String(tr.opacity != null ? tr.opacity : 0.85);
            node.textContent = (tr.prefix || '') + '译文预览效果';
        } else if (kind === 'secondary') {
            const sec = theme.secondary || {};
            const secSize = sec.size > 0 ? sec.size : Math.max(14, Math.round((theme.fontSize || 32) * 0.8));
            applyPreviewTextBase(st, theme, textShadow);
            st.fontSize = secSize + 'px';
            st.fontWeight = String(theme.fontWeight || 400);
            st.color = sec.color || '#7dd3fc';
            st.opacity = String(sec.opacity != null ? sec.opacity : 0.9);
            node.textContent = '副原文预览效果（麦克风）';
        } else if (kind === 'timestamp') {
            const ts = theme.timestamp || {};
            if (ts.format === 'none') return null;
            st.color = ts.color || '#a1a1aa';
            st.fontSize = (ts.size || 14) + 'px';
            st.fontWeight = '400';
            st.fontFamily = '"Cascadia Code", "Consolas", monospace';
            st.textAlign = theme.textAlign || 'center';
            node.textContent = formatPreviewTimestamp(ts.format);
        } else {
            // 自定义 text
            applyPreviewTextBase(st, theme, textShadow);
            if (typeof el.fontSize === 'number' && el.fontSize > 0) st.fontSize = el.fontSize + 'px';
            if (typeof el.fontWeight === 'number' && el.fontWeight > 0) st.fontWeight = String(el.fontWeight);
            st.color = el.color || '#ffffff';
            if (typeof el.opacity === 'number') st.opacity = String(el.opacity);
            st.textAlign = el.align || 'center';
            node.textContent = (el.prefix || '') + resolvePreviewPlaceholders(el.content || '自定义文本');
        }
        return node;
    }

    function updateSubtitlePreview() {
        const preview = $('#subtitle-preview');
        const box = $('#subtitle-preview-box');
        if (!preview || !box) return;

        const win = getCurrentSubtitleWindow();
        const theme = (win && win.theme) ? win.theme : defaultSubtitleTheme();
        if (!win) {
            box.innerHTML = '';
            return;
        }

        const textShadow = buildTextShadow(theme.textShadowColor, theme.textShadowStrength);

        // 容器对齐锚点 + 卡片最大宽度（与真实窗口一致）
        const alignXMap = { left: 'flex-start', center: 'center', right: 'flex-end' };
        const alignYMap = { top: 'flex-start', center: 'center', bottom: 'flex-end' };
        preview.style.justifyContent = alignXMap[theme.anchorX] || 'center';
        preview.style.alignItems = alignYMap[theme.anchorY] || 'flex-end';

        // 容器样式（背景/模糊/内边距/布局）
        const b = box.style;
        b.maxWidth = (theme.maxWidthPct != null ? theme.maxWidthPct : 100) + '%';
        b.background = hexToRgba(theme.bgColor, theme.bgOpacity != null ? theme.bgOpacity : 0.6);
        b.backdropFilter = `blur(${theme.blur != null ? theme.blur : 20}px)`;
        b.webkitBackdropFilter = `blur(${theme.blur != null ? theme.blur : 20}px)`;
        b.padding = `${theme.paddingY != null ? theme.paddingY : 12}px ${theme.paddingX != null ? theme.paddingX : 24}px`;
        const horizontal = theme.layout === 'horizontal';
        b.flexDirection = horizontal ? 'row' : 'column';
        b.alignItems = horizontal ? 'center' : 'stretch';
        b.gap = horizontal ? '16px' : '6px';

        // 按 elements 数组顺序重建预览（顺序即显示顺序）
        box.innerHTML = '';
        (win.elements || []).forEach(el => {
            if (!el.enabled) return;
            const node = renderPreviewElement(el, theme, textShadow);
            if (node) box.appendChild(node);
        });

        // 点击原文元素切换 最终/临时 预览态
        const origEl = box.querySelector('[data-preview-kind="original"]');
        if (origEl) {
            origEl.title = '点击切换 最终/临时 状态预览';
            origEl.style.cursor = 'pointer';
            origEl.addEventListener('click', () => {
                state.previewInterim = !state.previewInterim;
                updateSubtitlePreview();
            });
        }

        // 同步 value-display 文本
        updateValueDisplay('subtitle-font-size', `${theme.fontSize || 32}px`);
        updateValueDisplay('subtitle-opacity', `${Math.round((theme.bgOpacity != null ? theme.bgOpacity : 0.6) * 100)}%`);
        updateValueDisplay('subtitle-blur', `${theme.blur != null ? theme.blur : 20}px`);
        updateValueDisplay('subtitle-lines', `${theme.maxLines != null ? theme.maxLines : 3} 行`);
        updateValueDisplay('subtitle-max-width', `${theme.maxWidthPct != null ? theme.maxWidthPct : 100}%`);
        updateValueDisplay('subtitle-line-height', (theme.lineHeight != null ? theme.lineHeight : 1.4).toFixed(1));
        updateValueDisplay('subtitle-letter-spacing', `${theme.letterSpacing != null ? theme.letterSpacing : 0}px`);
        updateValueDisplay('subtitle-text-shadow-strength', String(theme.textShadowStrength != null ? theme.textShadowStrength : 4));
        updateValueDisplay('subtitle-padding-x', `${theme.paddingX != null ? theme.paddingX : 24}px`);
        updateValueDisplay('subtitle-padding-y', `${theme.paddingY != null ? theme.paddingY : 12}px`);
        updateValueDisplay('subtitle-interim-opacity', `${Math.round((theme.interimOpacity != null ? theme.interimOpacity : 0.7) * 100)}%`);
        updateValueDisplay('subtitle-translation-size', `${(theme.translation && theme.translation.size) || 24}px`);
        updateValueDisplay('subtitle-translation-opacity', `${Math.round(((theme.translation && theme.translation.opacity) != null ? theme.translation.opacity : 0.85) * 100)}%`);
        updateValueDisplay('subtitle-speaker-size', `${(theme.speaker && theme.speaker.size) || 16}px`);
        updateValueDisplay('subtitle-timestamp-size', `${(theme.timestamp && theme.timestamp.size) || 14}px`);
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

    // ===== 实时字幕 v3：窗口 / 主题 / 元素模型 =====

    /// 固定元素 kind（不可删除，仅可开关/排序）
    const FIXED_ELEMENT_KINDS = ['speaker', 'original', 'translation', 'secondary', 'timestamp'];

    /// 固定元素展示名
    const FIXED_ELEMENT_LABELS = {
        speaker: '说话人',
        original: '原文',
        translation: '译文',
        secondary: '副原文（麦克风）',
        timestamp: '时间戳'
    };

    /// 自定义元素展示名
    function elementTypeLabel(kind) {
        if (kind === 'divider') return '分隔线';
        if (kind === 'spacer') return '间距';
        return '文本';
    }

    const WEIGHT_OPTIONS = [100, 200, 300, 400, 500, 600, 700, 800, 900];

    /// 新建主题（与契约第 1 节默认值一致，camelCase）
    function defaultSubtitleTheme() {
        return {
            preset: 'custom',
            fontFamily: 'SimHei',
            fontSize: 32,
            fontWeight: 400,
            italic: false,
            textAlign: 'center',
            letterSpacing: 0,
            lineHeight: 1.4,
            textShadowColor: '#000000',
            textShadowStrength: 4,
            interimColor: '#ffffff',
            interimOpacity: 0.7,
            bgColor: '#000000',
            bgOpacity: 0.6,
            blur: 20,
            paddingX: 24,
            paddingY: 12,
            maxLines: 3,
            layout: 'vertical',
            anchorX: 'center',
            anchorY: 'bottom',
            maxWidthPct: 100,
            fontColor: '#ffffff',
            translation: { size: 24, weight: 400, color: '#ffffff', opacity: 0.85, prefix: '' },
            speaker: { color: '#818cf8', size: 16, prefix: '' },
            timestamp: { color: '#a1a1aa', size: 14, format: 'HH:MM:SS' },
            secondary: { color: '#7dd3fc', size: 0, opacity: 0.9 }
        };
    }

    function defaultFixedElements() {
        return [
            { kind: 'speaker', id: 'speaker', enabled: false, label: '说话人', content: '', prefix: '', color: '', fontSize: 0, fontWeight: 0, opacity: 1, align: '' },
            { kind: 'original', id: 'original', enabled: true, label: '原文', content: '', prefix: '', color: '', fontSize: 0, fontWeight: 0, opacity: 1, align: '' },
            { kind: 'translation', id: 'translation', enabled: true, label: '译文', content: '', prefix: '', color: '', fontSize: 0, fontWeight: 0, opacity: 1, align: '' },
            { kind: 'secondary', id: 'secondary', enabled: false, label: '副原文（麦克风）', content: '', prefix: '', color: '', fontSize: 0, fontWeight: 0, opacity: 1, align: '' },
            { kind: 'timestamp', id: 'timestamp', enabled: false, label: '时间戳', content: '', prefix: '', color: '', fontSize: 0, fontWeight: 0, opacity: 1, align: '' }
        ];
    }

    /// 新建默认窗口（primary）
    function defaultSubtitleWindow() {
        return {
            id: 'primary',
            name: '默认字幕',
            enabled: true,
            x: -1,
            y: -1,
            width: 1200,
            height: 120,
            alwaysOnTop: true,
            clickThrough: false,
            obsMode: false,
            autoFit: true,
            translation: { engine: 'none', targetLang: '英文', interim: true },
            theme: defaultSubtitleTheme(),
            elements: defaultFixedElements()
        };
    }

    /// 规整窗口对象（补齐缺失的嵌套结构，camelCase）
    function normalizeWindow(w) {
        const src = w || {};
        const theme = normalizeTheme(src.theme);
        const elements = Array.isArray(src.elements) && src.elements.length
            ? src.elements.map(normalizeElement)
            : defaultFixedElements();
        return {
            id: src.id || 'primary',
            name: src.name || '默认字幕',
            enabled: src.enabled !== false,
            x: src.x != null ? src.x : -1,
            y: src.y != null ? src.y : -1,
            width: src.width || 1200,
            height: src.height || 120,
            alwaysOnTop: src.alwaysOnTop !== false,
            clickThrough: src.clickThrough === true,
            obsMode: src.obsMode === true,
            autoFit: src.autoFit !== false,
            translation: {
                engine: (src.translation && src.translation.engine) || 'none',
                targetLang: (src.translation && src.translation.targetLang) || '英文',
                interim: !src.translation || src.translation.interim !== false
            },
            theme,
            elements
        };
    }

    function normalizeTheme(t) {
        const src = t || {};
        const base = defaultSubtitleTheme();
        return {
            preset: src.preset || 'custom',
            fontFamily: src.fontFamily || base.fontFamily,
            fontSize: src.fontSize != null ? src.fontSize : base.fontSize,
            fontWeight: src.fontWeight != null ? src.fontWeight : base.fontWeight,
            italic: src.italic === true,
            textAlign: src.textAlign || base.textAlign,
            letterSpacing: src.letterSpacing != null ? src.letterSpacing : base.letterSpacing,
            lineHeight: src.lineHeight != null ? src.lineHeight : base.lineHeight,
            textShadowColor: src.textShadowColor || base.textShadowColor,
            textShadowStrength: src.textShadowStrength != null ? src.textShadowStrength : base.textShadowStrength,
            interimColor: src.interimColor || base.interimColor,
            interimOpacity: src.interimOpacity != null ? src.interimOpacity : base.interimOpacity,
            bgColor: src.bgColor || base.bgColor,
            bgOpacity: src.bgOpacity != null ? src.bgOpacity : base.bgOpacity,
            blur: src.blur != null ? src.blur : base.blur,
            paddingX: src.paddingX != null ? src.paddingX : base.paddingX,
            paddingY: src.paddingY != null ? src.paddingY : base.paddingY,
            maxLines: src.maxLines != null ? src.maxLines : base.maxLines,
            layout: src.layout || base.layout,
            anchorX: src.anchorX || base.anchorX,
            anchorY: src.anchorY || base.anchorY,
            maxWidthPct: src.maxWidthPct != null ? src.maxWidthPct : base.maxWidthPct,
            fontColor: src.fontColor || base.fontColor,
            translation: { ...base.translation, ...(src.translation || {}) },
            speaker: { ...base.speaker, ...(src.speaker || {}) },
            timestamp: { ...base.timestamp, ...(src.timestamp || {}) },
            secondary: { ...base.secondary, ...(src.secondary || {}) }
        };
    }

    function normalizeElement(e) {
        const src = e || {};
        const isFixed = FIXED_ELEMENT_KINDS.includes(src.kind);
        const label = src.label || (isFixed ? FIXED_ELEMENT_LABELS[src.kind] : elementTypeLabel(src.kind));
        return {
            kind: isFixed ? src.kind : (['text', 'divider', 'spacer'].includes(src.kind) ? src.kind : 'text'),
            id: isFixed ? src.kind : (src.id || genCustomElementId()),
            enabled: src.enabled !== false,
            label: label || src.kind || 'text',
            content: src.content || '',
            prefix: src.prefix || '',
            color: isFixed ? (src.color || '') : (src.color || '#ffffff'),
            fontSize: src.fontSize != null ? src.fontSize : 0,
            fontWeight: src.fontWeight != null ? src.fontWeight : 0,
            opacity: src.opacity != null ? src.opacity : 1,
            align: src.align || ''
        };
    }

    function getCurrentSubtitleWindow() {
        return (state.subtitleWindows || []).find(w => w.id === state.currentWindowId) || null;
    }

    function getCurrentTheme() {
        const win = getCurrentSubtitleWindow();
        return win ? win.theme : null;
    }

    /// 从配置加载窗口列表（判定新模型 windows 数组，旧配置视为空并提示，不崩溃）
    function loadWindowsIntoState(config) {
        const sub = (config && config.subtitle) || {};
        let windows = null;
        if (Array.isArray(sub.windows)) {
            windows = sub.windows.map(normalizeWindow);
        }
        if (!windows || !windows.length) {
            windows = [defaultSubtitleWindow()];
            addLog('warn', '未检测到新版字幕窗口配置，已初始化默认字幕窗口', 'subtitle');
        }
        state.subtitleWindows = windows;
        if (!state.subtitleWindows.find(w => w.id === state.currentWindowId)) {
            state.currentWindowId = state.subtitleWindows[0].id || 'primary';
        }
        renderWindowList();
    }

    /// 渲染窗口下拉选择器
    function renderWindowList() {
        const sel = $('#subtitle-window-select');
        if (!sel) return;
        const current = state.currentWindowId;
        sel.innerHTML = (state.subtitleWindows || []).map(w =>
            `<option value="${escapeHtml(w.id)}"${w.id === current ? ' selected' : ''}>${escapeHtml(w.name || w.id)}${w.enabled ? '' : '（已停用）'}</option>`
        ).join('');
        const delBtn = $('#btn-remove-subtitle-window');
        if (delBtn) delBtn.disabled = current === 'primary';
    }

    /// 切换窗口：当前 UI 草稿写回 state，再载入目标窗口
    function switchSubtitleWindow(newId) {
        if (!newId || newId === state.currentWindowId) return;
        flushCurrentWindowName();
        state.currentWindowId = newId;
        const win = getCurrentSubtitleWindow();
        if (win && state.config) {
            populateSubtitleUiFromWindow(win, state.config);
        }
        renderWindowList();
        refreshAllIndicators();
    }

    /// 同步窗口名称输入框到当前窗口对象
    function flushCurrentWindowName() {
        const win = getCurrentSubtitleWindow();
        const nameInput = $('#subtitle-window-name');
        if (win && nameInput) {
            const name = nameInput.value.trim();
            if (name) win.name = name;
        }
    }

    /// 同声传译设置区显隐（引擎关闭时隐藏目标语言/中间结果）
    function updateTranslationUI() {
        const engine = ($('#subtitle-translation-engine') || {}).value || 'none';
        const enabled = engine !== 'none';
        const langGroup = $('#subtitle-translation-lang-group');
        const interimGroup = $('#subtitle-translation-interim-group');
        if (langGroup) langGroup.style.display = enabled ? '' : 'none';
        if (interimGroup) interimGroup.style.display = enabled ? '' : 'none';
    }

    /// 静默保存当前全部设置草稿（窗口增删前调用，避免丢失未保存修改）
    async function saveCurrentWindowDraft() {
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

    /// 后端窗口变更后重新加载并选中指定窗口
    async function reloadWindowsAfterBackendChange(selectId) {
        if (!invoke) return;
        const cfg = await invoke('get_config');
        state.config = cfg;
        loadWindowsIntoState(cfg);
        state.currentWindowId = selectId || 'primary';
        const win = getCurrentSubtitleWindow();
        if (win) populateSubtitleUiFromWindow(win, cfg);
        renderWindowList();
        state.settingsDirty = false;
        updateSubtitlePreview();
        refreshAllIndicators();
    }

    async function addSubtitleWindow() {
        if (!invoke) return;
        await saveCurrentWindowDraft();
        try {
            const id = await invoke('subtitle_add_window');
            await reloadWindowsAfterBackendChange(id);
            addLog('info', '已添加字幕窗口', 'subtitle');
        } catch (err) {
            console.error('Failed to add subtitle window:', err);
            alert('添加窗口失败: ' + err);
        }
    }

    async function duplicateSubtitleWindow() {
        if (!invoke) return;
        await saveCurrentWindowDraft();
        try {
            const id = await invoke('subtitle_duplicate_window', { windowId: state.currentWindowId || 'primary' });
            await reloadWindowsAfterBackendChange(id);
            addLog('info', '已复制字幕窗口', 'subtitle');
        } catch (err) {
            console.error('Failed to duplicate subtitle window:', err);
            alert('复制窗口失败: ' + err);
        }
    }

    async function removeSubtitleWindow() {
        if (!invoke || state.currentWindowId === 'primary') return;
        const { confirmed } = await showConfirmDialog(
            '删除窗口',
            '确定删除当前字幕窗口吗？此操作不可恢复。',
            '删除'
        );
        if (!confirmed) return;
        await saveCurrentWindowDraft();
        try {
            await invoke('subtitle_remove_window', { windowId: state.currentWindowId });
            await reloadWindowsAfterBackendChange('primary');
            addLog('info', '已删除字幕窗口', 'subtitle');
        } catch (err) {
            console.error('Failed to remove subtitle window:', err);
            alert('删除窗口失败: ' + err);
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
        if (Array.isArray(segments)) state.transcriptSegments = segments;
        const segs = state.transcriptSegments;
        const countEl = $('#transcript-status');
        if (countEl) countEl.textContent = segs.length ? `共 ${segs.length} 条` : '';
        if (!segs.length) {
            list.innerHTML = '<div class="transcript-empty">开启实时字幕后，定稿句段将显示在这里</div>';
            return;
        }
        const wasAtBottom = list.scrollHeight - list.scrollTop - list.clientHeight < 40;
        // 仅渲染最近 N 条，防止 DOM 节点无限增长导致卡顿
        const TRANSCRIPT_DOM_CAP = 400;
        const shown = segs.length > TRANSCRIPT_DOM_CAP ? segs.slice(-TRANSCRIPT_DOM_CAP) : segs;
        const omitted = segs.length - shown.length;
        let html = '';
        if (omitted > 0) {
            html += `<div class="transcript-item" style="color:var(--text-tertiary)">… 更早的 ${omitted} 条已省略（导出可保留完整记录）</div>`;
        }
        html += shown.map(seg => {
            const srcBadge = seg.source === 'B' ? '<span class="tr-source-b">麦克风</span>' : '';
            const speaker = seg.speaker ? `<span class="tr-speaker">${escapeHtml(seg.speaker)}:</span>` : '';
            const trans = seg.translation ? `<span class="tr-translation">译: ${escapeHtml(seg.translation)}</span>` : '';
            return `<div class="transcript-item"><span class="tr-time">[${formatTranscriptTime(seg.start_ms != null ? seg.start_ms : seg.startMs)}]</span>${srcBadge}${speaker}${escapeHtml(seg.text)}${trans}</div>`;
        }).join('');
        list.innerHTML = html;
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


    function updatePresetUI() {
        const theme = getCurrentTheme();
        const preset = theme ? theme.preset : 'custom';
        $$('#subtitle-preset .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn.dataset.preset === preset);
        });
        const activeBtn = $(`#subtitle-preset .seg-btn[data-preset="${preset}"]`);
        if (activeBtn) moveSegIndicator(activeBtn);
    }

    function markPresetCustom() {
        const theme = getCurrentTheme();
        if (theme && theme.preset !== 'custom') {
            theme.preset = 'custom';
            updatePresetUI();
        }
        // 主题/元素被手动修改 → 标记未保存（switch/seg 控件不触发 view 级 input/change）
        if (!state.populatingSettings) state.settingsDirty = true;
    }

    function applySubtitlePreset(preset) {
        const win = getCurrentSubtitleWindow();
        if (!win) return;
        if (!state.populatingSettings) state.settingsDirty = true;
        if (preset === 'custom') {
            win.theme.preset = 'custom';
            updatePresetUI();
            updateSubtitlePreview();
            return;
        }
        const setEnabled = (kind, on) => {
            const el = win.elements.find(e => e.kind === kind);
            if (el) el.enabled = on;
        };
        setEnabled('original', true);
        setEnabled('translation', preset === 'bilingual' || preset === 'live');
        setEnabled('speaker', preset === 'meeting');
        setEnabled('timestamp', preset === 'meeting' || preset === 'live');
        setEnabled('secondary', false);
        win.theme.layout = preset === 'live' ? 'horizontal' : 'vertical';
        if (preset === 'clean') {
            win.translation.engine = 'none';
        } else if (preset === 'bilingual' || preset === 'live') {
            if (win.translation.engine === 'none') win.translation.engine = 'llm';
        }
        win.theme.preset = preset;
        // 重新同步 UI（含预设/布局/引擎/元素开关）并刷新预览
        if (state.config) populateSubtitleUiFromWindow(win, state.config);
        updateSubtitlePreview();
    }

    // ===== 元素编辑器（统一列表：固定 + 自定义，顺序即显示顺序） =====
    function genCustomElementId() {
        return 'c_' + Date.now().toString(36) + Math.floor(Math.random() * 1000).toString(36);
    }

    function renderElementEditor() {
        const list = $('#subtitle-element-editor');
        const win = getCurrentSubtitleWindow();
        if (!list || !win) return;
        list.innerHTML = '';
        const els = win.elements || [];
        if (!els.length) {
            list.innerHTML = '<div class="custom-element-empty">暂无元素，点击下方按钮添加自定义元素</div>';
            return;
        }
        els.forEach((el, i) => {
            const isFixed = FIXED_ELEMENT_KINDS.includes(el.kind);
            const item = document.createElement('div');
            item.className = 'element-editor-item' + (isFixed ? ' is-fixed' : ' is-custom') +
                (state.selectedElementId === el.id ? ' selected' : '');
            item.dataset.id = el.id;
            item.dataset.kind = el.kind;

            const typeLabel = isFixed ? FIXED_ELEMENT_LABELS[el.kind] : elementTypeLabel(el.kind);
            const header = document.createElement('div');
            header.className = 'ee-header';
            header.innerHTML =
                `<div class="switch ee-enable" data-on="${el.enabled ? 'true' : 'false'}" title="启用/停用"><div class="switch-knob"></div></div>` +
                `<span class="ee-badge ee-badge-${el.kind}">${escapeHtml(typeLabel)}</span>` +
                `<span class="ee-label">${escapeHtml(el.label || typeLabel)}</span>` +
                `<div class="ee-move">` +
                `<button class="icon-btn ee-up" type="button" title="上移"${i === 0 ? ' disabled' : ''}>▲</button>` +
                `<button class="icon-btn ee-down" type="button" title="下移"${i === els.length - 1 ? ' disabled' : ''}>▼</button>` +
                `</div>`;
            item.appendChild(header);

            if (!isFixed) {
                const body = document.createElement('div');
                body.className = 'ee-body';
                body.innerHTML = elementEditorBodyHtml(el);
                item.appendChild(body);
            }
            list.appendChild(item);
        });
        bindElementEditorEvents();
        // 刷新动态渲染的滑块填充进度
        list.querySelectorAll('input[type="range"]').forEach(slider => {
            const min = parseFloat(slider.min) || 0;
            const max = parseFloat(slider.max) || 100;
            const val = parseFloat(slider.value);
            const pct = max > min ? ((val - min) / (max - min)) * 100 : 0;
            slider.style.setProperty('--fill', pct + '%');
        });
        updateDeleteElementButton();
    }

    function elementEditorBodyHtml(el) {
        const weightOpts = WEIGHT_OPTIONS.map(w => `<option value="${w}" ${el.fontWeight === w ? 'selected' : ''}>${w}</option>`).join('');
        if (el.kind === 'divider') {
            return `<div class="ee-style-row">` +
                `<label class="ee-mini-label">颜色</label>` +
                `<input type="color" class="color-input ee-field" data-field="color" value="${escapeHtml(el.color || '#ffffff')}" title="颜色">` +
                `<label class="ee-mini-label">透明度</label>` +
                `<input type="range" min="0" max="100" class="slider ee-field" data-field="opacity_pct" value="${Math.round((el.opacity != null ? el.opacity : 0.3) * 100)}" title="透明度">` +
                `</div>`;
        }
        if (el.kind === 'spacer') {
            return `<div class="ee-style-row">` +
                `<label class="ee-mini-label">高度 (px)</label>` +
                `<input type="range" min="4" max="96" class="slider ee-field" data-field="fontSize" value="${el.fontSize || 12}" title="高度">` +
                `</div>`;
        }
        return `<div class="ee-style-row ee-style-row-block">` +
            `<input type="text" class="text-input ee-field" data-field="content" placeholder="内容（支持 {time} {date} {datetime} {text} {translation} {speaker}）" value="${escapeHtml(el.content || '')}">` +
            `</div>` +
            `<div class="ee-style-row">` +
            `<label class="ee-mini-label">前缀</label>` +
            `<input type="text" class="text-input ee-field ee-prefix-input" data-field="prefix" placeholder="前缀" value="${escapeHtml(el.prefix || '')}">` +
            `<label class="ee-mini-label">颜色</label>` +
            `<input type="color" class="color-input ee-field" data-field="color" value="${escapeHtml(el.color || '#ffffff')}" title="颜色">` +
            `<label class="ee-mini-label">字号</label>` +
            `<input type="range" min="8" max="96" class="slider ee-field" data-field="fontSize" value="${el.fontSize || 18}" title="字号">` +
            `<label class="ee-mini-label">字重</label>` +
            `<select class="text-input ee-field" data-field="fontWeight">${weightOpts}</select>` +
            `<label class="ee-mini-label">透明度</label>` +
            `<input type="range" min="0" max="100" class="slider ee-field" data-field="opacity_pct" value="${Math.round((el.opacity != null ? el.opacity : 0.9) * 100)}" title="透明度">` +
            `<label class="ee-mini-label">对齐</label>` +
            `<select class="text-input ee-field" data-field="align">` +
            `<option value="left" ${el.align === 'left' ? 'selected' : ''}>左</option>` +
            `<option value="center" ${(el.align === 'center' || !el.align) ? 'selected' : ''}>中</option>` +
            `<option value="right" ${el.align === 'right' ? 'selected' : ''}>右</option>` +
            `</select>` +
            `</div>`;
    }

    function bindElementEditorEvents() {
        const list = $('#subtitle-element-editor');
        if (!list) return;
        list.querySelectorAll('.element-editor-item').forEach(item => {
            const id = item.dataset.id;
            const enableSw = item.querySelector('.ee-enable');
            if (enableSw) enableSw.addEventListener('click', () => toggleElementEnabled(id));
            const upBtn = item.querySelector('.ee-up');
            if (upBtn) upBtn.addEventListener('click', () => moveElement(id, -1));
            const downBtn = item.querySelector('.ee-down');
            if (downBtn) downBtn.addEventListener('click', () => moveElement(id, 1));

            // 自定义元素：点击头部选中（用于「删除所选自定义元素」）
            const header = item.querySelector('.ee-header');
            if (header && item.classList.contains('is-custom')) {
                header.addEventListener('click', (e) => {
                    if (e.target.closest('.ee-up, .ee-down, .ee-enable')) return;
                    selectElement(id);
                });
            }

            item.querySelectorAll('.ee-field').forEach(input => {
                const field = input.dataset.field;
                const evt = (input.tagName === 'SELECT') ? 'change' : 'input';
                input.addEventListener(evt, () => {
                    updateElementField(id, field, input);
                    updateSubtitlePreview();
                });
            });
        });
    }

    function findElement(id) {
        const win = getCurrentSubtitleWindow();
        if (!win) return null;
        return (win.elements || []).find(e => e.id === id) || null;
    }

    function toggleElementEnabled(id) {
        const el = findElement(id);
        if (!el) return;
        el.enabled = !el.enabled;
        renderElementEditor();
        markPresetCustom();
        updateSubtitlePreview();
    }

    function selectElement(id) {
        state.selectedElementId = (state.selectedElementId === id) ? null : id;
        const list = $('#subtitle-element-editor');
        if (list) {
            list.querySelectorAll('.element-editor-item').forEach(it =>
                it.classList.toggle('selected', it.dataset.id === state.selectedElementId));
        }
        updateDeleteElementButton();
    }

    function updateDeleteElementButton() {
        const btn = $('#btn-delete-element');
        if (!btn) return;
        const sel = state.selectedElementId ? findElement(state.selectedElementId) : null;
        const isCustom = sel && !FIXED_ELEMENT_KINDS.includes(sel.kind);
        btn.disabled = !isCustom;
    }

    function updateElementField(id, field, input) {
        const el = findElement(id);
        if (!el) return;
        let val;
        if (input.type === 'range' || input.type === 'number') {
            val = parseFloat(input.value);
        } else {
            val = input.value;
        }
        if (field === 'opacity_pct') {
            el.opacity = val / 100;
        } else if (field === 'fontSize' || field === 'fontWeight') {
            el[field] = parseInt(val) || 0;
        } else {
            el[field] = val;
        }
        markPresetCustom();
    }

    function addElement(kind) {
        const win = getCurrentSubtitleWindow();
        if (!win) return;
        const defaults = {
            text: { label: '自定义文本', content: '自定义文本', fontSize: 18, fontWeight: 400, opacity: 0.9, align: 'center' },
            divider: { label: '分隔线', opacity: 0.3, fontSize: 0 },
            spacer: { label: '间距', fontSize: 12, opacity: 1 }
        };
        const d = defaults[kind] || defaults.text;
        const el = {
            kind,
            id: genCustomElementId(),
            enabled: true,
            label: d.label,
            content: kind === 'text' ? d.content : '',
            prefix: '',
            color: '#ffffff',
            fontSize: d.fontSize || 18,
            fontWeight: d.fontWeight || 400,
            opacity: d.opacity != null ? d.opacity : 0.9,
            align: d.align || 'center'
        };
        win.elements.push(el);
        state.selectedElementId = el.id;
        renderElementEditor();
        markPresetCustom();
        updateSubtitlePreview();
    }

    function removeSelectedElement() {
        const win = getCurrentSubtitleWindow();
        if (!win || !state.selectedElementId) return;
        const el = findElement(state.selectedElementId);
        if (!el || FIXED_ELEMENT_KINDS.includes(el.kind)) return;
        win.elements = win.elements.filter(e => e.id !== state.selectedElementId);
        state.selectedElementId = null;
        renderElementEditor();
        markPresetCustom();
        updateSubtitlePreview();
    }

    function moveElement(id, dir) {
        const win = getCurrentSubtitleWindow();
        if (!win) return;
        const idx = win.elements.findIndex(e => e.id === id);
        if (idx < 0) return;
        const newIdx = idx + dir;
        if (newIdx < 0 || newIdx >= win.elements.length) return;
        const [item] = win.elements.splice(idx, 1);
        win.elements.splice(newIdx, 0, item);
        renderElementEditor();
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

    /// 组装 AppConfig.subtitle 新模型（全局字段 + 窗口数组，state 即实时模型）
    function collectSubtitleConfig() {
        // 窗口名称等实时字段写回 state
        flushCurrentWindowName();

        const sub = {};

        // 字幕开关热键（VK 码）
        const hotkeyInput = $('#subtitle-hotkey');
        if (hotkeyInput) {
            const vk = nameToVirtualKey(hotkeyInput.value);
            if (vk) sub.hotkey = vk;
        }

        // 音源类型
        const sourceBtn = $('#subtitle-audio-source .seg-btn.active');
        sub.audioSource = sourceBtn ? sourceBtn.dataset.source : 'microphone';

        // 音频输入设备
        const deviceSel = $('#setting-subtitle-input-device');
        sub.inputDevice = deviceSel ? deviceSel.value : '';

        // 同声传译 LLM 全局接口
        const llmUrl = $('#subtitle-llm-url');
        const llmKey = $('#subtitle-llm-key');
        const llmModel = $('#subtitle-llm-model');
        sub.translationLlm = {
            apiUrl: llmUrl ? llmUrl.value.trim() : '',
            apiKey: llmKey ? llmKey.value.trim() : '',
            model: llmModel ? llmModel.value.trim() : ''
        };

        // 窗口数组（state 即实时模型，深拷贝）
        sub.windows = (state.subtitleWindows || []).map(w => JSON.parse(JSON.stringify(w)));

        return sub;
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
                { id: 'setting-subtitle-input-device', current: state.config?.subtitle?.inputDevice || '' },
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
        // 浏览器模式兜底：生成默认窗口
        if (!state.subtitleWindows.length) {
            state.subtitleWindows = [defaultSubtitleWindow()];
            state.currentWindowId = 'primary';
        }
    }

    /// 填充字幕全局设置（音源/热键/设备/LLM 接口，所有窗口共享）
    function populateSubtitleGlobalUi(config) {
        const sub = (config && config.subtitle) || {};
        const subDeviceSel = $('#setting-subtitle-input-device');
        if (subDeviceSel) subDeviceSel.value = sub.inputDevice || '';

        const subHotkeyInput = $('#subtitle-hotkey');
        if (subHotkeyInput) subHotkeyInput.value = virtualKeyToName(sub.hotkey || 0x76);

        const audioSource = sub.audioSource || 'microphone';
        const sourceActiveBtn = $(`#subtitle-audio-source .seg-btn[data-source="${audioSource}"]`);
        $$('#subtitle-audio-source .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === sourceActiveBtn);
        });
        if (sourceActiveBtn) moveSegIndicator(sourceActiveBtn);
        updateSubtitleSourceUI(audioSource);

        const llm = sub.translationLlm || {};
        const llmUrl = $('#subtitle-llm-url');
        const llmKey = $('#subtitle-llm-key');
        const llmModel = $('#subtitle-llm-model');
        if (llmUrl) llmUrl.value = llm.apiUrl || '';
        if (llmKey) llmKey.value = llm.apiKey || '';
        if (llmModel) llmModel.value = llm.model || '';
    }

    /// 将当前窗口配置填充到字幕设置 UI（主题/元素/翻译/窗口控制）
    function populateSubtitleUiFromWindow(win, config) {
        if (!win) return;
        const theme = win.theme || defaultSubtitleTheme();
        const setVal = (id, value) => {
            const el = $(`#${id}`);
            if (el && value !== undefined && value !== null) el.value = value;
        };

        // 字体
        setVal('subtitle-font-family', theme.fontFamily);
        setVal('subtitle-font-size', theme.fontSize);
        setVal('subtitle-font-weight', theme.fontWeight);
        const italicSwitch = $('#subtitle-italic');
        if (italicSwitch) italicSwitch.dataset.on = theme.italic === true ? 'true' : 'false';

        // 文字
        setVal('subtitle-font-color', theme.fontColor);
        setVal('subtitle-text-shadow-color', theme.textShadowColor);
        setVal('subtitle-text-shadow-strength', theme.textShadowStrength);
        setVal('subtitle-lines', theme.maxLines);
        const align = theme.textAlign || 'center';
        const alignActiveBtn = $(`#subtitle-text-align .seg-btn[data-mode="${align}"]`);
        $$('#subtitle-text-align .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === alignActiveBtn);
        });
        if (alignActiveBtn) moveSegIndicator(alignActiveBtn);

        // 行高 / 字间距
        setVal('subtitle-line-height', theme.lineHeight);
        setVal('subtitle-letter-spacing', theme.letterSpacing);

        // 背景
        setVal('subtitle-bg-color', theme.bgColor);
        setVal('subtitle-opacity', Math.round((theme.bgOpacity != null ? theme.bgOpacity : 0.6) * 100));
        setVal('subtitle-blur', theme.blur);
        setVal('subtitle-padding-x', theme.paddingX);
        setVal('subtitle-padding-y', theme.paddingY);

        // 临时文字
        setVal('subtitle-interim-color', theme.interimColor);
        setVal('subtitle-interim-opacity', Math.round((theme.interimOpacity != null ? theme.interimOpacity : 0.7) * 100));

        // 布局
        const layout = theme.layout || 'vertical';
        const layoutActiveBtn = $(`#subtitle-layout .seg-btn[data-mode="${layout}"]`);
        $$('#subtitle-layout .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === layoutActiveBtn);
        });
        if (layoutActiveBtn) moveSegIndicator(layoutActiveBtn);

        const anchorX = theme.anchorX || 'center';
        const axBtn = $(`#subtitle-anchor-x .seg-btn[data-mode="${anchorX}"]`);
        $$('#subtitle-anchor-x .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === axBtn);
        });
        if (axBtn) moveSegIndicator(axBtn);

        const anchorY = theme.anchorY || 'bottom';
        const ayBtn = $(`#subtitle-anchor-y .seg-btn[data-mode="${anchorY}"]`);
        $$('#subtitle-anchor-y .seg-btn').forEach(btn => {
            btn.classList.toggle('active', btn === ayBtn);
        });
        if (ayBtn) moveSegIndicator(ayBtn);

        setVal('subtitle-max-width', theme.maxWidthPct != null ? theme.maxWidthPct : 100);

        // 译文样式
        const tr = theme.translation || {};
        setVal('subtitle-translation-color', tr.color);
        setVal('subtitle-translation-size', tr.size);
        setVal('subtitle-translation-weight', tr.weight);
        setVal('subtitle-translation-opacity', Math.round((tr.opacity != null ? tr.opacity : 0.85) * 100));
        setVal('subtitle-translation-prefix', tr.prefix);

        // 说话人样式
        const spk = theme.speaker || {};
        setVal('subtitle-speaker-color', spk.color);
        setVal('subtitle-speaker-size', spk.size);
        setVal('subtitle-speaker-prefix', spk.prefix);

        // 时间戳样式
        const ts = theme.timestamp || {};
        setVal('subtitle-timestamp-color', ts.color);
        setVal('subtitle-timestamp-size', ts.size);
        setVal('subtitle-timestamp-format', ts.format);

        // 预设模板
        updatePresetUI();

        // 同声传译（当前窗口）
        const trans = win.translation || {};
        setVal('subtitle-translation-engine', trans.engine || 'none');
        setVal('subtitle-translation-lang', trans.targetLang || '英文');
        const interimSw = $('#subtitle-translation-interim');
        if (interimSw) interimSw.dataset.on = trans.interim !== false ? 'true' : 'false';

        // 窗口控制
        const onTopSw = $('#subtitle-always-on-top');
        if (onTopSw) onTopSw.dataset.on = win.alwaysOnTop !== false ? 'true' : 'false';
        const clickSw = $('#subtitle-click-through');
        if (clickSw) clickSw.dataset.on = win.clickThrough === true ? 'true' : 'false';
        const obsSw = $('#subtitle-obs-mode');
        if (obsSw) obsSw.dataset.on = win.obsMode === true ? 'true' : 'false';
        const autoFitSw = $('#subtitle-auto-fit');
        if (autoFitSw) autoFitSw.dataset.on = win.autoFit !== false ? 'true' : 'false';

        // 窗口名称
        const nameInput = $('#subtitle-window-name');
        if (nameInput) nameInput.value = win.name || '';

        // 元素编辑器
        renderElementEditor();
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
            loadWindowsIntoState(config);
            populateSubtitleGlobalUi(config);
            const win = getCurrentSubtitleWindow();
            populateSubtitleUiFromWindow(win, config);
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

        // 字幕配置：新模型（全局字段 + 窗口数组），与既有 subtitle 合并以保留未编辑字段
        newConfig.subtitle = Object.assign({}, newConfig.subtitle, collectSubtitleConfig());

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
            // 把最新配置应用到所有已存在的字幕窗口并让它们重拉主题
            try {
                await invoke('subtitle_push_theme');
            } catch (e) {
                console.warn('Failed to push subtitle theme:', e);
            }
            // 重新注册字幕开关全局热键（吞键，避免浏览器快捷键干扰）
            try {
                await invoke('apply_subtitle_hotkey');
            } catch (e) {
                console.warn('Failed to apply subtitle hotkey:', e);
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
        if (toggleBtn) toggleBtn.addEventListener('click', toggleSubtitle);

        // 主题分段控件：点击 → 写回当前窗口 theme → 刷新预览
        function bindThemeSeg(segId, key) {
            $$(`#${segId} .seg-btn`).forEach(btn => {
                btn.addEventListener('click', () => {
                    $$(`#${segId} .seg-btn`).forEach(b => b.classList.remove('active'));
                    btn.classList.add('active');
                    const theme = getCurrentTheme();
                    if (theme) theme[key] = btn.dataset.mode;
                    markPresetCustom();
                    updateSubtitlePreview();
                    moveSegIndicator(btn);
                });
            });
        }

        // 子样式控件（theme.translation/speaker/timestamp 分组字段）
        function bindNestedStyle(id, group, key, parse, display) {
            const el = $(`#${id}`);
            if (!el) return;
            const apply = () => {
                const theme = getCurrentTheme();
                if (!theme) return;
                if (!theme[group]) theme[group] = {};
                theme[group][key] = parse ? parse(el.value) : el.value;
                if (display) updateValueDisplay(id, display(el.value));
                markPresetCustom();
                updateSubtitlePreview();
            };
            el.addEventListener('input', apply);
            el.addEventListener('change', apply);
        }

        // 数值型主题控件
        [
            { id: 'subtitle-font-size', key: 'fontSize', display: v => `${v}px` },
            { id: 'subtitle-letter-spacing', key: 'letterSpacing', display: v => `${v}px` },
            { id: 'subtitle-line-height', key: 'lineHeight', display: v => parseFloat(v).toFixed(1) },
            { id: 'subtitle-text-shadow-strength', key: 'textShadowStrength', display: v => `${v}` },
            { id: 'subtitle-blur', key: 'blur', display: v => `${v}px` },
            { id: 'subtitle-padding-x', key: 'paddingX', display: v => `${v}px` },
            { id: 'subtitle-padding-y', key: 'paddingY', display: v => `${v}px` },
            { id: 'subtitle-lines', key: 'maxLines', display: v => `${v} 行` },
            { id: 'subtitle-max-width', key: 'maxWidthPct', display: v => `${v}%` }
        ].forEach(({ id, key, display }) => {
            const el = $(`#${id}`);
            if (!el) return;
            const apply = () => {
                const theme = getCurrentTheme();
                if (!theme) return;
                theme[key] = parseFloat(el.value);
                if (display) updateValueDisplay(id, display(el.value));
                markPresetCustom();
                updateSubtitlePreview();
            };
            el.addEventListener('input', apply);
            el.addEventListener('change', apply);
        });

        // 百分比型主题控件（0-100 → 0-1）
        [
            { id: 'subtitle-opacity', key: 'bgOpacity' },
            { id: 'subtitle-interim-opacity', key: 'interimOpacity' }
        ].forEach(({ id, key }) => {
            const el = $(`#${id}`);
            if (!el) return;
            const apply = () => {
                const theme = getCurrentTheme();
                if (!theme) return;
                theme[key] = parseInt(el.value) / 100;
                updateValueDisplay(id, `${el.value}%`);
                markPresetCustom();
                updateSubtitlePreview();
            };
            el.addEventListener('input', apply);
            el.addEventListener('change', apply);
        });

        // 字符串/整数型主题控件
        [
            { id: 'subtitle-font-family', key: 'fontFamily' },
            { id: 'subtitle-font-weight', key: 'fontWeight', parse: v => parseInt(v) || 400 },
            { id: 'subtitle-font-color', key: 'fontColor' },
            { id: 'subtitle-text-shadow-color', key: 'textShadowColor' },
            { id: 'subtitle-bg-color', key: 'bgColor' },
            { id: 'subtitle-interim-color', key: 'interimColor' }
        ].forEach(({ id, key, parse }) => {
            const el = $(`#${id}`);
            if (!el) return;
            const apply = () => {
                const theme = getCurrentTheme();
                if (!theme) return;
                theme[key] = parse ? parse(el.value) : el.value;
                markPresetCustom();
                updateSubtitlePreview();
            };
            el.addEventListener('input', apply);
            el.addEventListener('change', apply);
        });

        // 斜体开关
        const italicSw = $('#subtitle-italic');
        if (italicSw) {
            italicSw.addEventListener('click', () => {
                italicSw.dataset.on = italicSw.dataset.on === 'true' ? 'false' : 'true';
                const theme = getCurrentTheme();
                if (theme) theme.italic = italicSw.dataset.on === 'true';
                markPresetCustom();
                updateSubtitlePreview();
            });
        }

        // 译文 / 说话人 / 时间戳 子样式
        bindNestedStyle('subtitle-translation-color', 'translation', 'color');
        bindNestedStyle('subtitle-translation-size', 'translation', 'size', v => parseInt(v) || 0, v => `${v}px`);
        bindNestedStyle('subtitle-translation-weight', 'translation', 'weight', v => parseInt(v) || 400);
        bindNestedStyle('subtitle-translation-opacity', 'translation', 'opacity', v => parseInt(v) / 100, v => `${v}%`);
        bindNestedStyle('subtitle-translation-prefix', 'translation', 'prefix');
        bindNestedStyle('subtitle-speaker-color', 'speaker', 'color');
        bindNestedStyle('subtitle-speaker-size', 'speaker', 'size', v => parseInt(v) || 0, v => `${v}px`);
        bindNestedStyle('subtitle-speaker-prefix', 'speaker', 'prefix');
        bindNestedStyle('subtitle-timestamp-color', 'timestamp', 'color');
        bindNestedStyle('subtitle-timestamp-size', 'timestamp', 'size', v => parseInt(v) || 0, v => `${v}px`);
        bindNestedStyle('subtitle-timestamp-format', 'timestamp', 'format');

        // 对齐 / 布局 / 锚点 segmented controls
        bindThemeSeg('subtitle-text-align', 'textAlign');
        bindThemeSeg('subtitle-layout', 'layout');
        bindThemeSeg('subtitle-anchor-x', 'anchorX');
        bindThemeSeg('subtitle-anchor-y', 'anchorY');

        // 音源类型 segmented control
        $$('#subtitle-audio-source .seg-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                $$('#subtitle-audio-source .seg-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                updateSubtitleSourceUI(btn.dataset.source);
                // 切到同传模式时自动启用「副原文（麦克风）」元素，
                // 保证麦克风副字幕立即可见（用户仍可在元素编辑器中关闭）
                if (btn.dataset.source === 'dual') {
                    const win = getCurrentSubtitleWindow();
                    if (win) {
                        const secondary = win.elements.find(e => e.kind === 'secondary');
                        if (secondary && !secondary.enabled) {
                            secondary.enabled = true;
                            renderElementEditor();
                            updateSubtitlePreview();
                        }
                    }
                }
                if (!state.populatingSettings) state.settingsDirty = true;
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

        // ===== 自定义元素添加 / 删除按钮 =====
        [
            { id: 'btn-add-element-text', kind: 'text' },
            { id: 'btn-add-element-divider', kind: 'divider' },
            { id: 'btn-add-element-spacer', kind: 'spacer' }
        ].forEach(({ id, kind }) => {
            const btn = $(`#${id}`);
            if (btn) btn.addEventListener('click', () => addElement(kind));
        });
        const delElementBtn = $('#btn-delete-element');
        if (delElementBtn) delElementBtn.addEventListener('click', removeSelectedElement);

        // ===== 应用设置按钮：保存新模型 + 应用热键 =====
        const applyBtn = $('#btn-apply-subtitle');
        if (applyBtn) {
            applyBtn.addEventListener('click', async () => {
                applyBtn.disabled = true;
                applyBtn.textContent = '应用中...';
                try {
                    await saveSettings();
                } finally {
                    applyBtn.disabled = false;
                    applyBtn.textContent = '应用设置';
                }
            });
        }

        // ===== 窗口控制开关（置顶/穿透/OBS 额外调用 set_window_flag，auto_fit 只走保存） =====
        const alwaysOnTopSw = $('#subtitle-always-on-top');
        if (alwaysOnTopSw) {
            alwaysOnTopSw.addEventListener('click', async () => {
                const on = alwaysOnTopSw.dataset.on !== 'true';
                alwaysOnTopSw.dataset.on = on ? 'true' : 'false';
                const win = getCurrentSubtitleWindow();
                if (win) win.alwaysOnTop = on;
                if (!state.populatingSettings) state.settingsDirty = true;
                if (invoke) {
                    try {
                        await invoke('subtitle_set_window_flag', { windowId: state.currentWindowId, flag: 'always_on_top', value: on });
                    } catch (err) {
                        console.error('Failed to set always on top:', err);
                    }
                }
            });
        }

        const clickThroughSw = $('#subtitle-click-through');
        if (clickThroughSw) {
            clickThroughSw.addEventListener('click', async () => {
                const on = clickThroughSw.dataset.on !== 'true';
                clickThroughSw.dataset.on = on ? 'true' : 'false';
                const win = getCurrentSubtitleWindow();
                if (win) win.clickThrough = on;
                if (!state.populatingSettings) state.settingsDirty = true;
                if (invoke) {
                    try {
                        await invoke('subtitle_set_window_flag', { windowId: state.currentWindowId, flag: 'click_through', value: on });
                    } catch (err) {
                        console.error('Failed to set click through:', err);
                    }
                }
            });
        }

        const obsModeSw = $('#subtitle-obs-mode');
        if (obsModeSw) {
            obsModeSw.addEventListener('click', async () => {
                const on = obsModeSw.dataset.on !== 'true';
                obsModeSw.dataset.on = on ? 'true' : 'false';
                const win = getCurrentSubtitleWindow();
                if (win) win.obsMode = on;
                if (!state.populatingSettings) state.settingsDirty = true;
                if (invoke) {
                    try {
                        await invoke('subtitle_set_window_flag', { windowId: state.currentWindowId, flag: 'obs_mode', value: on });
                        setStatus('ready', 'OBS 兼容模式已切换（需重启应用生效）');
                        setTimeout(() => setStatus('idle', '就绪'), 4000);
                    } catch (err) {
                        console.error('Failed to set OBS mode:', err);
                    }
                }
            });
        }

        const autoFitSw = $('#subtitle-auto-fit');
        if (autoFitSw) {
            autoFitSw.addEventListener('click', () => {
                const on = autoFitSw.dataset.on !== 'true';
                autoFitSw.dataset.on = on ? 'true' : 'false';
                const win = getCurrentSubtitleWindow();
                if (win) win.autoFit = on;
                if (!state.populatingSettings) state.settingsDirty = true;
            });
        }

        // 显示/隐藏字幕窗口（当前窗口）
        const showBtn = $('#btn-show-subtitle-window');
        if (showBtn) {
            showBtn.addEventListener('click', async () => {
                if (!invoke) return;
                try {
                    await invoke('subtitle_show_window', { windowId: state.currentWindowId, show: true });
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
                    await invoke('subtitle_show_window', { windowId: state.currentWindowId, show: false });
                } catch (err) {
                    console.error('Failed to hide subtitle window:', err);
                }
            });
        }

        // ===== 窗口管理 =====
        const windowSelect = $('#subtitle-window-select');
        if (windowSelect) windowSelect.addEventListener('change', () => switchSubtitleWindow(windowSelect.value));

        const windowNameInput = $('#subtitle-window-name');
        if (windowNameInput) {
            windowNameInput.addEventListener('input', () => {
                const win = getCurrentSubtitleWindow();
                if (win) {
                    const name = windowNameInput.value.trim();
                    if (name) win.name = name;
                }
            });
        }

        const addWindowBtn = $('#btn-add-subtitle-window');
        if (addWindowBtn) addWindowBtn.addEventListener('click', addSubtitleWindow);
        const dupWindowBtn = $('#btn-duplicate-subtitle-window');
        if (dupWindowBtn) dupWindowBtn.addEventListener('click', duplicateSubtitleWindow);
        const rmWindowBtn = $('#btn-remove-subtitle-window');
        if (rmWindowBtn) rmWindowBtn.addEventListener('click', removeSubtitleWindow);

        // ===== 同声传译设置（当前窗口） =====
        const transEngineSel = $('#subtitle-translation-engine');
        if (transEngineSel) {
            transEngineSel.addEventListener('change', () => {
                const win = getCurrentSubtitleWindow();
                if (win) win.translation.engine = transEngineSel.value || 'none';
                updateTranslationUI();
                markPresetCustom();
                updateSubtitlePreview();
            });
        }
        const transLangSel = $('#subtitle-translation-lang');
        if (transLangSel) {
            transLangSel.addEventListener('change', () => {
                const win = getCurrentSubtitleWindow();
                if (win) win.translation.targetLang = transLangSel.value || '英文';
                if (!state.populatingSettings) state.settingsDirty = true;
            });
        }
        const transInterimSw = $('#subtitle-translation-interim');
        if (transInterimSw) {
            transInterimSw.addEventListener('click', () => {
                transInterimSw.dataset.on = transInterimSw.dataset.on === 'true' ? 'false' : 'true';
                const win = getCurrentSubtitleWindow();
                if (win) win.translation.interim = transInterimSw.dataset.on === 'true';
                if (!state.populatingSettings) state.settingsDirty = true;
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

        // 转录更新（增量：append=新增句段，update=译文落地）
        listen('subtitle-transcript-updated', (event) => {
            const p = event.payload || {};
            if (p.type === 'append' && Array.isArray(p.segments)) {
                state.transcriptSegments.push(...p.segments);
                renderTranscript();
            } else if (p.type === 'update' && Array.isArray(p.updates)) {
                p.updates.forEach(u => {
                    const idx = Array.isArray(u) ? u[0] : u.index;
                    const text = Array.isArray(u) ? u[1] : u.translation;
                    const seg = state.transcriptSegments.find(x => x.index === idx);
                    if (seg) seg.translation = text || '';
                });
                renderTranscript();
            }
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
