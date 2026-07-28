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
        config: null,
        subtitleSettings: {
            fontSize: 32,
            opacity: 60,
            blur: 20,
            lines: 3
        },
        unlisteners: [],
        isMouseDown: false
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

    function switchView(viewName) {
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
            loadDownloadedModels();
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

    function updateSubtitlePreview() {
        const preview = $('.subtitle-preview');
        const previewText = $('#subtitle-preview-text');
        if (!preview || !previewText) return;

        // 收集当前所有控件值
        const cfg = collectSubtitleSettings();
        if (!cfg) return;

        // 应用样式到预览区域
        const el = previewText.style;
        el.fontFamily = `"${cfg.subtitle_font_family}", sans-serif`;
        el.fontSize = cfg.subtitle_font_size + 'px';
        el.fontWeight = cfg.subtitle_bold ? '700' : String(cfg.subtitle_font_weight);
        el.fontStyle = cfg.subtitle_italic ? 'italic' : 'normal';
        el.color = cfg.subtitle_font_color;
        el.textAlign = cfg.subtitle_text_align;
        el.letterSpacing = cfg.subtitle_letter_spacing + 'px';
        el.lineHeight = String(cfg.subtitle_line_height);
        el.padding = `${cfg.subtitle_padding_y}px ${cfg.subtitle_padding_x}px`;
        el.borderRadius = cfg.subtitle_border_radius + 'px';

        // 背景
        const bgOpacity = cfg.subtitle_bg_opacity;
        el.background = hexToRgba(cfg.subtitle_bg_color, bgOpacity);

        // 模糊
        el.backdropFilter = `blur(${cfg.subtitle_blur}px)`;
        el.webkitBackdropFilter = `blur(${cfg.subtitle_blur}px)`;

        // 边框
        el.borderWidth = cfg.subtitle_border_width + 'px';
        el.borderStyle = cfg.subtitle_border_width > 0 ? 'solid' : 'none';
        el.borderColor = cfg.subtitle_border_color;

        // 文字阴影
        el.textShadow = buildTextShadow(cfg.subtitle_text_shadow_color, cfg.subtitle_text_shadow_strength);

        // 行数限制（CSS -webkit-line-clamp）
        preview.style.WebkitLineClamp = cfg.subtitle_max_lines;

        // 更新所有 value-display
        updateValueDisplay('subtitle-font-size', `${cfg.subtitle_font_size}px`);
        updateValueDisplay('subtitle-opacity', `${Math.round(cfg.subtitle_bg_opacity * 100)}%`);
        updateValueDisplay('subtitle-blur', `${cfg.subtitle_blur}px`);
        updateValueDisplay('subtitle-lines', `${cfg.subtitle_max_lines} 行`);
        updateValueDisplay('subtitle-line-height', cfg.subtitle_line_height.toFixed(1));
        updateValueDisplay('subtitle-letter-spacing', `${cfg.subtitle_letter_spacing}px`);
        updateValueDisplay('subtitle-text-shadow-strength', String(cfg.subtitle_text_shadow_strength));
        updateValueDisplay('subtitle-border-radius', `${cfg.subtitle_border_radius}px`);
        updateValueDisplay('subtitle-border-width', `${cfg.subtitle_border_width}px`);
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
        cfg.subtitle_border_radius = parseInt(($('#subtitle-border-radius') || {}).value || 12);
        cfg.subtitle_border_color = ($('#subtitle-border-color') || {}).value || '#ffffff';
        cfg.subtitle_border_width = parseInt(($('#subtitle-border-width') || {}).value || 0);
        cfg.subtitle_padding_x = parseInt(($('#subtitle-padding-x') || {}).value || 24);
        cfg.subtitle_padding_y = parseInt(($('#subtitle-padding-y') || {}).value || 12);
        cfg.subtitle_max_lines = parseInt(($('#subtitle-lines') || {}).value || 3);
        cfg.subtitle_interim_color = ($('#subtitle-interim-color') || {}).value || '#ffffff';
        cfg.subtitle_interim_opacity = parseInt(($('#subtitle-interim-opacity') || {}).value || 70) / 100;
        return cfg;
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
        } catch (err) {
            console.error('Failed to load config:', err);
            // 初始化默认配置，确保用户操作（如切换模式）能被保存
            state.config = { basic: { dictation_mode: state.dictationMode } };
            setupDefaultSettings();
        }

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

    async function loadDownloadedModels() {
        if (!invoke) return;

        const listEl = $('#downloaded-models-list');
        if (!listEl) return;

        try {
            const models = await invoke('list_available_models');
            if (!models || models.length === 0) {
                listEl.innerHTML = '<span class="empty-hint">暂无已下载模型</span>';
                return;
            }

            listEl.innerHTML = models.map(m => {
                const sizeMB = (m.size / (1024 * 1024)).toFixed(1);
                return `<div class="model-item"><span class="model-name">${m.name}</span><span class="model-size">${sizeMB} MB</span></div>`;
            }).join('');
        } catch (err) {
            console.error('Failed to list models:', err);
            listEl.innerHTML = '<span class="empty-hint">加载失败</span>';
        }
    }

    async function downloadModel() {
        if (!invoke) return;

        const selectEl = $('#whisper-model-select');
        const btn = $('#btn-download-model');
        const progressRow = $('#download-progress-row');
        const progressBar = $('#download-progress-bar');
        const progressText = $('#download-progress-text');

        if (!selectEl || !btn) return;

        const modelName = selectEl.value;

        btn.disabled = true;
        btn.textContent = '下载中...';
        if (progressRow) progressRow.style.display = 'flex';
        if (progressBar) progressBar.style.width = '0%';
        if (progressText) progressText.textContent = '0%';

        try {
            await invoke('download_whisper_model', { model_name: modelName });
            if (progressBar) progressBar.style.width = '100%';
            if (progressText) progressText.textContent = '完成!';
            btn.textContent = '下载完成';
            setTimeout(() => {
                btn.disabled = false;
                btn.textContent = '下载';
                if (progressRow) progressRow.style.display = 'none';
                loadDownloadedModels();
            }, 1500);
        } catch (err) {
            console.error('Download failed:', err);
            btn.disabled = false;
            btn.textContent = '下载失败';
            if (progressText) progressText.textContent = '失败';
            setTimeout(() => {
                btn.textContent = '下载';
                if (progressRow) progressRow.style.display = 'none';
            }, 2000);
        }
    }

    function setupDefaultSettings() {
        const dirEl = $('#models-dir-path');
        if (dirEl) dirEl.textContent = '运行时加载';
    }

    function populateSettings(config) {
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
        }

        if (config.basic) {
            const hotkeyInput = $('#setting-hotkey-batch');
            const outputMode = $('#setting-output-mode');
            const outputLang = $('#setting-output-lang');
            const modelSelect = $('#whisper-model-select');
            if (hotkeyInput) hotkeyInput.value = virtualKeyToName(config.basic.hotkey || 0x71);
            if (outputMode) outputMode.value = config.basic.output_mode || 'clipboard';
            if (outputLang) outputLang.value = config.basic.output_language || 'auto';
            if (modelSelect && config.model && config.model.local_whisper_model) {
                const match = config.model.local_whisper_model.match(/ggml-(tiny|base|small|medium)\.bin/);
                if (match) modelSelect.value = match[1];
            }

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
            const setVal = (id, value) => {
                const el = $(`#${id}`);
                if (el && value !== undefined && value !== null) el.value = value;
            };
            setVal('subtitle-font-family', config.subtitle.subtitle_font_family);
            setVal('subtitle-font-size', config.subtitle.subtitle_font_size);
            setVal('subtitle-font-weight', config.subtitle.subtitle_font_weight);
            setVal('subtitle-font-color', config.subtitle.subtitle_font_color);
            setVal('subtitle-letter-spacing', config.subtitle.subtitle_letter_spacing);
            setVal('subtitle-line-height', config.subtitle.subtitle_line_height);
            setVal('subtitle-text-shadow-color', config.subtitle.subtitle_text_shadow_color);
            setVal('subtitle-text-shadow-strength', config.subtitle.subtitle_text_shadow_strength);
            setVal('subtitle-bg-color', config.subtitle.subtitle_bg_color);
            setVal('subtitle-opacity', Math.round((config.subtitle.subtitle_bg_opacity ?? 0.6) * 100));
            setVal('subtitle-blur', config.subtitle.subtitle_blur);
            setVal('subtitle-border-radius', config.subtitle.subtitle_border_radius);
            setVal('subtitle-border-color', config.subtitle.subtitle_border_color);
            setVal('subtitle-border-width', config.subtitle.subtitle_border_width);
            setVal('subtitle-padding-x', config.subtitle.subtitle_padding_x);
            setVal('subtitle-padding-y', config.subtitle.subtitle_padding_y);
            setVal('subtitle-lines', config.subtitle.subtitle_max_lines);
            setVal('subtitle-interim-color', config.subtitle.subtitle_interim_color);
            setVal('subtitle-interim-opacity', Math.round((config.subtitle.subtitle_interim_opacity ?? 0.7) * 100));

            // 开关：加粗 / 斜体
            const boldSwitch = $('#subtitle-bold');
            if (boldSwitch) boldSwitch.dataset.on = config.subtitle.subtitle_italic === true ? 'true' : 'false';
            // 注：后端无单独 bold 字段，bold 通过 font_weight=700 体现
            const isBold = (config.subtitle.subtitle_font_weight || 400) >= 700;
            if (boldSwitch) boldSwitch.dataset.on = isBold ? 'true' : 'false';
            const italicSwitch = $('#subtitle-italic');
            if (italicSwitch) italicSwitch.dataset.on = config.subtitle.subtitle_italic === true ? 'true' : 'false';

            // 对齐
            const align = config.subtitle.subtitle_text_align || 'center';
            state.subtitleAlign = align;
            const alignActiveBtn = $(`#subtitle-text-align .seg-btn[data-mode="${align}"]`);
            $$('#subtitle-text-align .seg-btn').forEach(btn => {
                btn.classList.toggle('active', btn === alignActiveBtn);
            });
            if (alignActiveBtn) moveSegIndicator(alignActiveBtn);

            updateSubtitlePreview();
        }

        if (config.features) {
            const punctuation = $('#setting-punctuation');
            const emoji = $('#setting-emoji');
            const indicator = $('#setting-indicator');
            if (punctuation) punctuation.checked = config.features.allow_punctuation !== false;
            if (emoji) emoji.checked = config.features.allow_emoji !== false;
            if (indicator) indicator.checked = config.features.enable_indicator !== false;
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

        updateModelBadge();
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

        const modelSelect = $('#whisper-model-select');
        if (modelSelect) {
            newConfig.model.local_whisper_model = `ggml-${modelSelect.value}.bin`;
        }

        const outputMode = $('#setting-output-mode');
        const outputLang = $('#setting-output-lang');
        if (outputMode) newConfig.basic.output_mode = outputMode.value;
        if (outputLang) newConfig.basic.output_language = outputLang.value;

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
        if (punctuation) newConfig.features.allow_punctuation = punctuation.checked;
        if (emoji) newConfig.features.allow_emoji = emoji.checked;
        if (indicator) newConfig.features.enable_indicator = indicator.checked;

        newConfig.advanced.trigger_mode = state.triggerMode;
        newConfig.basic.dictation_mode = state.dictationMode;

        const sensSlider = $('#setting-vad-sensitivity');
        const silenceSlider = $('#setting-vad-silence');
        if (sensSlider) newConfig.vad.vad_sensitivity = parseInt(sensSlider.value) / 100;
        if (silenceSlider) newConfig.vad.vad_silence_duration_ms = parseInt(silenceSlider.value);

        // 字幕配置：从所有新控件收集
        const sub = collectSubtitleSettings();
        if (sub) {
            Object.assign(newConfig.subtitle, sub);
            // bold=true 时强制 700，否则保留 font_weight
            if (sub.subtitle_bold && newConfig.subtitle.subtitle_font_weight < 700) {
                newConfig.subtitle.subtitle_font_weight = 700;
            } else if (!sub.subtitle_bold && newConfig.subtitle.subtitle_font_weight >= 700) {
                newConfig.subtitle.subtitle_font_weight = 400;
            }
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
            return;
        }

        try {
            await invoke('save_config', { newConfig: newConfig });
            state.config = newConfig;
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

    function initWindowControls() {
        const closeBtn = $('#btn-close');
        const minimizeBtn = $('#btn-minimize');

        if (closeBtn) {
            closeBtn.addEventListener('click', async () => {
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
            item.addEventListener('click', () => {
                const view = item.dataset.view;
                if (view) {
                    switchView(view);
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
            'subtitle-border-radius', 'subtitle-border-color', 'subtitle-border-width',
            'subtitle-padding-x', 'subtitle-padding-y', 'subtitle-lines',
            'subtitle-interim-color', 'subtitle-interim-opacity'
        ];
        previewIds.forEach(id => {
            const el = $(`#${id}`);
            if (el) {
                el.addEventListener('input', updateSubtitlePreview);
                el.addEventListener('change', updateSubtitlePreview);
            }
        });

        // 开关：加粗 / 斜体
        ['subtitle-bold', 'subtitle-italic'].forEach(id => {
            const sw = $(`#${id}`);
            if (sw) {
                sw.addEventListener('click', () => {
                    sw.dataset.on = sw.dataset.on === 'true' ? 'false' : 'true';
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

        // 字幕窗口控制开关
        const alwaysOnTopSw = $('#subtitle-always-on-top');
        if (alwaysOnTopSw) {
            alwaysOnTopSw.addEventListener('click', async () => {
                const on = alwaysOnTopSw.dataset.on === 'true';
                alwaysOnTopSw.dataset.on = on ? 'false' : 'true';
                if (invoke) {
                    try {
                        await invoke('set_subtitle_always_on_top', { onTop: !on });
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
                if (invoke) {
                    try {
                        await invoke('set_subtitle_click_through', { clickThrough: !on });
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
                if (invoke) {
                    try {
                        await invoke('set_subtitle_obs_mode', { obsMode: !on });
                    } catch (err) {
                        console.error('Failed to set OBS mode:', err);
                    }
                }
            });
        }

        // 显示/隐藏字幕窗口
        const showBtn = $('#btn-show-subtitle-window');
        if (showBtn) {
            showBtn.addEventListener('click', async () => {
                if (!invoke) return;
                try {
                    await invoke('show_subtitle_window', { show: true });
                    // 同步推送当前配置
                    await invoke('push_subtitle_config');
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
                    await invoke('show_subtitle_window', { show: false });
                } catch (err) {
                    console.error('Failed to hide subtitle window:', err);
                }
            });
        }
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

        const downloadBtn = $('#btn-download-model');
        if (downloadBtn) {
            downloadBtn.addEventListener('click', downloadModel);
        }

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
                // 重新加载已下载模型列表（新目录可能已有模型）
                await loadDownloadedModels();
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
            // 重新加载已下载模型列表
            await loadDownloadedModels();
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
    }

    const logs = [];
    let currentLogFilter = 'all';
    // 防止渲染风暴：批量更新时只渲染一次
    let renderScheduled = false;

    function addLog(level, message, source) {
        const now = new Date();
        const dateStr = `${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
        const timeStr = `${dateStr} ${now.toLocaleTimeString('zh-CN', { hour12: false })}.${String(now.getMilliseconds()).padStart(3, '0')}`;
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

    function escapeHtml(text) {
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
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
            loadSettings();
            updateSubtitlePreview();

            if (invoke) {
                invoke('is_subtitle_running').then(running => {
                    state.isSubtitleActive = running;
                    updateSubtitleButton();
                }).catch(() => {});
            }
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        listen('model-download-progress', (event) => {
            const data = event.payload;
            const progressBar = $('#download-progress-bar');
            const progressText = $('#download-progress-text');
            if (progressBar && data && data.progress !== undefined) {
                progressBar.style.width = `${data.progress}%`;
            }
            if (progressText && data && data.progress !== undefined) {
                progressText.textContent = `${data.progress}%`;
            }
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });

        listen('subtitle-text', () => {
        }).then(unlisten => {
            state.unlisteners.push(unlisten);
        });
    }

    function init() {
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
