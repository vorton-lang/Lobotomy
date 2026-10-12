// One SVG source; rasterize with the Playwright browser already used by the project.
// No image editor, image library or packaging tool is needed.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const root = fileURLToPath(new URL('../', import.meta.url));
const directory = path.join(root, 'public', 'icons');
const sizes = [16, 24, 32, 48, 256];
const source = await fs.readFile(path.join(directory, 'lobotomy.svg'), 'utf8');
const url = `data:image/svg+xml;base64,${Buffer.from(source).toString('base64')}`;
const check = process.argv.includes('--check');
const browser = await chromium.launch({
  // Optional local browser; otherwise use the same browser as the project's e2e tests.
  executablePath: process.env.ICON_CHROMIUM_PATH,
  channel: !process.env.ICON_CHROMIUM_PATH && process.platform === 'win32' ? 'msedge' : undefined,
});
try {
  const page = await browser.newPage({ deviceScaleFactor: 1 });
  const frames = [];
  for (const size of sizes) {
    const data = await page.evaluate(async ({ url, size }) => {
      const image = new Image();
      image.src = url;
      await image.decode();
      const canvas = document.createElement('canvas');
      canvas.width = canvas.height = size;
      canvas.getContext('2d').drawImage(image, 0, 0, size, size);
      return canvas.toDataURL('image/png').split(',')[1];
    }, { url, size });
    frames.push(Buffer.from(data, 'base64'));
  }

  // ICO directory with PNG frames (Windows Vista and later). 0 encodes 256 px.
  const header = Buffer.alloc(6 + sizes.length * 16);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(sizes.length, 4);
  let offset = header.length;
  sizes.forEach((size, i) => {
    const entry = 6 + i * 16;
    header[entry] = header[entry + 1] = size % 256;
    header.writeUInt16LE(1, entry + 4);
    header.writeUInt16LE(32, entry + 6);
    header.writeUInt32LE(frames[i].length, entry + 8);
    header.writeUInt32LE(offset, entry + 12);
    offset += frames[i].length;
  });
  const outputs = sizes.map((size, i) => [`lobotomy-${size}.png`, frames[i]]);
  outputs.push(['lobotomy.ico', Buffer.concat([header, ...frames])]);
  for (const [name, bytes] of outputs) {
    const destination = path.join(directory, name);
    if (check) assert.ok((await fs.readFile(destination)).equals(bytes), `${name} is stale; run npm run icons:generate`);
    else await fs.writeFile(destination, bytes);
  }
  console.log(`${check ? 'Checked' : 'Generated'} ${outputs.length} icon assets from lobotomy.svg`);

  if (process.argv.includes('--preview')) {
    const raster = (size) => `data:image/png;base64,${frames[sizes.indexOf(size)].toString('base64')}`;
    const row = (background, foreground, label, monochrome = false) => `
      <section style="background:${background};color:${foreground}">
        <div class="label">${label}</div>
        <div class="samples" style="${monochrome ? 'filter:grayscale(1)' : ''}">
          ${[256, 16, 24, 32, 48].map((size) => `<figure><img src="${raster(size)}" width="${size}" height="${size}"><figcaption>${size} px</figcaption></figure>`).join('')}
        </div>
      </section>`;
    await page.setViewportSize({ width: 1040, height: 980 });
    await page.setContent(`<!doctype html><style>
      *{box-sizing:border-box}body{margin:0;padding:32px;background:#e5e9eb;font-family:Arial,sans-serif;color:#242b32}
      h1{font-size:26px;letter-spacing:2px;margin:0 0 8px}p{font-size:14px;margin:0 0 24px;color:#52616b}
      section{padding:20px 28px;margin-bottom:16px;border-radius:16px}.label{font-size:12px;letter-spacing:2px}
      .samples{display:flex;align-items:center;justify-content:space-between;min-height:284px;gap:28px}
      figure{margin:0;min-width:72px;text-align:center}img{display:block;margin:auto}figcaption{font-size:12px;margin-top:12px;opacity:.7}
      section:last-child .samples{min-height:90px}section:last-child figure:first-child{display:none}
    </style><h1>LOBOTOMY / OPEN DOOR</h1><p>Original vector mark · cool white L · integrated teal node · transparent corners</p>
    ${row('#f8fafb', '#242b32', 'LIGHT / ACTUAL PIXEL SIZES')}
    ${row('#11171d', '#edf3f5', 'DARK / ACTUAL PIXEL SIZES')}
    ${row('#c9d0d5', '#242b32', 'MONOCHROME / ACTUAL PIXEL SIZES', true)}`);
    await page.evaluate(() => Promise.all([...document.images].map((image) => image.decode())));
    await page.screenshot({ path: path.join(root, '..', 'notes', 'assets', 'app-icon-preview.png'), fullPage: true });
  }
} finally {
  await browser.close();
}
