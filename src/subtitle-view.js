/* =====================================================================
   实时字幕页（v4）
   ---------------------------------------------------------------------
   - 舞台：iframe 嵌入真实渲染器 subtitle.html?embed=1，所见即所得；
     未运行时显示按当前设置生成的示例，运行中跟随后端快照实时刷新。
   - 检查器：外观 / 元素 / 同传 / 输入 / 窗口，控件用 data-bind 声明式绑定，
     改动即预览、500ms 后自动保存（subtitle_save_settings 只写 subtitle 段）。
   - 转录：按字幕 ID 增量追加，译文按 ID + 语言回填。
   依赖 app.js 暴露的 window.V2T。
   ===================================================================== */
(function () {
    'use strict';

    const V = window.V2T;
    if (!V) return;
    const { $, $$, invoke, listen } = V;

    // ==================== 常量 ====================
    const FIXED_KINDS = ['speaker', 'original', 'translation', 'secondary', 'timestamp'];
    const FIXED_LABELS = { speaker: '说话人', original: '原文', translation: '译文', secondary: '副字幕（我）', timestamp: '时间' };
    const CUSTOM_LABELS = { text: '自定义文本', divider: '分隔线', spacer: '间距' };
    const WEIGHTS = [[300, '细'], [400, '常规'], [500, '中等'], [600, '半粗'], [700, '粗'], [800, '特粗'], [900, '超粗']];
    const LANG_SHORT = { '中文': '中', '英文': 'EN', '日文': '日', '韩文': '한', '法文': 'FR', '德文': 'DE', '西班牙文': 'ES', '俄文': 'RU' };
    const SOURCE_NAMES = { microphone: '麦克风', system: '系统声音', dual: '双向同传' };
    const SOURCE_HINTS = {
        microphone: '识别所选麦克风的声音，适合演讲、录课、自己直播。',
        system: '直接捕获电脑正在播放的声音（WASAPI 环回），看视频、开会、看直播都能出字幕，无需选择设备。',
        dual: '系统声音作为「对方」显示原文与译文；麦克风作为「我」显示在副字幕。适合线上会议双向同传。',
    };
    const SAVE_DELAY = 500;
    const STAGE_MIN = 200;
    const STAGE_MAX = 420;
    const TRANSCRIPT_DOM_CAP = 500;

    // ==================== 默认模型（与 config.rs 默认值一致） ====================
    function defaultTheme() {
        return {
            preset: 'custom', fontFamily: 'SimHei', fontSize: 32, fontWeight: 400, italic: false,
            fontColor: '#ffffff', textAlign: 'center', letterSpacing: 0, lineHeight: 1.4,
            textShadowColor: '#000000', textShadowStrength: 4, interimColor: '#ffffff', interimOpacity: 0.7,
            bgColor: '#000000', bgOpacity: 0.6, blur: 20, paddingX: 24, paddingY: 12, maxLines: 3,
            layout: 'vertical', anchorX: 'center', anchorY: 'bottom', maxWidthPct: 100, autoWrap: false,
            translation: { size: 24, weight: 400, color: '#ffffff', opacity: 0.85, prefix: '' },
            speaker: { color: '#818cf8', size: 16, prefix: '' },
            timestamp: { color: '#a1a1aa', size: 14, format: 'HH:MM:SS' },
            secondary: { color: '#7dd3fc', size: 0, opacity: 0.9 },
        };
    }
    function fixedElement(kind, enabled) {
        return { kind, id: kind, enabled, label: FIXED_LABELS[kind], content: '', prefix: '', color: '', fontSize: 0, fontWeight: 0, opacity: 1, align: '' };
    }
    function defaultElements() {
        return [fixedElement('speaker', false), fixedElement('original', true), fixedElement('translation', true),
            fixedElement('secondary', false), fixedElement('timestamp', false)];
    }
    function defaultWindow() {
        return {
            id: 'primary', name: '默认字幕', enabled: true, x: -1, y: -1, width: 1200, height: 120,
            alwaysOnTop: true, clickThrough: false, obsMode: true, autoFit: true,
            translation: { engine: 'none', targetLang: '英文', interim: true },
            theme: defaultTheme(), elements: defaultElements(),
        };
    }
    function normalizeWindow(w) {
        const base = defaultWindow();
        const src = w || {};
        const theme = Object.assign(defaultTheme(), src.theme || {});
        ['translation', 'speaker', 'timestamp', 'secondary'].forEach(k => {
            theme[k] = Object.assign(defaultTheme()[k], (src.theme && src.theme[k]) || {});
        });
        const elements = Array.isArray(src.elements) && src.elements.length ? src.elements.map(e => Object.assign({
            kind: 'text', id: '', enabled: true, label: '', content: '', prefix: '', color: '#ffffff', fontSize: 0, fontWeight: 0, opacity: 1, align: 'center',
        }, e)) : defaultElements();
        // 固定元素补齐（旧配置可能缺 secondary 等）
        FIXED_KINDS.forEach(k => { if (!elements.some(e => e.kind === k)) elements.push(fixedElement(k, false)); });
        return Object.assign(base, src, {
            theme, elements,
            translation: Object.assign(base.translation, src.translation || {}),
        });
    }
    function normalizeSettings(sub) {
        const src = sub || {};
        const windows = Array.isArray(src.windows) && src.windows.length ? src.windows.map(normalizeWindow) : [defaultWindow()];
        return {
            hotkey: src.hotkey || 0x76,
            audioSource: ['microphone', 'system', 'dual'].includes(src.audioSource) ? src.audioSource : 'microphone',
            inputDevice: src.inputDevice || '',
            translationLlm: Object.assign({ apiUrl: '', apiKey: '', model: '' }, src.translationLlm || {}),
            windows,
        };
    }
    const clone = (o) => JSON.parse(JSON.stringify(o));

    // ==================== 状态 ====================
    const S = {
        settings: normalizeSettings(null),
        current: 'primary',
        running: false,
        visible: {},
        transcript: [],
        frameReady: false,
        lastStatus: '',
        expanded: null,
        save: { timer: null, inflight: null, again: false },
        pull: { queued: false, busy: false, fallback: null },
    };
    const win = () => S.settings.windows.find(w => w.id === S.current) || S.settings.windows[0];
    const viewActive = () => V.state.currentView === 'subtitle';

    // ==================== 声明式绑定 ====================
    function resolve(bind) {
        const parts = bind.split('.');
        const scope = parts.shift();
        let obj;
        if (scope === 'theme') obj = win().theme;
        else if (scope === 'window') obj = win();
        else if (scope === 'global') obj = S.settings;
        else if (scope === 'el') obj = win().elements.find(e => e.id === parts.shift());
        if (!obj) return null;
        const key = parts.pop();
        for (const p of parts) {
            if (obj[p] == null || typeof obj[p] !== 'object') obj[p] = {};
            obj = obj[p];
        }
        return { obj, key };
    }
    function readBind(bind) {
        const r = resolve(bind);
        return r ? r.obj[r.key] : undefined;
    }
    function writeBind(bind, value) {
        const r = resolve(bind);
        if (r) r.obj[r.key] = value;
    }
    function parse(raw, type) {
        if (type === 'int') return parseInt(raw, 10) || 0;
        if (type === 'float') return parseFloat(raw) || 0;
        if (type === 'bool') return raw === true || raw === 'true';
        return raw;
    }
    function fmt(value, kind) {
        const n = Number(value);
        switch (kind) {
            case 'px': return `${n}px`;
            case 'pct': return `${Math.round(n * 100)}%`;
            case 'pct100': return `${n}%`;
            case 'x1': return n.toFixed(1);
            case 'lines': return `${n} 行`;
            case 'auto': return n > 0 ? `${n}px` : '自动';
            default: return String(value);
        }
    }

    /// 把模型值写回 root 下所有 data-bind 控件
    function fillControls(root) {
        root.querySelectorAll('[data-bind]').forEach(el => {
            const bind = el.dataset.bind;
            const type = el.dataset.type;
            let v = readBind(bind);
            if (el.classList.contains('switch')) {
                el.dataset.on = String(type === 'engine' ? (v && v !== 'none') : !!v);
            } else if (el.classList.contains('segmented-control')) {
                const target = String(v);
                el.querySelectorAll('.seg-btn').forEach(b => b.classList.toggle('active', b.dataset.value === target));
            } else if (el.tagName === 'SELECT') {
                if (el.dataset.options === 'weight' && !el.options.length) {
                    el.innerHTML = WEIGHTS.map(([w, n]) => `<option value="${w}">${n} ${w}</option>`).join('');
                }
                const val = v == null ? '' : String(v);
                if (val && !Array.from(el.options).some(o => o.value === val)) {
                    el.add(new Option(val, val));
                }
                el.value = val;
            } else if (el.type === 'color') {
                el.value = v || '#ffffff';
            } else {
                el.value = v == null ? '' : v;
                if (el.type === 'range') V.syncSliderFill(el);
            }
            updateOutput(el);
        });
        requestAnimationFrame(() => root.querySelectorAll('.segmented-control .seg-btn.active').forEach(b => V.moveSegIndicator(b)));
    }
    function updateOutput(el) {
        const out = el.nextElementSibling;
        if (out && out.tagName === 'OUTPUT') out.textContent = fmt(el.value, out.dataset.fmt);
    }

    function onControl(el, rawValue) {
        const bind = el.dataset.bind;
        const type = el.dataset.type;
        let value;
        if (el.classList.contains('switch')) {
            const on = el.dataset.on !== 'true';
            el.dataset.on = String(on);
            value = type === 'engine' ? (on ? 'llm' : 'none') : on;
        } else {
            value = parse(rawValue, type);
        }
        writeBind(bind, value);
        if (el.dataset.flag && invoke) {
            invoke('subtitle_set_window_flag', { windowId: S.current, flag: el.dataset.flag, value }).catch(err => V.addLog('warn', `窗口开关设置失败：${err}`, 'subtitle'));
        }
        if (bind.startsWith('theme.') || bind.startsWith('el.')) markCustom();
        if (bind === 'window.name') renderChips();
        if (bind.startsWith('window.translation') || bind === 'global.inputDevice') renderDependent();
        changed();
    }

    function markCustom() {
        const t = win().theme;
        if (t.preset !== 'custom') {
            t.preset = 'custom';
            renderPresets();
        }
    }

    /// 任一设置变化：刷新预览 + 安排保存
    function changed() {
        pushTheme();
        if (!S.running) pushSample();
        scheduleSave();
    }

    function bindControls() {
        const view = $('#view-subtitle');
        const handle = (e) => {
            const el = e.target.closest('[data-bind]');
            if (!el || !view.contains(el)) return;
            if (el.tagName === 'INPUT' || el.tagName === 'SELECT') {
                updateOutput(el);
                if (el.type === 'range') V.syncSliderFill(el);
                onControl(el, el.value);
            }
        };
        view.addEventListener('input', handle);
        view.addEventListener('change', (e) => {
            // 文本/下拉在 input 时已处理；change 只补 select（部分 WebView 不对 select 发 input）
            if (e.target.tagName === 'SELECT') handle(e);
        });
        view.addEventListener('click', (e) => {
            const sw = e.target.closest('.switch[data-bind]');
            if (sw && view.contains(sw)) { onControl(sw); return; }
            const btn = e.target.closest('.segmented-control[data-bind] .seg-btn');
            if (btn) {
                const seg = btn.closest('.segmented-control');
                seg.querySelectorAll('.seg-btn').forEach(b => b.classList.toggle('active', b === btn));
                V.moveSegIndicator(btn);
                onControl(seg, btn.dataset.value);
            }
        });
    }

    // ==================== 自动保存 ====================
    function setSaveState(kind, detail) {
        const el = $('#sv-save-state');
        if (!el) return;
        el.dataset.state = kind;
        el.textContent = { pending: '正在保存…', saved: '已自动保存', error: '保存失败' }[kind] || '';
        el.title = detail ? String(detail) : '';
    }
    function scheduleSave() {
        setSaveState('pending');
        clearTimeout(S.save.timer);
        S.save.timer = setTimeout(flushSave, SAVE_DELAY);
    }
    /// 立即保存（开启字幕、增删窗口前调用，保证后端拿到最新设置）
    async function flushSave() {
        clearTimeout(S.save.timer);
        S.save.timer = null;
        if (!invoke) { setSaveState('saved'); return; }
        if (S.save.inflight) {
            S.save.again = true;
            return S.save.inflight;
        }
        const payload = clone(S.settings);
        S.save.inflight = invoke('subtitle_save_settings', { settings: payload })
            .then(saved => {
                adoptBackendState(saved);
                if (V.state.config) V.state.config.subtitle = clone(S.settings);
                setSaveState('saved');
                renderHotkey();
            })
            .catch(err => {
                console.error('保存字幕设置失败:', err);
                setSaveState('error', err);
                V.showToast('字幕设置保存失败：' + err, 'error');
            })
            .finally(() => {
                S.save.inflight = null;
                if (S.save.again) {
                    S.save.again = false;
                    flushSave();
                }
            });
        return S.save.inflight;
    }
    /// 后端维护的字段（几何、启用状态）回写到本地模型，不覆盖保存期间的新改动
    function adoptBackendState(saved) {
        (saved && saved.windows || []).forEach(sw => {
            const w = S.settings.windows.find(x => x.id === sw.id);
            if (w) Object.assign(w, { x: sw.x, y: sw.y, width: sw.width, height: sw.height, enabled: sw.enabled });
        });
    }

    // ==================== 加载 ====================
    function loadFromConfig(config) {
        S.settings = normalizeSettings(config && config.subtitle);
        if (!S.settings.windows.some(w => w.id === S.current)) S.current = S.settings.windows[0].id;
        renderAll();
    }
    async function reloadFromBackend(selectId) {
        if (!invoke) return;
        try {
            const cfg = await invoke('get_config');
            if (V.state.config) V.state.config.subtitle = cfg.subtitle;
            if (selectId) S.current = selectId;
            loadFromConfig(cfg);
        } catch (err) {
            console.error('重新加载字幕配置失败:', err);
        }
    }

    // ==================== 渲染 ====================
    function renderAll() {
        renderChips();
        renderPresets();
        renderAnchor();
        renderSources();
        renderElements();
        renderHotkey();
        renderDependent();
        fillControls($('#view-subtitle'));
        pushTheme();
        if (!S.running) pushSample();
        else pullSnapshot();
    }

    function renderChips() {
        const box = $('#sv-window-chips');
        if (!box) return;
        box.innerHTML = '';
        S.settings.windows.forEach(w => {
            const chip = document.createElement('button');
            chip.type = 'button';
            chip.className = 'sv-chip' + (w.id === S.current ? ' active' : '');
            chip.setAttribute('role', 'tab');
            chip.dataset.id = w.id;
            const shown = S.running && S.visible[w.id] !== false && w.enabled !== false;
            chip.innerHTML = `<span class="sv-chip-dot${shown ? ' on' : ''}"></span><span class="sv-chip-name"></span>`;
            chip.querySelector('.sv-chip-name').textContent = w.name || '字幕窗口';
            const lang = w.translation && w.translation.engine !== 'none' ? (LANG_SHORT[w.translation.targetLang] || w.translation.targetLang) : '';
            if (lang) {
                const tag = document.createElement('span');
                tag.className = 'sv-chip-tag';
                tag.textContent = lang;
                chip.appendChild(tag);
            }
            box.appendChild(chip);
        });
    }

    function selectWindow(id) {
        if (id === S.current) return;
        S.current = id;
        S.expanded = null;
        renderAll();
    }

    function renderPresets() {
        const preset = win().theme.preset;
        $$('#sv-presets .sv-preset').forEach(b => b.classList.toggle('active', b.dataset.preset === preset));
    }

    function applyPreset(preset) {
        const w = win();
        const on = (kind, v) => { const e = w.elements.find(x => x.kind === kind); if (e) e.enabled = v; };
        on('original', true);
        on('translation', preset !== 'clean');
        on('speaker', preset === 'meeting');
        on('timestamp', preset === 'meeting');
        on('secondary', S.settings.audioSource === 'dual');
        w.theme.layout = preset === 'live' ? 'horizontal' : 'vertical';
        w.theme.autoWrap = preset === 'meeting';
        if (preset === 'clean') w.translation.engine = 'none';
        else if (w.translation.engine === 'none') w.translation.engine = 'llm';
        w.theme.preset = preset;
        renderAll();
        scheduleSave();
    }

    function renderAnchor() {
        const grid = $('#sv-anchor-grid');
        if (!grid) return;
        const t = win().theme;
        if (!grid.children.length) {
            ['top', 'center', 'bottom'].forEach(y => ['left', 'center', 'right'].forEach(x => {
                const b = document.createElement('button');
                b.type = 'button';
                b.className = 'sv-anchor-cell';
                b.dataset.x = x;
                b.dataset.y = y;
                b.setAttribute('aria-label', `${y}-${x}`);
                grid.appendChild(b);
            }));
        }
        grid.querySelectorAll('.sv-anchor-cell').forEach(b => {
            b.classList.toggle('active', b.dataset.x === t.anchorX && b.dataset.y === t.anchorY);
        });
    }

    function renderSources() {
        const src = S.settings.audioSource;
        $$('#sv-sources .sv-source').forEach(b => b.classList.toggle('active', b.dataset.source === src));
        const hint = $('#sv-source-hint');
        if (hint) hint.textContent = SOURCE_HINTS[src] || '';
        const row = $('#sv-device-row');
        if (row) row.hidden = src === 'system';
    }

    function renderHotkey() {
        const name = V.virtualKeyToName(S.settings.hotkey) || 'F7';
        const chip = $('#sv-hotkey-chip');
        if (chip) chip.textContent = name;
        const input = $('#subtitle-hotkey');
        if (input && input.dataset.listening !== 'true') input.value = name;
    }

    /// 依赖多个字段的显示：同传状态、提示条、舞台说明、窗口按钮
    function renderDependent() {
        const w = win();
        const translating = w.translation.engine !== 'none';
        $$('#view-subtitle [data-requires="translate"]').forEach(r => { r.hidden = !translating; });
        const ts = $('#sv-translate-state');
        if (ts) ts.textContent = translating ? `翻译成${w.translation.targetLang}` : '关闭';

        // 识别引擎状态 & 提示条
        const cfg = V.state.config || {};
        const model = (cfg.model_selection && cfg.model_selection.subtitle_model) || 'doubao';
        const hasKey = !!(cfg.model && String(cfg.model.doubao_api_key || '').trim());
        let notice = '';
        if (V.state.config) {
            if (model !== 'doubao') notice = '实时字幕需要豆包流式识别，请在「设置 → 语音识别模型」中切换字幕模型。';
            else if (!hasKey) notice = '还没有填写豆包 API Key，实时字幕无法连接识别服务。';
        }
        const bar = $('#sv-notice');
        if (bar) {
            bar.hidden = !notice;
            $('#sv-notice-text').textContent = notice;
        }
        const eng = $('#sv-engine-state');
        if (eng) {
            eng.textContent = notice ? '未就绪：' + (hasKey ? '字幕模型不是豆包' : '缺少豆包 API Key') : '豆包流式识别 · 已就绪';
            eng.dataset.ok = notice ? 'false' : 'true';
        }

        // 舞台说明
        const meta = $('#sv-stage-meta');
        if (meta) {
            const parts = [w.name || '字幕窗口', SOURCE_NAMES[S.settings.audioSource]];
            parts.push(translating ? `同传 → ${w.translation.targetLang}` : '不翻译');
            let text = parts.join(' · ');
            if (!S.running && S.lastStatus) text += `　｜　上次会话：${S.lastStatus}`;
            meta.textContent = text;
        }

        // 窗口显示按钮
        const visBtn = $('#sv-window-visibility');
        if (visBtn) {
            const shown = S.running && S.visible[S.current] !== false && w.enabled !== false;
            visBtn.textContent = shown ? '隐藏此窗口' : '显示此窗口';
            visBtn.dataset.shown = String(shown);
        }
        const rm = $('#sv-remove-window');
        if (rm) rm.disabled = S.current === 'primary';

        // 运行状态
        const btn = $('#toggle-subtitle-btn');
        if (btn) {
            btn.classList.toggle('active', S.running);
            btn.querySelector('.btn-label').textContent = S.running ? '停止字幕' : '开启实时字幕';
        }
        const pill = $('#sv-live-pill');
        if (pill) {
            pill.dataset.live = String(S.running);
            pill.querySelector('.sv-live-text').textContent = S.running ? '实时' : '示例预览';
        }
    }

    // ==================== 元素编辑器 ====================
    function elementLabel(e) {
        return FIXED_KINDS.includes(e.kind) ? FIXED_LABELS[e.kind] : (CUSTOM_LABELS[e.kind] || '文本');
    }
    function row(label, ctl) {
        return `<div class="sv-row"><label>${label}</label><div class="sv-ctl">${ctl}</div></div>`;
    }
    const color = (bind) => `<input type="color" class="color-input" data-bind="${bind}">`;
    const range = (bind, min, max, step, type, f) => `<input type="range" min="${min}" max="${max}" step="${step}" data-bind="${bind}" data-type="${type}"><output data-fmt="${f || ''}"></output>`;
    const text = (bind, ph) => `<input type="text" class="text-input" data-bind="${bind}" placeholder="${V.escapeHtml(ph || '')}">`;
    const weight = (bind) => `<select class="text-input" data-bind="${bind}" data-type="int" data-options="weight"></select>`;

    function elementBody(e) {
        const p = `el.${e.id}`;
        switch (e.kind) {
            case 'original':
                return '<p class="sv-hint">原文的字体、颜色、描边在「外观」中设置。</p>';
            case 'translation':
                return row('颜色', color('theme.translation.color') + range('theme.translation.opacity', 0, 1, 0.05, 'float', 'pct'))
                    + row('字号', range('theme.translation.size', 10, 96, 1, 'int', 'px'))
                    + row('字重', weight('theme.translation.weight'))
                    + row('前缀', text('theme.translation.prefix', '如「译：」'));
            case 'secondary':
                return '<p class="sv-hint">仅在「双向同传」音源下显示麦克风（我）的原声。</p>'
                    + row('颜色', color('theme.secondary.color') + range('theme.secondary.opacity', 0, 1, 0.05, 'float', 'pct'))
                    + row('字号', range('theme.secondary.size', 0, 96, 1, 'int', 'auto'));
            case 'speaker':
                return row('颜色', color('theme.speaker.color'))
                    + row('字号', range('theme.speaker.size', 8, 48, 1, 'int', 'px'))
                    + row('前缀', text('theme.speaker.prefix', '如「🎙 」'));
            case 'timestamp':
                return row('颜色', color('theme.timestamp.color'))
                    + row('字号', range('theme.timestamp.size', 8, 48, 1, 'int', 'px'))
                    + row('格式', `<select class="text-input" data-bind="theme.timestamp.format"><option value="HH:MM:SS">时:分:秒</option><option value="MM:SS">分:秒</option></select>`);
            case 'divider':
                return row('颜色', color(`${p}.color`) + range(`${p}.opacity`, 0, 1, 0.05, 'float', 'pct'))
                    + `<div class="sv-el-foot"><button class="ghost-btn btn-sm scene-btn-danger" data-el-remove="${e.id}" type="button">删除</button></div>`;
            case 'spacer':
                return row('高度', range(`${p}.fontSize`, 2, 64, 1, 'int', 'px'))
                    + `<div class="sv-el-foot"><button class="ghost-btn btn-sm scene-btn-danger" data-el-remove="${e.id}" type="button">删除</button></div>`;
            default:
                return row('内容', text(`${p}.content`, '{time} · {speaker}'))
                    + row('颜色', color(`${p}.color`) + range(`${p}.opacity`, 0, 1, 0.05, 'float', 'pct'))
                    + row('字号', range(`${p}.fontSize`, 8, 72, 1, 'int', 'px'))
                    + row('字重', weight(`${p}.fontWeight`))
                    + row('对齐', `<div class="segmented-control" data-bind="${p}.align"><button class="seg-btn" data-value="left" type="button">左</button><button class="seg-btn" data-value="center" type="button">中</button><button class="seg-btn" data-value="right" type="button">右</button></div>`)
                    + `<div class="sv-el-foot"><button class="ghost-btn btn-sm scene-btn-danger" data-el-remove="${e.id}" type="button">删除</button></div>`;
        }
    }

    function elementSummary(e) {
        if (e.kind === 'text') return e.content || '（空）';
        if (e.kind === 'secondary') return S.settings.audioSource === 'dual' ? '' : '双向同传时显示';
        if (e.kind === 'translation') return win().translation.engine === 'none' ? '同传未开启' : '';
        return '';
    }

    function renderElements() {
        const list = $('#sv-elements');
        if (!list) return;
        const els = win().elements;
        list.innerHTML = els.map((e, i) => `
            <div class="sv-el${S.expanded === e.id ? ' open' : ''}${e.enabled ? '' : ' off'}" data-id="${V.escapeHtml(e.id)}">
                <div class="sv-el-head" data-el-expand="${V.escapeHtml(e.id)}">
                    <svg class="sv-el-caret" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="m9 6 6 6-6 6"/></svg>
                    <span class="sv-el-name">${elementLabel(e)}</span>
                    <span class="sv-el-summary">${V.escapeHtml(elementSummary(e))}</span>
                    <button class="icon-btn sv-el-move" data-el-move="-1" type="button" title="上移"${i === 0 ? ' disabled' : ''}>
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 15 6-6 6 6"/></svg>
                    </button>
                    <button class="icon-btn sv-el-move" data-el-move="1" type="button" title="下移"${i === els.length - 1 ? ' disabled' : ''}>
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"/></svg>
                    </button>
                    <div class="switch" data-el-toggle="${V.escapeHtml(e.id)}" data-on="${e.enabled}"><div class="switch-knob"></div></div>
                </div>
                ${S.expanded === e.id ? `<div class="sv-el-body">${elementBody(e)}</div>` : ''}
            </div>`).join('');
        const body = list.querySelector('.sv-el-body');
        if (body) {
            fillControls(body);
            V.ensureSegIndicators();
        }
    }

    function bindElementEditor() {
        const list = $('#sv-elements');
        list.addEventListener('click', (e) => {
            const item = e.target.closest('.sv-el');
            if (!item) return;
            const id = item.dataset.id;
            const els = win().elements;
            const idx = els.findIndex(x => x.id === id);
            const toggle = e.target.closest('[data-el-toggle]');
            if (toggle) {
                els[idx].enabled = !els[idx].enabled;
                markCustom();
                renderElements();
                changed();
                return;
            }
            const move = e.target.closest('[data-el-move]');
            if (move) {
                const to = idx + parseInt(move.dataset.elMove, 10);
                if (to < 0 || to >= els.length) return;
                const [it] = els.splice(idx, 1);
                els.splice(to, 0, it);
                markCustom();
                renderElements();
                changed();
                return;
            }
            const rm = e.target.closest('[data-el-remove]');
            if (rm) {
                els.splice(idx, 1);
                S.expanded = null;
                renderElements();
                changed();
                return;
            }
            if (e.target.closest('[data-el-expand]') && !e.target.closest('.sv-el-body')) {
                S.expanded = S.expanded === id ? null : id;
                renderElements();
            }
        });
        $$('[data-add-element]').forEach(b => b.addEventListener('click', () => {
            const kind = b.dataset.addElement;
            const id = 'c_' + Date.now().toString(36) + Math.floor(Math.random() * 1296).toString(36);
            win().elements.push({
                kind, id, enabled: true, label: CUSTOM_LABELS[kind], content: kind === 'text' ? '{time}' : '', prefix: '',
                color: '#ffffff', fontSize: kind === 'spacer' ? 12 : (kind === 'text' ? 18 : 0), fontWeight: 400,
                opacity: kind === 'divider' ? 0.3 : 1, align: 'center',
            });
            S.expanded = id;
            markCustom();
            renderElements();
            changed();
        }));
    }

    // ==================== 预览（iframe） ====================
    function post(type, payload) {
        const frame = $('#sv-preview-frame');
        if (!frame || !frame.contentWindow || !S.frameReady) return;
        frame.contentWindow.postMessage({ source: 'v2t-preview', type, payload }, '*');
    }
    function pushTheme() {
        const w = win();
        post('theme', {
            windowId: w.id,
            flags: { alwaysOnTop: w.alwaysOnTop, clickThrough: w.clickThrough, obsMode: w.obsMode, autoFit: false },
            theme: clone(w.theme),
            elements: clone(w.elements),
            translation: clone(w.translation),
        });
    }

    const SAMPLE = {
        zh: ['欢迎来到今天的发布会。', '接下来演示实时字幕和同声传译。'],
        en: ['Welcome to today\'s keynote.', 'Next, a demo of live captions and interpretation.'],
        liveZh: ['它会把你说的话', '实时变成文字'],
        liveEn: ['It turns what you say', 'into text in real time'],
        tr: {
            '英文': ['Welcome to today\'s keynote.', 'Next, a demo of live captions and interpretation.', 'It turns what you say into text'],
            '日文': ['本日の発表会へようこそ。', '続いてリアルタイム字幕と同時通訳をご紹介します。', '話した言葉をリアルタイムで文字に'],
            '韩文': ['오늘 발표회에 오신 것을 환영합니다.', '이어서 실시간 자막과 동시통역을 시연합니다.', '말하는 내용을 실시간으로 텍스트로'],
            '中文': ['欢迎来到今天的主题演讲。', '接下来演示实时字幕和同声传译。', '它会把你说的话实时变成文字'],
            '法文': ['Bienvenue à la keynote d\'aujourd\'hui.', 'Place à la démo des sous-titres en direct.', 'Il transforme votre voix en texte'],
            '德文': ['Willkommen zur heutigen Keynote.', 'Gleich zeigen wir Live-Untertitel und Dolmetschen.', 'Es verwandelt Sprache sofort in Text'],
            '西班牙文': ['Bienvenidos a la presentación de hoy.', 'Ahora, una demo de subtítulos en vivo.', 'Convierte lo que dices en texto'],
            '俄文': ['Добро пожаловать на сегодняшнюю презентацию.', 'Далее — демонстрация живых субтитров.', 'Превращает речь в текст'],
        },
    };
    function sampleSnapshot() {
        const w = win();
        const translating = w.translation.engine !== 'none';
        const lang = w.translation.targetLang;
        const englishSource = translating && lang === '中文';
        const lines = englishSource ? SAMPLE.en : SAMPLE.zh;
        const live = englishSource ? SAMPLE.liveEn : SAMPLE.liveZh;
        const tr = SAMPLE.tr[lang] || SAMPLE.tr['英文'];
        const dual = S.settings.audioSource === 'dual';
        const speaker = dual ? '对方' : SOURCE_NAMES[S.settings.audioSource];
        const empty = { speaker: '', lines: [], live: { definite: '', indefinite: '' }, liveTranslation: '' };
        return {
            windowId: w.id, running: false, version: 0, status: '', dual, translating,
            a: {
                speaker,
                lines: lines.map((t, i) => ({ id: i + 1, text: t, translation: translating ? tr[i] : '' })),
                live: { definite: live[0], indefinite: live[1] },
                liveTranslation: translating && w.translation.interim ? tr[2] : '',
            },
            b: dual ? { speaker: '我', lines: [{ id: 9, text: englishSource ? 'Sounds great.' : '好的，我明白了。', translation: '' }], live: { definite: '', indefinite: englishSource ? 'one question' : '我想问一下' }, liveTranslation: '' } : empty,
        };
    }
    function pushSample() {
        post('snapshot', sampleSnapshot());
    }

    /// 运行中：收到信号 → 合流到下一帧拉取一次快照
    function schedulePull() {
        if (!S.running || !viewActive() || document.hidden) return;
        if (S.pull.queued) return;
        S.pull.queued = true;
        requestAnimationFrame(() => { S.pull.queued = false; pullSnapshot(); });
    }
    async function pullSnapshot() {
        if (!invoke || S.pull.busy) return;
        S.pull.busy = true;
        try {
            const snap = await invoke('subtitle_snapshot', { windowId: S.current });
            if (S.running) post('snapshot', snap);
            if (snap && snap.status) S.lastStatus = snap.status;
        } catch (err) {
            console.error('拉取字幕快照失败:', err);
        } finally {
            S.pull.busy = false;
        }
    }
    function setRunning(running) {
        if (S.running === running) return;
        S.running = running;
        V.state.isSubtitleActive = running;
        clearInterval(S.pull.fallback);
        if (running) {
            S.lastStatus = '';
            // 信号是主通道；低频兜底防止漏信号
            S.pull.fallback = setInterval(schedulePull, 1000);
            pullSnapshot();
        } else {
            pushSample();
        }
        renderChips();
        renderDependent();
    }

    // ==================== 转录 ====================
    function mmss(ms) {
        const t = Math.floor((ms || 0) / 1000);
        const h = Math.floor(t / 3600);
        const m = String(Math.floor((t % 3600) / 60)).padStart(2, '0');
        const s = String(t % 60).padStart(2, '0');
        return h ? `${h}:${m}:${s}` : `${m}:${s}`;
    }
    function transcriptNode(e) {
        const item = document.createElement('div');
        item.className = 'sv-tr-item';
        item.dataset.id = e.id;
        item.dataset.source = e.source || 'A';
        item.innerHTML = `<div class="sv-tr-meta"><span class="sv-tr-time"></span><span class="sv-tr-speaker"></span></div><div class="sv-tr-body"><div class="sv-tr-text"></div></div>`;
        item.querySelector('.sv-tr-time').textContent = mmss(e.startMs);
        item.querySelector('.sv-tr-speaker').textContent = e.speaker || '';
        item.querySelector('.sv-tr-text').textContent = e.text;
        Object.entries(e.translations || {}).forEach(([lang, t]) => setTranslationNode(item, lang, t));
        return item;
    }
    function setTranslationNode(item, lang, textValue) {
        const body = item.querySelector('.sv-tr-body');
        let line = Array.from(body.querySelectorAll('.sv-tr-trans')).find(n => n.dataset.lang === lang);
        if (!line) {
            line = document.createElement('div');
            line.className = 'sv-tr-trans';
            line.dataset.lang = lang;
            line.innerHTML = '<span class="sv-tr-lang"></span><span class="sv-tr-trans-text"></span>';
            line.querySelector('.sv-tr-lang').textContent = LANG_SHORT[lang] || lang;
            body.appendChild(line);
        }
        line.querySelector('.sv-tr-trans-text').textContent = textValue;
    }
    function nearBottom(list) {
        return list.scrollHeight - list.scrollTop - list.clientHeight < 48;
    }
    function renderTranscript() {
        const list = $('#sv-transcript');
        if (!list) return;
        list.innerHTML = '';
        if (!S.transcript.length) {
            list.innerHTML = `<div class="sv-tr-empty"><b>还没有转录</b><span>开启实时字幕后，每一句定稿的话都会带着时间记录在这里，可导出为 SRT 字幕或会议纪要。</span></div>`;
        } else {
            const frag = document.createDocumentFragment();
            S.transcript.slice(-TRANSCRIPT_DOM_CAP).forEach(e => frag.appendChild(transcriptNode(e)));
            list.appendChild(frag);
            list.scrollTop = list.scrollHeight;
        }
        updateCount();
    }
    function appendTranscript(entries) {
        const list = $('#sv-transcript');
        if (!list || !entries.length) return;
        const stick = nearBottom(list);
        if (!S.transcript.length) list.innerHTML = '';
        S.transcript.push(...entries);
        entries.forEach(e => list.appendChild(transcriptNode(e)));
        while (list.children.length > TRANSCRIPT_DOM_CAP) list.firstElementChild.remove();
        if (stick) list.scrollTop = list.scrollHeight;
        else $('#sv-transcript-jump').hidden = false;
        updateCount();
    }
    function applyTranslation(id, lang, textValue) {
        const entry = S.transcript.find(e => e.id === id);
        if (!entry) return;
        entry.translations = entry.translations || {};
        entry.translations[lang] = textValue;
        const list = $('#sv-transcript');
        const item = list && list.querySelector(`.sv-tr-item[data-id="${id}"]`);
        if (!item) return;
        const stick = nearBottom(list);
        setTranslationNode(item, lang, textValue);
        if (stick) list.scrollTop = list.scrollHeight;
    }
    function updateCount() {
        const el = $('#sv-transcript-count');
        if (el) el.textContent = S.transcript.length ? String(S.transcript.length) : '';
    }
    function transcriptText() {
        return S.transcript.map(e => {
            const head = `[${mmss(e.startMs)}] ${e.speaker ? e.speaker + '：' : ''}${e.text}`;
            const trs = Object.values(e.translations || {}).map(t => `    ${t}`);
            return [head, ...trs].join('\n');
        }).join('\n');
    }
    async function loadTranscript() {
        if (!invoke) { renderTranscript(); return; }
        try {
            const entries = await invoke('get_subtitle_transcript');
            S.transcript = Array.isArray(entries) ? entries : [];
        } catch (err) {
            S.transcript = [];
        }
        renderTranscript();
    }

    // ==================== 动作 ====================
    async function toggleSession() {
        if (!invoke) return;
        const btn = $('#toggle-subtitle-btn');
        btn.disabled = true;
        try {
            if (!S.running) await flushSave();
            const running = await invoke('toggle_subtitle');
            setRunning(!!running);
        } catch (err) {
            V.showToast('字幕开关失败：' + err, 'error');
        } finally {
            btn.disabled = false;
        }
    }

    async function addWindow(duplicate) {
        if (!invoke) {
            const base = duplicate ? clone(win()) : defaultWindow();
            base.id = 'w_' + Date.now().toString(36);
            base.name = duplicate ? `${win().name} 副本` : `字幕窗口 ${S.settings.windows.length + 1}`;
            S.settings.windows.push(normalizeWindow(base));
            S.current = base.id;
            renderAll();
            return;
        }
        try {
            await flushSave();
            const id = await invoke(duplicate ? 'subtitle_duplicate_window' : 'subtitle_add_window', duplicate ? { windowId: S.current } : {});
            await reloadFromBackend(id);
            V.showToast(duplicate ? '已复制字幕窗口' : '已新建字幕窗口', 'success');
        } catch (err) {
            V.showToast('操作失败：' + err, 'error');
        }
    }

    async function removeWindow() {
        if (S.current === 'primary') return;
        const w = win();
        const res = await V.showConfirmDialog('删除字幕窗口', `确定删除「${w.name}」吗？它的样式和翻译设置会一并删除。`, '删除');
        if (!res || !res.confirmed) return;
        if (!invoke) {
            S.settings.windows = S.settings.windows.filter(x => x.id !== w.id);
            S.current = 'primary';
            renderAll();
            return;
        }
        try {
            await flushSave();
            await invoke('subtitle_remove_window', { windowId: w.id });
            await reloadFromBackend('primary');
        } catch (err) {
            V.showToast('删除失败：' + err, 'error');
        }
    }

    async function toggleWindowVisibility() {
        if (!invoke) return;
        const btn = $('#sv-window-visibility');
        const show = btn.dataset.shown !== 'true';
        try {
            if (show) await flushSave();
            await invoke('subtitle_show_window', { windowId: S.current, show });
            S.visible[S.current] = show;
            const w = win();
            if (show) w.enabled = true;
            renderChips();
            renderDependent();
        } catch (err) {
            V.showToast('窗口操作失败：' + err, 'error');
        }
    }

    async function exportTranscript(format) {
        if (!S.transcript.length) { V.showToast('还没有转录内容', 'warn'); return; }
        if (!invoke) return;
        try {
            const path = await invoke('export_subtitle_transcript', { format });
            if (path) V.showToast('已导出：' + path, 'success', 4000);
        } catch (err) {
            if (String(err).includes('取消')) return;
            V.showToast('导出失败：' + err, 'error');
        }
    }

    async function clearTranscript() {
        if (!S.transcript.length) return;
        try {
            if (invoke) await invoke('clear_subtitle_transcript');
            S.transcript = [];
            renderTranscript();
        } catch (err) {
            V.showToast('清空失败：' + err, 'error');
        }
    }

    // 热键捕获（全局字幕开关，存 Windows VK 码）
    function bindHotkey() {
        const input = $('#subtitle-hotkey');
        if (!input) return;
        input.addEventListener('click', () => {
            if (input.dataset.listening === 'true') return;
            input.dataset.listening = 'true';
            input.value = '按下按键…';
            const done = () => {
                input.dataset.listening = 'false';
                document.removeEventListener('keydown', onKey, true);
                renderHotkey();
            };
            const onKey = (e) => {
                e.preventDefault();
                e.stopPropagation();
                if (e.key === 'Escape') { done(); return; }
                // 全局快捷键会被系统吞掉：字母数字会让正常打字失灵，只开放 F1–F12
                const vk = /^F\d{1,2}$/.test(e.key) ? V.nameToVirtualKey(e.key) : null;
                if (!vk) {
                    V.showToast('字幕开关请选择 F1–F12（全局生效，字母键会影响正常打字）', 'warn', 3600);
                } else if (vk !== S.settings.hotkey) {
                    S.settings.hotkey = vk;
                    scheduleSave();
                }
                done();
            };
            document.addEventListener('keydown', onKey, true);
            input.addEventListener('blur', done, { once: true });
        });
    }

    // ==================== 初始化 ====================
    function bindUi() {
        bindControls();
        bindElementEditor();
        bindHotkey();

        $('#toggle-subtitle-btn').addEventListener('click', toggleSession);
        $('#sv-window-chips').addEventListener('click', (e) => {
            const chip = e.target.closest('.sv-chip');
            if (chip) selectWindow(chip.dataset.id);
        });
        $('#sv-add-window').addEventListener('click', () => addWindow(false));
        $('#sv-duplicate-window').addEventListener('click', () => addWindow(true));
        $('#sv-remove-window').addEventListener('click', removeWindow);
        $('#sv-window-visibility').addEventListener('click', toggleWindowVisibility);
        $('#sv-notice-action').addEventListener('click', () => V.switchView('settings'));

        $('#sv-tabs').addEventListener('click', (e) => {
            const tab = e.target.closest('.seg-btn');
            if (!tab) return;
            $$('#sv-tabs .seg-btn').forEach(b => b.classList.toggle('active', b === tab));
            V.moveSegIndicator(tab);
            $$('#view-subtitle .sv-pane').forEach(p => p.classList.toggle('active', p.dataset.pane === tab.dataset.tab));
            try { localStorage.setItem('v2t-subtitle-tab', tab.dataset.tab); } catch (err) {}
            requestAnimationFrame(() => {
                const pane = $(`#view-subtitle .sv-pane[data-pane="${tab.dataset.tab}"]`);
                pane.querySelectorAll('input[type="range"]').forEach(V.syncSliderFill);
                pane.querySelectorAll('.segmented-control .seg-btn.active').forEach(b => V.moveSegIndicator(b));
            });
        });

        $('#sv-presets').addEventListener('click', (e) => {
            const b = e.target.closest('.sv-preset');
            if (b) applyPreset(b.dataset.preset);
        });
        $('#sv-anchor-grid').addEventListener('click', (e) => {
            const cell = e.target.closest('.sv-anchor-cell');
            if (!cell) return;
            const t = win().theme;
            t.anchorX = cell.dataset.x;
            t.anchorY = cell.dataset.y;
            markCustom();
            renderAnchor();
            changed();
        });
        $('#sv-sources').addEventListener('click', (e) => {
            const b = e.target.closest('.sv-source');
            if (!b || b.dataset.source === S.settings.audioSource) return;
            S.settings.audioSource = b.dataset.source;
            // 双向同传需要副字幕；切走后关掉，避免空占位
            S.settings.windows.forEach(w => {
                const sec = w.elements.find(x => x.kind === 'secondary');
                if (sec) sec.enabled = b.dataset.source === 'dual';
            });
            renderSources();
            renderElements();
            renderDependent();
            changed();
        });
        $('#sv-backdrops').addEventListener('click', (e) => {
            const b = e.target.closest('.sv-backdrop-btn');
            if (!b) return;
            $$('#sv-backdrops .sv-backdrop-btn').forEach(x => x.classList.toggle('active', x === b));
            $('#sv-stage').dataset.backdrop = b.dataset.backdrop;
            try { localStorage.setItem('v2t-subtitle-backdrop', b.dataset.backdrop); } catch (err) {}
        });

        $$('#view-subtitle [data-export]').forEach(b => b.addEventListener('click', () => exportTranscript(b.dataset.export)));
        $('#sv-transcript-copy').addEventListener('click', async () => {
            if (!S.transcript.length) { V.showToast('还没有转录内容', 'warn'); return; }
            if (await V.copyToClipboard(transcriptText())) V.showToast(`已复制 ${S.transcript.length} 条转录`, 'success');
        });
        $('#sv-transcript-clear').addEventListener('click', clearTranscript);
        const list = $('#sv-transcript');
        const jump = $('#sv-transcript-jump');
        list.addEventListener('scroll', () => { if (nearBottom(list)) jump.hidden = true; });
        jump.addEventListener('click', () => { list.scrollTop = list.scrollHeight; jump.hidden = true; });

        // 预览渲染器就绪握手
        window.addEventListener('message', (e) => {
            const m = e.data || {};
            if (m.source !== 'v2t-preview') return;
            if (m.type === 'size') {
                const stage = $('#sv-stage');
                stage.style.height = Math.max(STAGE_MIN, Math.min(STAGE_MAX, m.height)) + 'px';
                return;
            }
            if (m.type !== 'ready') return;
            S.frameReady = true;
            pushTheme();
            if (S.running) pullSnapshot(); else pushSample();
        });

        // iframe 可能先于本脚本加载完、ready 已错过：主动 ping 一次，加载完成时再 ping
        const frame = $('#sv-preview-frame');
        const ping = () => { if (frame.contentWindow) frame.contentWindow.postMessage({ source: 'v2t-preview', type: 'ping' }, '*'); };
        frame.addEventListener('load', ping);
        ping();

        // 恢复每个用户自己的界面偏好
        try {
            const tab = localStorage.getItem('v2t-subtitle-tab');
            const btn = tab && $(`#sv-tabs .seg-btn[data-tab="${tab}"]`);
            if (btn) btn.click();
            const bd = localStorage.getItem('v2t-subtitle-backdrop');
            const bdBtn = bd && $(`#sv-backdrops .sv-backdrop-btn[data-backdrop="${bd}"]`);
            if (bdBtn) bdBtn.click();
        } catch (err) {}
    }

    function bindBackend() {
        document.addEventListener('v2t:config', (e) => loadFromConfig(e.detail));
        document.addEventListener('v2t:config-saved', renderDependent);
        document.addEventListener('v2t:view', (e) => {
            if (e.detail !== 'subtitle') return;
            V.loadInputDevices();
            renderDependent();
            requestAnimationFrame(() => {
                $$('#view-subtitle .sv-pane.active input[type="range"]').forEach(V.syncSliderFill);
                $$('#view-subtitle .segmented-control .seg-btn.active').forEach(b => V.moveSegIndicator(b));
            });
            if (S.running) pullSnapshot();
        });
        document.addEventListener('visibilitychange', () => { if (!document.hidden) schedulePull(); });
        if (!listen) return;

        const on = (name, fn) => listen(name, fn).then(un => V.state.unlisteners.push(un)).catch(() => {});
        on('subtitle-signal', schedulePull);
        on('subtitle-session-started', () => {
            S.transcript = [];
            renderTranscript();
            S.settings.windows.forEach(w => { if (w.enabled !== false) S.visible[w.id] = true; });
            setRunning(true);
        });
        on('subtitle-session-stopped', async () => {
            await pullSnapshot();
            setRunning(false);
        });
        on('subtitle-window-state', (e) => {
            const p = e.payload || {};
            if (p.windowId == null) return;
            S.visible[p.windowId] = p.visible !== false;
            const w = S.settings.windows.find(x => x.id === p.windowId);
            if (w && p.visible) w.enabled = true;
            renderChips();
            renderDependent();
        });
        on('subtitle-transcript-updated', (e) => {
            const p = e.payload || {};
            if (p.type === 'append' && Array.isArray(p.entries)) appendTranscript(p.entries);
            else if (p.type === 'translation') applyTranslation(p.id, p.lang, p.text);
        });
        on('app-ready', syncRuntime);
    }

    async function syncRuntime() {
        if (!invoke) return;
        try { setRunning(!!(await invoke('is_subtitle_running'))); } catch (err) {}
        loadTranscript();
    }

    function init() {
        bindUi();
        bindBackend();
        loadFromConfig(V.state.config);
        renderTranscript();
        syncRuntime();
        // 供「设置」页保存整份配置时带上最新字幕设置
        V.subtitleSettings = () => clone(S.settings);
    }

    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
    else init();
})();
