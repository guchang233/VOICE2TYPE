/* =====================================================================
   视频配音（v2）
   ---------------------------------------------------------------------
   一条固定的四步流程：视频 → 识别字幕 → 配音 → 导出。
   - 监视器：原片 / 成片播放，当前句字幕叠加，时间轴色块（语速、配音状态）可点击跳转；
   - 字幕列表：直接改字（Enter 在光标处拆句、行首 Backspace 并入上一句）、改时间、
     单句试听配音、语速超标提醒、合成状态逐句回显；结构性修改可撤销；
   - 工程（视频路径 + 字幕 + 成片）存 localStorage，重启不丢稿。
   依赖 app.js 暴露的 window.V2T。
   ===================================================================== */
(function () {
    'use strict';

    const V = window.V2T;
    if (!V) return;
    const { $, $$, invoke, listen } = V;

    const PROJECT_KEY = 'v2t-dub-project';
    const SETTINGS_KEY = 'v2t-dub-settings';
    const VIDEO_EXT = /\.(mp4|mkv|mov|avi|webm|flv|ts|m4v|wmv)$/i;
    const TAIL_ROOM_MS = 1500;   // 与后端一致：最后一句可向后借用的时长
    const UNDO_LIMIT = 50;
    const STEPS = ['video', 'asr', 'voice', 'export'];

    const DEFAULT_SETTINGS = {
        asrProvider: 'ali-dashscope', asrLang: '', asrWords: true, asrItn: true,
        model: '', speed: null, volume: null, temperature: null, topP: null, normalize: null,
        bgVolume: 0, outDir: '',
    };

    const D = {
        project: null,          // { videoPath, segments, durationMs, outputPath, srtPath, result, size }
        settings: Object.assign({}, DEFAULT_SETTINGS),
        step: 'video',
        running: null,          // 'prepare' | 'generate'
        progress: { percent: 0, message: '' },
        segStatus: {},          // index → { status, fit, ms, error }
        active: -1,
        playUntil: null,
        source: 'original',
        undo: [],
        raf: 0,
        saveTimer: null,
        previewAudio: null,
        textUndoArmed: false,
    };

    // ==================== 工具 ====================
    const clone = (o) => JSON.parse(JSON.stringify(o));
    const segs = () => (D.project ? D.project.segments : []);
    const fileName = (p) => String(p || '').split(/[\\/]/).pop();
    const stem = (p) => fileName(p).replace(/\.[^.]+$/, '');
    const viewActive = () => V.state.currentView === 'dubbing';

    function fmtClock(ms, withFraction) {
        const t = Math.max(0, ms || 0) / 1000;
        const h = Math.floor(t / 3600);
        const m = Math.floor((t % 3600) / 60);
        const s = Math.floor(t % 60);
        const base = (h ? `${h}:${String(m).padStart(2, '0')}` : String(m).padStart(2, '0')) + ':' + String(s).padStart(2, '0');
        return withFraction ? `${base}.${String(Math.floor((t % 1) * 10))}` : base;
    }
    /// 「mm:ss.f」「h:mm:ss.f」或纯秒数 → 毫秒
    function parseClock(text) {
        const parts = String(text).trim().split(':').map(Number);
        if (!parts.length || parts.some(isNaN)) return null;
        const secs = parts.reduce((acc, v) => acc * 60 + v, 0);
        return Math.max(0, Math.round(secs * 1000));
    }

    /// 与后端 estimate_fit_speed 同一套估算：中日韩约 4.8 字/秒，拉丁约 13 字符/秒
    function naturalMs(text, speed) {
        let cjk = 0, latin = 0;
        for (const c of text || '') {
            if (/[\u4e00-\u9fff\u3400-\u4dbf\u3040-\u30ff\uac00-\ud7af]/.test(c)) cjk++;
            else if (/[\p{L}\p{N}]/u.test(c)) latin++;
        }
        return (cjk / 4.8 + latin / 13) * 1000 / (speed || 1);
    }
    function roomOf(i) {
        const list = segs();
        const s = list[i];
        const next = list[i + 1];
        const dur = Math.max(0, s.end_ms - s.start_ms);
        return next ? Math.max(dur, next.start_ms - s.start_ms) : dur + TAIL_ROOM_MS;
    }
    /// 语速等级：ok / warn（需轻微压缩）/ bad（压缩明显，建议删字或延长）
    function rateOf(i) {
        const s = segs()[i];
        const room = roomOf(i);
        if (!s.text.trim() || room <= 0) return { level: 'ok', ratio: 0 };
        const ratio = naturalMs(s.text, ttsSpeed()) / room;
        return { level: ratio > 1.6 ? 'bad' : ratio > 1.2 ? 'warn' : 'ok', ratio };
    }

    function ttsCfg() {
        return (V.state.config && V.state.config.tts) || {};
    }
    function ttsSpeed() {
        const v = D.settings.speed;
        return v == null ? (ttsCfg().speed || 1) : v;
    }

    // ==================== 持久化 ====================
    function saveProject() {
        clearTimeout(D.saveTimer);
        D.saveTimer = setTimeout(() => {
            try {
                if (D.project) localStorage.setItem(PROJECT_KEY, JSON.stringify(D.project));
                else localStorage.removeItem(PROJECT_KEY);
            } catch (e) { /* 存储已满或不可用：只影响重启后恢复 */ }
        }, 300);
    }
    function saveSettings() {
        try { localStorage.setItem(SETTINGS_KEY, JSON.stringify(D.settings)); } catch (e) {}
    }
    function restore() {
        try {
            const s = JSON.parse(localStorage.getItem(SETTINGS_KEY) || 'null');
            if (s) D.settings = Object.assign({}, DEFAULT_SETTINGS, s);
            const p = JSON.parse(localStorage.getItem(PROJECT_KEY) || 'null');
            if (p && p.videoPath) {
                p.segments = Array.isArray(p.segments) ? p.segments : [];
                D.project = p;
                D.step = p.outputPath ? 'export' : p.segments.length ? 'voice' : 'asr';
            }
        } catch (e) {}
    }

    // ==================== 工程 ====================
    function openVideo(path) {
        D.project = { videoPath: path, segments: [], durationMs: 0, outputPath: null, srtPath: null, result: null, size: '' };
        D.segStatus = {};
        D.undo = [];
        D.step = 'asr';
        D.source = 'original';
        saveProject();
        renderAll();
        loadVideo();
    }
    async function pickVideo() {
        if (D.running) return;
        if (!invoke) { openVideo('C:/Videos/demo.mp4'); return; }
        try {
            const path = await invoke('pick_video_file');
            if (path) await confirmReplace(() => openVideo(path));
        } catch (err) {
            V.showToast('选择视频失败：' + err, 'error');
        }
    }
    async function confirmReplace(fn) {
        if (D.project && segs().length) {
            const res = await V.showConfirmDialog('更换视频', '当前视频的字幕和配音进度会被清空，确定继续吗？', '更换');
            if (!res || !res.confirmed) return;
        }
        fn();
    }
    async function closeProject() {
        if (D.running) return;
        const res = await V.showConfirmDialog('关闭视频', '关闭后字幕编辑记录会被清除（已导出的文件不受影响）。', '关闭');
        if (!res || !res.confirmed) return;
        const video = $('#dv-video');
        video.pause();
        video.removeAttribute('src');
        delete video.dataset.path;
        video.load();
        D.project = null;
        D.segStatus = {};
        D.undo = [];
        saveProject();
        renderAll();
    }

    function setSegments(list, { record = true } = {}) {
        if (record) pushUndo();
        D.project.segments = list.map((s, i) => ({
            index: i, start_ms: Math.max(0, Math.round(s.start_ms)), end_ms: Math.max(0, Math.round(s.end_ms)),
            text: s.text || '', words: s.words && s.words.length ? s.words : null,
        }));
        D.segStatus = {};
        markEdited();
        renderList();
        renderTimeline();
    }
    function markEdited() {
        if (D.project.outputPath) D.project.stale = true;
        saveProject();
        renderStepper();
        renderWarnCount();
    }
    function pushUndo() {
        const snap = clone(segs());
        const last = D.undo[D.undo.length - 1];
        if (last && JSON.stringify(last) === JSON.stringify(snap)) return;
        D.undo.push(snap);
        if (D.undo.length > UNDO_LIMIT) D.undo.shift();
    }
    function undo() {
        if (!D.undo.length || D.running) return;
        setSegments(D.undo.pop(), { record: false });
        V.showToast('已撤销', 'info', 1200);
    }

    // ==================== 编辑操作 ====================
    function splitAt(i, charPos) {
        const s = segs()[i];
        if (!s || charPos <= 0 || charPos >= s.text.length) return false;
        let a, b;
        if (s.words && s.words.length > 1) {
            // 对齐到最近的词边界，两侧都至少保留一个词
            let acc = 0, k = s.words.length - 1;
            for (let j = 0; j < s.words.length; j++) {
                acc += s.words[j].text.length;
                if (acc >= charPos) { k = j + 1; break; }
            }
            k = Math.max(1, Math.min(k, s.words.length - 1));
            const wa = s.words.slice(0, k), wb = s.words.slice(k);
            a = { start_ms: s.start_ms, end_ms: wa[wa.length - 1].end_ms, text: wa.map(w => w.text).join('').trim(), words: wa };
            b = { start_ms: wb[0].begin_ms, end_ms: s.end_ms, text: wb.map(w => w.text).join('').trim(), words: wb };
        } else {
            const at = s.start_ms + Math.round((s.end_ms - s.start_ms) * charPos / s.text.length);
            a = { start_ms: s.start_ms, end_ms: at, text: s.text.slice(0, charPos).trim() };
            b = { start_ms: at, end_ms: s.end_ms, text: s.text.slice(charPos).trim() };
        }
        const list = clone(segs());
        list.splice(i, 1, a, b);
        setSegments(list);
        return true;
    }
    function mergeUp(i) {
        if (i <= 0) return null;
        const list = clone(segs());
        const prev = list[i - 1], cur = list[i];
        const caret = prev.text.length;
        const latinJoin = /[A-Za-z0-9]$/.test(prev.text) && /^[A-Za-z0-9]/.test(cur.text);
        prev.text = prev.text + (latinJoin ? ' ' : '') + cur.text;
        prev.end_ms = Math.max(prev.end_ms, cur.end_ms);
        prev.words = prev.words || cur.words ? [...(prev.words || []), ...(cur.words || [])] : null;
        list.splice(i, 1);
        setSegments(list);
        return caret + (latinJoin ? 1 : 0);
    }
    function removeAt(i) {
        const list = clone(segs());
        list.splice(i, 1);
        setSegments(list);
        V.showToast(`已删除第 ${i + 1} 句（Ctrl+Z 撤销）`, 'info', 2000);
    }
    function addAtPlayhead() {
        const video = $('#dv-video');
        const t = Math.round((video.currentTime || 0) * 1000);
        const list = clone(segs());
        let at = list.findIndex(s => s.start_ms > t);
        if (at < 0) at = list.length;
        const prevEnd = at > 0 ? list[at - 1].end_ms : 0;
        const start = Math.max(t, prevEnd);
        const nextStart = at < list.length ? list[at].start_ms : start + 2500;
        list.splice(at, 0, { start_ms: start, end_ms: Math.max(start + 600, Math.min(start + 2500, nextStart)), text: '' });
        setSegments(list);
        focusRow(at, 0);
    }
    /// 按词级时间戳 / 字符比例重新切句，并合并过短的碎句
    function resegment(maxChars, minMs) {
        const endPunct = /[。！？；!?;.…"”]$/;
        const out = [];
        for (const s of segs()) {
            const text = s.text.trim();
            if (!text) continue;
            if (s.words && s.words.length > 1) {
                let pack = [];
                const flush = () => {
                    if (!pack.length) return;
                    out.push({ start_ms: pack[0].begin_ms, end_ms: pack[pack.length - 1].end_ms, text: pack.map(w => w.text).join('').trim(), words: pack });
                    pack = [];
                };
                for (const w of s.words) {
                    pack.push(w);
                    const joined = pack.map(x => x.text).join('');
                    if (joined.length >= maxChars || endPunct.test(joined)) flush();
                }
                flush();
            } else {
                const chars = [...text];
                const parts = Math.max(1, Math.ceil(chars.length / maxChars));
                const per = Math.ceil(chars.length / parts);
                const dur = s.end_ms - s.start_ms;
                for (let p = 0; p < parts; p++) {
                    const piece = chars.slice(p * per, (p + 1) * per).join('').trim();
                    if (!piece) continue;
                    const a = s.start_ms + Math.round(dur * (p * per) / chars.length);
                    const b = s.start_ms + Math.round(dur * Math.min(chars.length, (p + 1) * per) / chars.length);
                    out.push({ start_ms: a, end_ms: Math.max(b, a + 200), text: piece });
                }
            }
        }
        const merged = [];
        for (const s of out) {
            const prev = merged[merged.length - 1];
            if (prev && minMs > 0 && s.end_ms - s.start_ms < minMs && !endPunct.test(prev.text)) {
                prev.end_ms = s.end_ms;
                prev.text += s.text;
                prev.words = prev.words || s.words ? [...(prev.words || []), ...(s.words || [])] : null;
            } else {
                merged.push(s);
            }
        }
        setSegments(merged);
        V.showToast(`重新分段：${merged.length} 句`, 'success');
    }

    // ==================== 渲染 ====================
    function renderAll() {
        const has = !!D.project;
        $('#dv-empty').hidden = has;
        $('#dv-workspace').hidden = !has;
        $('#dv-close-project').hidden = !has || !!D.running;
        $('#dv-cancel').hidden = !D.running;
        if (!has) return;
        renderStepper();
        renderPanes();
        renderFile();
        renderVoice();
        renderSettings();
        renderResult();
        renderList();
        renderTimeline();
        renderSourceSwitch();
    }

    function stepState(step) {
        const p = D.project;
        if (step === 'video') return 'done';
        if (step === 'asr') return D.running === 'prepare' ? 'running' : segs().length ? 'done' : 'todo';
        if (step === 'voice') return D.running === 'generate' ? 'running' : p.outputPath && !p.stale ? 'done' : 'todo';
        return p.outputPath ? (p.stale ? 'stale' : 'done') : 'todo';
    }
    function renderStepper() {
        if (!D.project) return;
        const p = D.project;
        const sub = {
            video: fileName(p.videoPath) + (p.durationMs ? ` · ${fmtClock(p.durationMs)}` : ''),
            asr: D.running === 'prepare' ? `识别中 ${D.progress.percent}%` : segs().length ? `${segs().length} 句` : '未识别',
            voice: D.running === 'generate' ? `配音中 ${D.progress.percent}%` : (ttsCfg().reference_title || '默认音色'),
            export: p.outputPath ? (p.stale ? '字幕已改动，需重新生成' : '已导出') : '',
        };
        $$('#dv-stepper .dv-step').forEach(b => {
            const st = b.dataset.step;
            b.classList.toggle('active', st === D.step);
            b.dataset.state = stepState(st);
            const small = b.querySelector('small');
            if (small.textContent !== sub[st]) small.textContent = sub[st];
            small.title = sub[st];
        });
    }
    function goto(step) {
        if (!STEPS.includes(step)) return;
        D.step = step;
        renderStepper();
        renderPanes();
    }
    function renderPanes() {
        $$('#view-dubbing .dv-pane').forEach(p => p.classList.toggle('active', p.dataset.pane === D.step));
        const busy = !!D.running;
        $('#dv-run-asr').disabled = busy;
        $('#dv-run-asr').textContent = D.running === 'prepare' ? '识别中…' : segs().length ? '重新识别' : '开始识别';
        $('#dv-run-tts').disabled = busy || !segs().length;
        $('#dv-run-tts').textContent = D.running === 'generate' ? '配音中…' : D.project && D.project.outputPath ? '重新生成配音' : '生成配音';
        $('#dv-change-video').disabled = busy;
        $('#dv-import-srt').disabled = busy;
        renderProgress();
    }
    function renderProgress() {
        [['prepare', '#dv-asr-progress'], ['generate', '#dv-tts-progress']].forEach(([phase, sel]) => {
            const box = $(sel);
            const on = D.running === phase;
            box.hidden = !on;
            if (on) {
                box.querySelector('i').style.width = D.progress.percent + '%';
                box.querySelector('span').textContent = D.progress.message || '';
            }
        });
    }

    function renderFile() {
        const p = D.project;
        $('#dv-file-name').textContent = fileName(p.videoPath);
        $('#dv-file-meta').textContent = [p.size, p.durationMs ? fmtClock(p.durationMs) : '', p.videoPath].filter(Boolean).join(' · ');
    }

    function renderVoice() {
        const t = ttsCfg();
        const name = t.reference_title || (t.reference_id ? '自定义音色' : '默认音色');
        $('#dv-voice-name').textContent = name;
        $('#dv-voice-avatar').textContent = [...name][0] || '声';
        const hasKey = !!String(t.fish_api_key || '').trim();
        const sub = $('#dv-voice-sub');
        sub.textContent = hasKey ? 'Fish Audio · 与「语音合成」共用' : '未填写 Fish Audio API Key（在「语音合成」页设置）';
        sub.classList.toggle('warn', !hasKey && !!V.state.config);
    }

    function renderSettings() {
        const t = ttsCfg();
        const s = D.settings;
        const set = (sel, v) => { const el = $(sel); if (el) el.value = v; };
        set('#dv-asr-provider', s.asrProvider);
        set('#dv-asr-lang', s.asrLang);
        $('#dv-asr-words').dataset.on = String(!!s.asrWords);
        $('#dv-asr-itn').dataset.on = String(!!s.asrItn);
        set('#dv-tts-model', s.model || t.model || 's2.1-pro-free');
        set('#dv-tts-speed', s.speed ?? t.speed ?? 1);
        set('#dv-tts-volume', s.volume ?? t.volume ?? 0);
        set('#dv-tts-temp', s.temperature ?? t.temperature ?? 0.7);
        set('#dv-tts-top-p', s.topP ?? t.top_p ?? 0.7);
        $('#dv-tts-normalize').dataset.on = String(s.normalize ?? (t.normalize !== false));
        set('#dv-bg-volume', s.bgVolume || 0);
        set('#dv-out-dir', s.outDir || '');
        $$('#view-dubbing .dv-range input[type="range"]').forEach(r => { V.syncSliderFill(r); updateRangeLabel(r); });
        const ali = s.asrProvider !== 'global-compat';
        $$('#view-dubbing [data-ali-only]').forEach(el => { el.hidden = !ali; });
        const cfg = V.state.config;
        const hasAli = !!(cfg && cfg.model && String(cfg.model.dashscope_api_key || '').trim());
        const hint = $('#dv-asr-hint');
        hint.textContent = ali
            ? (cfg && !hasAli ? '还没有填写阿里云百炼 Key，会自动改用「设置 → 整段识别」的模型。' : 'qwen3-asr-flash-filetrans，长视频自动分块，按句给出时间轴。')
            : '使用「设置 → 整段识别」中配置的云端模型（需要支持时间戳）。';
        hint.classList.toggle('warn', ali && !!cfg && !hasAli);
    }
    function updateRangeLabel(r) {
        const out = r.parentElement.querySelector('output');
        if (!out) return;
        const v = parseFloat(r.value);
        out.textContent = {
            'dv-tts-speed': `${v.toFixed(2)}×`,
            'dv-tts-volume': `${v > 0 ? '+' : ''}${v} dB`,
            'dv-bg-volume': v > 0 ? `${Math.round(v * 100)}%` : '不保留',
        }[r.id] || v.toFixed(2);
    }

    function renderResult() {
        const p = D.project;
        const has = !!p.outputPath;
        $('#dv-result').hidden = !has;
        $('#dv-result-empty').hidden = has;
        $('#dv-open-folder').disabled = !has;
        if (!has) return;
        $('#dv-result-path').textContent = p.outputPath;
        const r = p.result || {};
        const parts = [`${r.segments || segs().length} 句`];
        if (r.compressedSegments) parts.push(`${r.compressedSegments} 句轻微加速`);
        if (r.truncatedSegments) parts.push(`${r.truncatedSegments} 句截尾`);
        if (r.failedSegments) parts.push(`${r.failedSegments} 句合成失败（已留空）`);
        parts.push('已附带同名 SRT');
        $('#dv-result-stats').textContent = parts.join(' · ');
    }

    function renderSourceSwitch() {
        const sw = $('#dv-source-switch');
        sw.hidden = !D.project.outputPath;
        sw.querySelectorAll('button').forEach(b => b.classList.toggle('active', b.dataset.src === D.source));
    }

    // ---- 字幕列表 ----
    function rowHtml(s, i) {
        const esc = V.escapeHtml;
        return `<div class="dv-row" data-i="${i}">
            <button class="dv-row-play" data-act="play" type="button" title="播放这一句"><span class="dv-row-no">${i + 1}</span><svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.5v15a1 1 0 0 0 1.5.9l12-7.5a1 1 0 0 0 0-1.8l-12-7.5A1 1 0 0 0 7 4.5Z"/></svg></button>
            <div class="dv-row-main">
                <div class="dv-row-text" contenteditable="${D.running ? 'false' : 'plaintext-only'}" spellcheck="false" data-placeholder="输入这一句要配的文字">${esc(s.text)}</div>
                <div class="dv-row-meta">
                    <input class="dv-t" data-f="start_ms" value="${fmtClock(s.start_ms, true)}" ${D.running ? 'disabled' : ''} aria-label="开始时间">
                    <span class="dv-t-sep">→</span>
                    <input class="dv-t" data-f="end_ms" value="${fmtClock(s.end_ms, true)}" ${D.running ? 'disabled' : ''} aria-label="结束时间">
                    <span class="dv-rate"></span>
                    <span class="dv-tts"></span>
                </div>
            </div>
            <div class="dv-row-ops">
                <button class="icon-btn" data-act="preview" type="button" title="试听这一句的配音">
                    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5 6 9H3v6h3l5 4V5Z"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/></svg>
                </button>
                <button class="icon-btn" data-act="split" type="button" title="从中间拆成两句（编辑时按 Enter 在光标处拆）" ${D.running ? 'disabled' : ''}>
                    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><path d="M8 7.5 20 16"/><path d="M8 16.5 20 8"/></svg>
                </button>
                <button class="icon-btn" data-act="merge" type="button" title="并入上一句" ${i === 0 || D.running ? 'disabled' : ''}>
                    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="m6 11 6-6 6 6"/><path d="M12 5v14"/></svg>
                </button>
                <button class="icon-btn dv-del" data-act="delete" type="button" title="删除这一句" ${D.running ? 'disabled' : ''}>
                    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>
                </button>
            </div>
        </div>`;
    }
    function renderList() {
        const list = $('#dv-list');
        if (!D.project) return;
        const all = segs();
        $('#dv-count').textContent = all.length ? String(all.length) : '';
        if (!all.length) {
            list.innerHTML = D.running === 'prepare'
                ? '<div class="dv-list-empty"><span class="dv-spinner"></span><b>正在识别…</b><span>识别出的句子会陆续出现在这里。</span></div>'
                : '<div class="dv-list-empty"><b>还没有字幕</b><span>在上方「识别字幕」一键识别原声，或者导入现成的 SRT。</span></div>';
            renderWarnCount();
            return;
        }
        const keep = list.scrollTop;
        list.innerHTML = all.map(rowHtml).join('');
        list.scrollTop = keep;
        all.forEach((_, i) => { updateRowRate(i); updateRowTts(i); });
        if (D.active >= 0) markActive(D.active, false);
        renderWarnCount();
    }
    function appendRows(added) {
        const list = $('#dv-list');
        if (list.querySelector('.dv-list-empty')) list.innerHTML = '';
        const stick = list.scrollHeight - list.scrollTop - list.clientHeight < 48;
        const start = segs().length - added.length;
        list.insertAdjacentHTML('beforeend', added.map((s, k) => rowHtml(s, start + k)).join(''));
        added.forEach((_, k) => updateRowRate(start + k));
        if (start > 0) updateRowRate(start - 1);   // 上一句的可用空间随新句变化
        if (stick) list.scrollTop = list.scrollHeight;
        $('#dv-count').textContent = String(segs().length);
    }
    const rowEl = (i) => $(`#dv-list .dv-row[data-i="${i}"]`);
    function updateRowRate(i) {
        const row = rowEl(i);
        if (!row) return;
        const r = rateOf(i);
        const chip = row.querySelector('.dv-rate');
        row.dataset.rate = r.level;
        chip.textContent = r.level === 'ok' ? '' : `${r.level === 'bad' ? '太快' : '偏快'} ${r.ratio.toFixed(1)}×`;
        chip.title = r.level === 'ok' ? '' : `按正常语速需要 ${(naturalMs(segs()[i].text, ttsSpeed()) / 1000).toFixed(1)} 秒，这里只有 ${(roomOf(i) / 1000).toFixed(1)} 秒，会被加速。可以删减文字或延长结束时间。`;
    }
    function updateRowTts(i) {
        const row = rowEl(i);
        if (!row) return;
        const st = D.segStatus[i];
        const chip = row.querySelector('.dv-tts');
        row.dataset.tts = st ? st.status : '';
        if (!st) { chip.textContent = ''; chip.title = ''; return; }
        const label = st.status === 'running' ? '合成中'
            : st.status === 'failed' ? '合成失败'
            : st.fit === 'compressed' ? '已配音 · 加速' : st.fit === 'truncated' ? '已配音 · 截尾' : '已配音';
        chip.textContent = label;
        chip.title = st.error || (st.ms ? `配音时长 ${(st.ms / 1000).toFixed(1)} 秒` : '');
    }
    function renderWarnCount() {
        const el = $('#dv-warn-count');
        if (!D.project) return;
        let warn = 0;
        segs().forEach((_, i) => { if (rateOf(i).level !== 'ok') warn++; });
        el.hidden = !warn;
        el.textContent = warn ? `${warn} 句语速偏快` : '';
    }
    function focusRow(i, caret) {
        requestAnimationFrame(() => {
            const row = rowEl(i);
            if (!row) return;
            const ed = row.querySelector('.dv-row-text');
            ed.focus();
            const node = ed.firstChild || ed;
            const range = document.createRange();
            const pos = Math.min(caret == null ? (node.textContent || '').length : caret, (node.textContent || '').length);
            try { range.setStart(node, pos); } catch (e) { range.selectNodeContents(ed); }
            range.collapse(true);
            const sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
            scrollRowIntoView(row);
        });
    }
    function scrollRowIntoView(row) {
        const list = $('#dv-list');
        const top = row.offsetTop - list.offsetTop;
        if (top < list.scrollTop + 8 || top + row.offsetHeight > list.scrollTop + list.clientHeight - 8) {
            list.scrollTop = Math.max(0, top - list.clientHeight / 3);
        }
    }
    function caretOffset(el) {
        const sel = window.getSelection();
        if (!sel.rangeCount || !el.contains(sel.anchorNode)) return -1;
        const r = sel.getRangeAt(0).cloneRange();
        r.selectNodeContents(el);
        r.setEnd(sel.anchorNode, sel.anchorOffset);
        return r.toString().length;
    }

    // ---- 时间轴 ----
    function totalMs() {
        const video = $('#dv-video');
        const vd = video && isFinite(video.duration) ? video.duration * 1000 : 0;
        const last = segs().length ? segs()[segs().length - 1].end_ms : 0;
        return Math.max(vd, D.project ? D.project.durationMs || 0 : 0, last, 1);
    }
    function renderTimeline() {
        if (!D.project) return;
        const total = totalMs();
        const box = $('#dv-timeline-blocks');
        box.innerHTML = segs().map((s, i) => {
            const left = (s.start_ms / total) * 100;
            const width = Math.max(0.25, ((s.end_ms - s.start_ms) / total) * 100);
            const st = D.segStatus[i];
            return `<i data-i="${i}" data-rate="${rateOf(i).level}" data-tts="${st ? st.status : ''}" style="left:${left}%;width:${width}%"></i>`;
        }).join('');
        updatePlayhead();
    }
    function updateTimelineBlock(i) {
        const b = $(`#dv-timeline-blocks i[data-i="${i}"]`);
        if (!b) return;
        b.dataset.rate = rateOf(i).level;
        b.dataset.tts = D.segStatus[i] ? D.segStatus[i].status : '';
    }

    // ==================== 播放器 ====================
    function loadVideo() {
        if (!D.project) return;
        const video = $('#dv-video');
        const path = D.source === 'output' && D.project.outputPath ? D.project.outputPath : D.project.videoPath;
        const url = V.toAssetUrl(path);
        $('#dv-player-msg').hidden = true;
        if (video.dataset.path === path) return;
        const t = video.currentTime || 0;
        video.dataset.path = path;
        video.src = url;
        video.addEventListener('loadedmetadata', () => { if (t) video.currentTime = Math.min(t, video.duration || t); }, { once: true });
    }
    function togglePlay() {
        const video = $('#dv-video');
        if (!video.src) return;
        D.playUntil = null;
        if (video.paused) video.play().catch(() => {}); else video.pause();
    }
    function playSegment(i) {
        const s = segs()[i];
        const video = $('#dv-video');
        if (!s || !video.src) return;
        video.currentTime = s.start_ms / 1000;
        D.playUntil = s.end_ms;
        video.play().catch(() => {});
    }
    function updatePlayhead() {
        const video = $('#dv-video');
        const ms = (video.currentTime || 0) * 1000;
        $('#dv-playhead').style.left = `${Math.min(100, (ms / totalMs()) * 100)}%`;
        $('#dv-time').textContent = `${fmtClock(ms)} / ${fmtClock(totalMs())}`;
        const all = segs();
        let idx = -1;
        for (let i = 0; i < all.length; i++) {
            if (ms >= all[i].start_ms && ms < all[i].end_ms) { idx = i; break; }
            if (all[i].start_ms > ms) break;
        }
        const cap = $('#dv-player-caption');
        const text = idx >= 0 ? all[idx].text : '';
        if (cap.textContent !== text) cap.textContent = text;
        cap.hidden = !text || D.source === 'output';
        if (idx !== D.active) markActive(idx, !video.paused);
    }
    function markActive(idx, follow) {
        const prev = rowEl(D.active);
        if (prev) prev.classList.remove('playing');
        D.active = idx;
        const row = rowEl(idx);
        if (!row) return;
        row.classList.add('playing');
        if (follow && $('#dv-follow').checked && !row.contains(document.activeElement)) scrollRowIntoView(row);
    }
    function tick() {
        const video = $('#dv-video');
        if (D.playUntil != null && video.currentTime * 1000 >= D.playUntil) {
            video.pause();
            D.playUntil = null;
        }
        updatePlayhead();
        D.raf = video.paused ? 0 : requestAnimationFrame(tick);
    }

    // ==================== 后端任务 ====================
    function ttsOverrides() {
        const s = D.settings;
        const t = ttsCfg();
        return {
            model: s.model || t.model || undefined,
            reference_id: t.reference_id || '',
            reference_title: t.reference_title || '',
            speed: s.speed ?? t.speed ?? 1,
            volume: s.volume ?? t.volume ?? 0,
            temperature: s.temperature ?? t.temperature ?? 0.7,
            top_p: s.topP ?? t.top_p ?? 0.7,
            normalize: s.normalize ?? (t.normalize !== false),
        };
    }
    function payloadSegments() {
        return segs().map(s => ({
            index: s.index, start_ms: s.start_ms, end_ms: s.end_ms, text: s.text,
            words: s.words ? s.words.map(w => ({ begin_ms: w.begin_ms, end_ms: w.end_ms, text: w.text })) : null,
        }));
    }
    function setRunning(phase) {
        D.running = phase;
        D.progress = { percent: 0, message: phase === 'prepare' ? '准备中…' : phase === 'generate' ? '准备中…' : '' };
        $('#dv-cancel').hidden = !phase;
        $('#dv-close-project').hidden = !!phase || !D.project;
        renderPanes();
        renderStepper();
    }

    async function runPrepare() {
        if (D.running || !D.project) return;
        if (segs().length) {
            const res = await V.showConfirmDialog('重新识别', '会用新的识别结果替换当前字幕（包括你的修改）。', '重新识别');
            if (!res || !res.confirmed) return;
            pushUndo();
        }
        D.project.segments = [];
        D.segStatus = {};
        setRunning('prepare');
        renderList();
        renderTimeline();
        if (!invoke) return;
        try {
            await invoke('dubbing_prepare', {
                videoPath: D.project.videoPath,
                options: {
                    asr_provider: D.settings.asrProvider,
                    ali_enable_words: !!D.settings.asrWords,
                    ali_enable_itn: !!D.settings.asrItn,
                    ali_language: D.settings.asrLang || '',
                },
            });
        } catch (err) {
            setRunning(null);
            renderList();
            V.showToast('无法开始识别：' + err, 'error');
        }
    }

    async function runGenerate() {
        if (D.running || !D.project) return;
        const t = ttsCfg();
        if (V.state.config && !String(t.fish_api_key || '').trim()) {
            V.showToast('请先在「语音合成」页填写 Fish Audio API Key', 'warn', 3600);
            return;
        }
        // 与后端同样规整（排序、去空），保证逐句状态的序号一一对应
        const cleaned = segs().filter(s => s.text.trim()).sort((a, b) => a.start_ms - b.start_ms);
        if (!cleaned.length) { V.showToast('没有可配音的字幕', 'warn'); return; }
        if (cleaned.length !== segs().length || cleaned.some((s, i) => s !== segs()[i])) setSegments(cleaned);
        D.segStatus = {};
        D.source = 'original';
        renderSourceSwitch();
        setRunning('generate');
        renderList();
        renderTimeline();
        if (!invoke) return;
        try {
            await invoke('dubbing_generate', {
                videoPath: D.project.videoPath,
                segments: payloadSegments(),
                options: { output_dir: D.settings.outDir || null, tts: ttsOverrides(), original_volume: D.settings.bgVolume || 0 },
            });
        } catch (err) {
            setRunning(null);
            renderList();
            V.showToast('无法开始配音：' + err, 'error');
        }
    }

    function onProgress(p) {
        if (!p || !D.project) return;
        const phase = p.phase || D.running;
        if (p.status === 'running') {
            if (!D.running) setRunning(phase);
            D.progress = { percent: p.percent || 0, message: p.message || '' };
            renderProgress();
            renderStepper();
            return;
        }
        const wasPhase = D.running || phase;
        setRunning(null);
        if (p.status === 'done') {
            const r = p.result || {};
            if (r.phase === 'prepare') {
                if (Array.isArray(r.segments)) D.project.segments = r.segments;
                if (r.durationMs) D.project.durationMs = r.durationMs;
                D.project.outputPath = null;
                D.project.stale = false;
                saveProject();
                renderList();
                renderTimeline();
                goto('voice');
                V.showToast(`识别完成：${segs().length} 句。可以直接在下方修改字幕，再生成配音`, 'success', 4200);
            } else {
                D.project.outputPath = r.output || null;
                D.project.srtPath = r.subtitle || null;
                D.project.result = r;
                D.project.stale = false;
                saveProject();
                D.source = 'output';
                loadVideo();
                renderResult();
                renderSourceSwitch();
                goto('export');
                V.showToast(r.failedSegments ? `配音完成，${r.failedSegments} 句合成失败已留空` : '配音完成', r.failedSegments ? 'warn' : 'success');
            }
        } else if (p.status === 'cancelled') {
            V.showToast('已取消', 'info');
            if (wasPhase === 'prepare') saveProject();
        } else if (p.status === 'error') {
            V.showToast((wasPhase === 'prepare' ? '识别失败：' : '配音失败：') + (p.message || ''), 'error', 8000);
        }
        renderAll();
    }

    function onTranscript(p) {
        if (!p || D.running !== 'prepare' || !Array.isArray(p.added) || !p.added.length) return;
        const base = segs().length;
        const added = p.added.map((s, k) => Object.assign({}, s, { index: base + k }));
        D.project.segments.push(...added);
        appendRows(added);
        renderTimeline();
        renderStepper();
    }

    function onSegment(p) {
        if (!p || D.running !== 'generate') return;
        D.segStatus[p.index] = { status: p.status, fit: p.fit, ms: p.ms, error: p.error };
        updateRowTts(p.index);
        updateTimelineBlock(p.index);
        if (p.status === 'running' && $('#dv-follow').checked) {
            const row = rowEl(p.index);
            if (row) scrollRowIntoView(row);
        }
    }

    async function previewLine(i, btn) {
        const s = segs()[i];
        if (!s || !s.text.trim()) { V.showToast('这一句还没有文字', 'warn'); return; }
        if (!invoke) return;
        btn.classList.add('loading');
        btn.disabled = true;
        try {
            const path = await invoke('dubbing_preview_segment', { text: s.text, slotMs: roomOf(i), tts: ttsOverrides() });
            if (D.previewAudio) D.previewAudio.pause();
            D.previewAudio = new Audio(V.toAssetUrl(path));
            D.previewAudio.play().catch(() => {});
        } catch (err) {
            V.showToast('试听失败：' + err, 'error');
        } finally {
            btn.classList.remove('loading');
            btn.disabled = false;
        }
    }

    async function importSrt() {
        if (D.running || !invoke) return;
        try {
            const list = await invoke('dubbing_import_srt');
            if (!list) return;
            if (segs().length) {
                const res = await V.showConfirmDialog('导入字幕', `用导入的 ${list.length} 句替换当前字幕吗？`, '替换');
                if (!res || !res.confirmed) return;
            }
            setSegments(list);
            goto('voice');
            V.showToast(`已导入 ${list.length} 句字幕`, 'success');
        } catch (err) {
            V.showToast('导入失败：' + err, 'error');
        }
    }
    async function exportSrt() {
        if (!segs().length) { V.showToast('还没有字幕', 'warn'); return; }
        if (!invoke) return;
        try {
            const path = await invoke('dubbing_export_srt', { segments: payloadSegments(), fileName: stem(D.project.videoPath) });
            if (path) V.showToast('已导出：' + path, 'success', 4000);
        } catch (err) {
            V.showToast('导出失败：' + err, 'error');
        }
    }

    // ==================== 事件绑定 ====================
    function bindList() {
        const list = $('#dv-list');
        list.addEventListener('click', (e) => {
            const btn = e.target.closest('button[data-act]');
            if (!btn) return;
            const i = Number(btn.closest('.dv-row').dataset.i);
            const act = btn.dataset.act;
            if (act === 'play') playSegment(i);
            else if (act === 'preview') previewLine(i, btn);
            else if (D.running) return;
            else if (act === 'split') {
                const s = segs()[i];
                if (!splitAt(i, Math.ceil(s.text.length / 2))) V.showToast('这一句太短，无法拆分', 'warn');
            } else if (act === 'merge') mergeUp(i);
            else if (act === 'delete') removeAt(i);
        });
        list.addEventListener('input', (e) => {
            const ed = e.target.closest('.dv-row-text');
            if (!ed) return;
            const i = Number(ed.closest('.dv-row').dataset.i);
            // 每次聚焦后的第一次输入前记一个撤销点（模型此时还是改动前的文字）
            if (D.textUndoArmed) { pushUndo(); D.textUndoArmed = false; }
            segs()[i].text = ed.textContent.replace(/\n/g, ' ');
            updateRowRate(i);
            updateTimelineBlock(i);
            if (i === D.active) $('#dv-player-caption').textContent = segs()[i].text;
            markEdited();
        });
        list.addEventListener('focusin', (e) => {
            if (e.target.closest('.dv-row-text')) D.textUndoArmed = true;
        });
        list.addEventListener('keydown', (e) => {
            const ed = e.target.closest('.dv-row-text');
            if (!ed || D.running) return;
            const i = Number(ed.closest('.dv-row').dataset.i);
            if (e.key === 'Enter') {
                e.preventDefault();
                const pos = caretOffset(ed);
                if (splitAt(i, pos)) focusRow(i + 1, 0);
            } else if (e.key === 'Backspace' && i > 0 && caretOffset(ed) === 0 && window.getSelection().isCollapsed) {
                e.preventDefault();
                const caret = mergeUp(i);
                if (caret != null) focusRow(i - 1, caret);
            } else if (e.key === 'ArrowDown' && e.altKey) {
                e.preventDefault();
                focusRow(Math.min(segs().length - 1, i + 1));
            } else if (e.key === 'ArrowUp' && e.altKey) {
                e.preventDefault();
                focusRow(Math.max(0, i - 1));
            }
        });
        // 粘贴只保留纯文本、单行
        list.addEventListener('paste', (e) => {
            const ed = e.target.closest('.dv-row-text');
            if (!ed) return;
            e.preventDefault();
            const text = (e.clipboardData.getData('text/plain') || '').replace(/\s*\n\s*/g, ' ');
            document.execCommand('insertText', false, text);
        });
        list.addEventListener('change', (e) => {
            const inp = e.target.closest('.dv-t');
            if (!inp) return;
            const i = Number(inp.closest('.dv-row').dataset.i);
            const s = segs()[i];
            const v = parseClock(inp.value);
            if (v == null) { inp.value = fmtClock(s[inp.dataset.f], true); return; }
            pushUndo();
            s[inp.dataset.f] = v;
            if (s.end_ms <= s.start_ms) {
                if (inp.dataset.f === 'start_ms') s.end_ms = s.start_ms + 500; else s.start_ms = Math.max(0, s.end_ms - 500);
            }
            inp.value = fmtClock(s[inp.dataset.f], true);
            markEdited();
            // 时间变化可能改变顺序与相邻句的可用空间
            const sorted = segs().every((x, k, arr) => k === 0 || arr[k - 1].start_ms <= x.start_ms);
            if (!sorted) setSegments(clone(segs()).sort((a, b) => a.start_ms - b.start_ms), { record: false });
            else {
                renderList();
                renderTimeline();
            }
        });
        list.addEventListener('keydown', (e) => {
            if (e.target.classList.contains('dv-t') && e.key === 'Enter') e.target.blur();
        });
    }

    function bindUi() {
        $('#dv-pick').addEventListener('click', pickVideo);
        $('#dv-change-video').addEventListener('click', pickVideo);
        $('#dv-import-srt').addEventListener('click', importSrt);
        $('#dv-close-project').addEventListener('click', closeProject);
        $('#dv-cancel').addEventListener('click', () => { if (invoke) invoke('dubbing_cancel').catch(() => {}); });
        $('#dv-run-asr').addEventListener('click', runPrepare);
        $('#dv-run-tts').addEventListener('click', runGenerate);
        $('#dv-export-srt').addEventListener('click', exportSrt);
        $('#dv-export-srt-2').addEventListener('click', exportSrt);
        $('#dv-add').addEventListener('click', () => { if (!D.running) addAtPlayhead(); });
        $('#dv-voice-pick').addEventListener('click', () => V.openVoiceLib());
        $('#dv-open-folder').addEventListener('click', () => {
            const p = D.project && D.project.outputPath;
            if (p && invoke) invoke('open_directory', { path: p.replace(/[\\/][^\\/]+$/, '') }).catch(err => V.showToast('打开失败：' + err, 'error'));
        });
        $('#dv-pick-dir').addEventListener('click', async () => {
            if (!invoke) return;
            const dir = await invoke('pick_dub_output_dir').catch(() => null);
            if (!dir) return;
            D.settings.outDir = dir;
            $('#dv-out-dir').value = dir;
            saveSettings();
        });
        $('#dv-out-dir').addEventListener('input', (e) => { D.settings.outDir = e.target.value.trim(); saveSettings(); });

        $('#dv-stepper').addEventListener('click', (e) => {
            const b = e.target.closest('.dv-step');
            if (b) goto(b.dataset.step);
        });
        $$('#view-dubbing .dv-next[data-goto]').forEach(b => b.addEventListener('click', () => goto(b.dataset.goto)));

        // 识别 / 合成参数（本页单独记忆，不改动「语音合成」页的设置）
        const settingInputs = {
            'dv-asr-provider': ['asrProvider', v => v], 'dv-asr-lang': ['asrLang', v => v], 'dv-tts-model': ['model', v => v],
            'dv-tts-speed': ['speed', parseFloat], 'dv-tts-volume': ['volume', parseFloat], 'dv-tts-temp': ['temperature', parseFloat],
            'dv-tts-top-p': ['topP', parseFloat], 'dv-bg-volume': ['bgVolume', parseFloat],
        };
        $('#view-dubbing').addEventListener('input', (e) => {
            const spec = settingInputs[e.target.id];
            if (!spec) return;
            D.settings[spec[0]] = spec[1](e.target.value);
            if (e.target.type === 'range') { V.syncSliderFill(e.target); updateRangeLabel(e.target); }
            saveSettings();
            if (e.target.id === 'dv-tts-speed' && D.project) {
                segs().forEach((_, i) => updateRowRate(i));
                renderWarnCount();
                renderTimeline();
            }
        });
        $('#view-dubbing').addEventListener('change', (e) => {
            if (e.target.id === 'dv-asr-provider' || e.target.id === 'dv-asr-lang' || e.target.id === 'dv-tts-model') {
                D.settings[settingInputs[e.target.id][0]] = e.target.value;
                saveSettings();
                renderSettings();
            }
        });
        [['dv-asr-words', 'asrWords'], ['dv-asr-itn', 'asrItn'], ['dv-tts-normalize', 'normalize']].forEach(([id, key]) => {
            $('#' + id).addEventListener('click', (e) => {
                const sw = e.currentTarget;
                const on = sw.dataset.on !== 'true';
                sw.dataset.on = String(on);
                D.settings[key] = on;
                saveSettings();
            });
        });

        // 重新分段
        $('#dv-reseg').addEventListener('click', () => { $('#dv-reseg-panel').hidden = !$('#dv-reseg-panel').hidden; });
        $('#dv-reseg-apply').addEventListener('click', () => {
            if (D.running || !segs().length) return;
            const chars = Math.max(6, parseInt($('#dv-reseg-chars').value, 10) || 20);
            const minMs = Math.max(0, parseFloat($('#dv-reseg-min').value) || 0) * 1000;
            resegment(chars, minMs);
            $('#dv-reseg-panel').hidden = true;
        });

        // 播放器
        const video = $('#dv-video');
        $('#dv-play').addEventListener('click', togglePlay);
        video.addEventListener('click', togglePlay);
        video.addEventListener('play', () => { $('#dv-player').classList.add('playing'); if (!D.raf) D.raf = requestAnimationFrame(tick); });
        video.addEventListener('pause', () => { $('#dv-player').classList.remove('playing'); updatePlayhead(); });
        video.addEventListener('seeked', updatePlayhead);
        video.addEventListener('loadedmetadata', () => {
            if (!D.project) return;
            if (D.source === 'original' && isFinite(video.duration)) {
                D.project.durationMs = Math.round(video.duration * 1000);
                D.project.size = video.videoWidth ? `${video.videoWidth}×${video.videoHeight}` : '';
                saveProject();
                renderFile();
                renderStepper();
            }
            renderTimeline();
        });
        video.addEventListener('error', () => {
            const msg = $('#dv-player-msg');
            msg.textContent = '预览播放器不支持这个视频格式（不影响识别和配音）';
            msg.hidden = false;
        });
        $('#dv-timeline').addEventListener('click', (e) => {
            const rect = e.currentTarget.getBoundingClientRect();
            const ms = ((e.clientX - rect.left) / rect.width) * totalMs();
            D.playUntil = null;
            video.currentTime = Math.max(0, ms / 1000);
            const block = e.target.closest('i[data-i]');
            if (block) {
                const row = rowEl(Number(block.dataset.i));
                if (row) scrollRowIntoView(row);
            }
        });
        $('#dv-source-switch').addEventListener('click', (e) => {
            const b = e.target.closest('button[data-src]');
            if (!b || b.dataset.src === D.source) return;
            D.source = b.dataset.src;
            renderSourceSwitch();
            loadVideo();
            updatePlayhead();
        });

        // 快捷键：空格播放、Ctrl+Z 撤销（编辑框内交给原生文字撤销）
        document.addEventListener('keydown', (e) => {
            if (!viewActive() || !D.project) return;
            const t = e.target;
            // 按钮聚焦时空格会原生触发点击，这里不再重复处理
            const editing = t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON'].includes(t.tagName);
            if (editing) return;
            if (e.code === 'Space') { e.preventDefault(); togglePlay(); }
            else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') { e.preventDefault(); undo(); }
        });

        // 拖入视频
        const drop = $('#dv-drop');
        if (listen) {
            const on = (name, fn) => listen(name, fn).then(un => V.state.unlisteners.push(un)).catch(() => {});
            on('tauri://drag-enter', () => { if (viewActive()) drop.classList.add('over'); });
            on('tauri://drag-leave', () => drop.classList.remove('over'));
            on('tauri://drag-drop', (e) => {
                drop.classList.remove('over');
                if (!viewActive() || D.running) return;
                const path = ((e.payload && e.payload.paths) || []).find(p => VIDEO_EXT.test(p));
                if (path) confirmReplace(() => openVideo(path));
                else V.showToast('请拖入视频文件', 'warn');
            });
        }
    }

    function bindBackend() {
        document.addEventListener('v2t:view', (e) => {
            if (e.detail !== 'dubbing') {
                const video = $('#dv-video');
                if (video && !video.paused) video.pause();
                return;
            }
            syncStatus();
            renderAll();
            if (D.project) loadVideo();
        });
        document.addEventListener('v2t:config', () => { if (D.project) { renderVoice(); renderSettings(); renderStepper(); } });
        document.addEventListener('v2t:voice', () => { if (D.project) { renderVoice(); renderStepper(); } });
        if (!listen) return;
        const on = (name, fn) => listen(name, fn).then(un => V.state.unlisteners.push(un)).catch(() => {});
        on('dubbing-progress', (e) => onProgress(e.payload));
        on('dubbing-transcript', (e) => onTranscript(e.payload));
        on('dubbing-segment', (e) => onSegment(e.payload));
    }

    /// 切回本页时与后端对齐：任务可能已在别处结束（如窗口隐藏期间）
    async function syncStatus() {
        if (!invoke) return;
        try {
            const busy = await invoke('dubbing_status');
            if (!busy && D.running) setRunning(null);
        } catch (e) {}
    }

    function init() {
        restore();
        bindUi();
        bindList();
        bindBackend();
        renderAll();
    }

    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
    else init();
})();
