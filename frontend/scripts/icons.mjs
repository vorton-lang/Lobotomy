// Rasterizes public/icons/lobotomy.svg into the PNG and ICO files beside it, with the browser the
// e2e tests use (playwright.config.ts). Run `npm run icons:generate` after changing the SVG.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from '@playwright/test';

const directory = fileURLToPath(new URL('../public/icons/', import.meta.url));
const sizes = [16, 24, 32, 48, 256];
const source = await fs.readFile(path.join(directory, 'lobotomy.svg'), 'utf8');
const url = `data:image/svg+xml;base64,${Buffer.from(source).toString('base64')}`;
const browser = await chromium.launch({ channel: process.platform === 'win32' ? 'msedge' : undefined });
try {
  const page = await browser.newPage({ deviceScaleFactor: 1 });
  const frames = [];
  for (const size of sizes) {
    const data = await page.evaluate(
      async ({ url, size }) => {
        const image = new Image();
        image.src = url;
        await image.decode();
        const canvas = document.createElement('canvas');
        canvas.width = canvas.height = size;
        canvas.getContext('2d').drawImage(image, 0, 0, size, size);
        return canvas.toDataURL('image/png').split(',')[1];
      },
      { url, size },
    );
    frames.push(Buffer.from(data, 'base64'));
    await fs.writeFile(path.join(directory, `lobotomy-${size}.png`), frames.at(-1));
  }

  // An ICO directory of PNG frames (Windows Vista and later); a size of 0 means 256 px.
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
  await fs.writeFile(path.join(directory, 'lobotomy.ico'), Buffer.concat([header, ...frames]));
} finally {
  await browser.close();
}
