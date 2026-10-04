// 渲染短视频封面：node cover.mjs → out/cover-*.png
import { chromium } from '/opt/node22/lib/node_modules/playwright/index.mjs';
import path from 'node:path';
const sizes = [['9x16', 1080, 1920], ['3x4', 1080, 1440], ['16x9', 1920, 1080]];
const browser = await chromium.launch();
for (const [name, w, h] of sizes) {
  const page = await browser.newPage({ viewport: { width: w, height: h } });
  await page.goto('file://' + path.resolve('cover.html') + `?w=${w}&h=${h}`);
  await page.waitForSelector('body[data-ready="1"]');
  await page.waitForTimeout(300);
  await page.screenshot({ path: `out/Voice2Type-cover-${name}.png` });
  await page.close();
}
await browser.close();
