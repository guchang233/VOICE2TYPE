// 宣传片渲染器：逐帧截图 film.html → ffmpeg 编码 → 混入配乐
//
// 用法（仓库根目录执行）：
//   node promo/render.mjs                        # 竖屏 1080x1920 @60fps
//   node promo/render.mjs --landscape            # 横屏 1920x1080
//   node promo/render.mjs --stills 3.5,12,20     # 仅导出若干静帧用于预览
//   node promo/render.mjs --fps 30 --from 10 --to 20
//
// 依赖：Node 18+、playwright（含 Chromium）、ffmpeg、python3 + numpy（配乐）、
//       字体 Noto Sans CJK SC / Inter / JetBrains Mono（见 promo/README.md）
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

let chromium;
try {
    ({ chromium } = await import('playwright'));
} catch {
    ({ chromium } = await import('/opt/node22/lib/node_modules/playwright/index.mjs'));
}

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const outDir = path.join(here, 'out');
const args = process.argv.slice(2);
const opt = (name, dflt) => {
    const i = args.indexOf('--' + name);
    return i >= 0 ? args[i + 1] : dflt;
};
const flag = name => args.includes('--' + name);

const landscape = flag('landscape');
const W = landscape ? 1920 : 1080;
const H = landscape ? 1080 : 1920;
const fps = +opt('fps', 60);
const tag = landscape ? '16x9' : '9x16';
fs.mkdirSync(path.join(outDir, 'assets'), { recursive: true });

// ---------- 静态服务 ----------
const types = { '.html': 'text/html; charset=utf-8', '.css': 'text/css', '.js': 'application/javascript', '.png': 'image/png', '.svg': 'image/svg+xml', '.json': 'application/json' };
const server = http.createServer((req, res) => {
    const u = decodeURIComponent(req.url.split('?')[0]);
    const p = path.join(root, u);
    if (!p.startsWith(root)) { res.writeHead(403); return res.end(); }
    fs.readFile(p, (err, data) => {
        if (err) { res.writeHead(404); return res.end(); }
        res.writeHead(200, { 'Content-Type': types[path.extname(p)] || 'application/octet-stream' });
        res.end(data);
    });
}).listen(0);
const base = `http://127.0.0.1:${server.address().port}`;

const browser = await chromium.launch({ args: ['--force-color-profile=srgb', '--disable-lcd-text', '--font-render-hinting=none'] });

// ---------- 主题截图（S7 场景使用） ----------
async function makeThemeAssets() {
    const ctx = await browser.newContext({ viewport: { width: 880, height: 720 }, deviceScaleFactor: 2 });
    const page = await ctx.newPage();
    for (const theme of ['dark', 'light', 'eye-care']) {
        await page.goto(`${base}/src/index.html`);
        await page.evaluate((th) => {
            document.documentElement.setAttribute('data-theme', th);
            const st = document.createElement('style');
            st.textContent = '*,*::before,*::after{transition:none!important} .view.active .view-body,.view.active .view-header{animation:none!important}';
            document.head.appendChild(st);
            document.getElementById('dictation-output').textContent = '明天上午十点，和设计团队过一下新版首页的方案。';
            const s = document.querySelector('#dictation-status .status-dot');
            s.className = 'status-dot ready';
            document.querySelector('#dictation-status .status-text').textContent = '识别完成';
        }, theme);
        await page.waitForTimeout(500);
        await page.screenshot({ path: path.join(outDir, 'assets', `app-${theme}.png`) });
    }
    await ctx.close();
}
await makeThemeAssets();

// ---------- 打开影片 ----------
const ctx = await browser.newContext({ viewport: { width: W, height: H }, deviceScaleFactor: 1 });
const page = await ctx.newPage();
const pageErrors = [];
page.on('pageerror', e => pageErrors.push(e.message));
await page.goto(`${base}/promo/film.html?w=${W}&h=${H}`);
await page.evaluate(() => window.__ready);
const duration = await page.evaluate(() => window.__duration);
const events = await page.evaluate(() => window.__events);
const eventsFile = path.join(outDir, `events-${tag}.json`);
fs.writeFileSync(eventsFile, JSON.stringify({ duration, events }, null, 1));

