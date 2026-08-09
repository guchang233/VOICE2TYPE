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
            // 模型和引擎检测改为手动触发
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

            overlay.innerHTML = `
                <div class="modal-content confirm-modal">
                    <div class="modal-header">
                        <h2>${title}</h2>
                    </div>
                    <div class="modal-body">
                        <p class="confirm-message">${message.replace(/\n/g, '<br>')}</p>
                    </div>
                    <div class="modal-footer">
                        ${selectHtml}
                        <button class="secondary-btn" data-action="cancel">取消</button>
                        <button class="solid-btn" data-action="confirm">${confirmText || '确认'}</button>
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

        // 恢复折叠状态
        try {
            const store = JSON.parse(localStorage.getItem('v2t-collapsed-sections') || '{}');
            $$('.settings-section').forEach(section => {
                const title = (section.querySelector('h3') || {}).textContent || section.id || 'section';
                if (store[title]) section.classList.add('collapsed');
            });
        } catch (e) {}
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
