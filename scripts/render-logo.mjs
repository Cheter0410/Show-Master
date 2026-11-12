// Renders the app icons from assets/logo.svg (and assets/logo-fullbleed.svg for
// the web's maskable icons) with the Playwright Chromium:
//   assets/icon.png (1024, transparent corners), assets/icon.ico (16-256),
//   ui/public/icon.svg, icon-192.png, icon-512.png, apple-touch-icon.png
// Run after changing the logo:  node scripts/render-logo.mjs   (needs python3 + Pillow for the .ico)
import { createRequire } from "node:module";
import { readFileSync, writeFileSync, copyFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { chromium } = require("playwright");
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const svg = (f) => readFileSync(join(root, "assets", f)).toString("base64");

const browser = await chromium.launch();
async function render(file, size, out) {
  const page = await browser.newPage({ viewport: { width: size, height: size } });
  await page.setContent(
    `<html><body style="margin:0;background:transparent"><img style="display:block" width="${size}" height="${size}" src="data:image/svg+xml;base64,${svg(file)}"></body></html>`,
  );
  await page.screenshot({ path: out, omitBackground: true });
  await page.close();
}
await render("logo.svg", 1024, join(root, "assets/icon.png"));
for (const [name, size] of [["icon-192.png", 192], ["icon-512.png", 512], ["apple-touch-icon.png", 180]]) {
  await render("logo-fullbleed.svg", size, join(root, "ui/public", name));
}
await browser.close();
copyFileSync(join(root, "assets/logo.svg"), join(root, "ui/public/icon.svg"));
execFileSync("python3", [
  "-c",
  "import sys; from PIL import Image; Image.open(sys.argv[1]).save(sys.argv[2], sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])",
  join(root, "assets/icon.png"),
  join(root, "assets/icon.ico"),
]);
console.log("icons written");