async function frameAt(t) {
    await page.evaluate(tt => window.__render(tt), t);
    return page.screenshot({ type: 'png' });
}

const stills = opt('stills');
if (stills) {
    const dir = path.join(outDir, `stills-${tag}`);
    fs.mkdirSync(dir, { recursive: true });
    // 场景内动画依赖逐帧推进：从场景起点按 30fps 推进到目标时间，保证与正式渲染一致
    const targets = stills.split(',').map(Number).sort((a, b) => a - b);
    let t = Math.max(0, targets[0] - 2.5);
    for (const target of targets) {
        if (t > target) t = Math.max(0, target - 2.5);
        while (t < target - 1e-6) { await page.evaluate(tt => window.__render(tt), t); t += 1 / 30; }
        const png = await frameAt(target);
        fs.writeFileSync(path.join(dir, `t${target.toFixed(2).padStart(6, '0')}.png`), png);
        t = target;
    }
    console.log('stills ->', dir);
    if (pageErrors.length) console.log('page errors:\n' + pageErrors.join('\n'));
    await browser.close();
    server.close();
    process.exit(0);
}

// ---------- 逐帧渲染 → ffmpeg ----------
const from = +opt('from', 0);
const to = +opt('to', duration);
const silent = path.join(outDir, `video-${tag}.mp4`);
const ff = spawn('ffmpeg', [
    '-y', '-loglevel', 'error',
    '-f', 'image2pipe', '-framerate', String(fps), '-c:v', 'png', '-i', '-',
    '-c:v', 'libx264', '-preset', 'slow', '-crf', '15', '-tune', 'film',
    '-pix_fmt', 'yuv420p', '-colorspace', 'bt709', '-color_primaries', 'bt709', '-color_trc', 'bt709',
    '-movflags', '+faststart', silent,
], { stdio: ['pipe', 'inherit', 'inherit'] });

const total = Math.round((to - from) * fps);
const t0 = Date.now();
for (let f = 0; f < total; f++) {
    const t = from + f / fps;
    const png = await frameAt(t);
    if (!ff.stdin.write(png)) await new Promise(r => ff.stdin.once('drain', r));
    if (f % (fps * 2) === 0) {
        const el = (Date.now() - t0) / 1000;
        process.stdout.write(`\r[${tag}] ${t.toFixed(1)}s / ${to}s  ${(f / Math.max(el, 0.001)).toFixed(1)} fps  `);
    }
}
ff.stdin.end();
await new Promise(r => ff.on('close', r));
console.log(`\n[${tag}] video done in ${((Date.now() - t0) / 1000).toFixed(0)}s`);
if (pageErrors.length) console.log('page errors:\n' + pageErrors.join('\n'));
await browser.close();
server.close();

// ---------- 配乐 + 混流 ----------
const wav = path.join(outDir, `soundtrack-${tag}.wav`);
if (!fs.existsSync(wav) || flag('rebuild-audio')) {
    const r = spawnSync('python3', [path.join(here, 'soundtrack.py'), eventsFile, wav], { stdio: 'inherit' });
    if (r.status !== 0) throw new Error('soundtrack.py failed');
}
if (from === 0 && Math.abs(to - duration) < 1e-6) {
    const final = path.join(outDir, `Voice2Type-launch-${tag}.mp4`);
    const r = spawnSync('ffmpeg', ['-y', '-loglevel', 'error', '-i', silent, '-i', wav,
        '-c:v', 'copy', '-c:a', 'aac', '-b:a', '256k', '-ar', '48000', '-shortest', '-movflags', '+faststart', final], { stdio: 'inherit' });
    if (r.status !== 0) throw new Error('mux failed');
    console.log('final ->', final);
}
