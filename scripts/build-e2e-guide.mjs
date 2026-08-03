// Render artifacts/e2e-guide.html -> artifacts/ProofKiosk-E2E-Demo-Guide.pdf
// Same toolchain as build-handbook.mjs (Playwright Chromium, print CSS).
//   node scripts/build-e2e-guide.mjs
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { chromium } from 'playwright';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const src = path.join(root, 'artifacts', 'e2e-guide.html');
const out = path.join(root, 'artifacts', 'ProofKiosk-E2E-Demo-Guide.pdf');

const browser = await chromium.launch();
const page = await browser.newPage();
await page.goto(pathToFileURL(src).href, { waitUntil: 'load' });
await page.emulateMedia({ media: 'print' });
await page.pdf({
  path: out,
  format: 'A4',
  printBackground: true,
  displayHeaderFooter: true,
  headerTemplate: '<div></div>',
  footerTemplate:
    '<div style="width:100%;font:8pt -apple-system,sans-serif;color:#8a8578;' +
    'padding:0 12mm;display:flex;justify-content:space-between">' +
    '<span>ProofKiosk — E2E Test &amp; Demo Guide</span>' +
    '<span class="pageNumber"></span></div>',
  margin: { top: '13mm', bottom: '14mm', left: '12mm', right: '12mm' },
});
await browser.close();
console.log(`wrote ${path.relative(root, out)}`);
