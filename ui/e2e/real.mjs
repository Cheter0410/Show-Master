// End-to-end test of the real application: starts the `showmaster` binary
// with a generated demo show (null audio output is fine) and drives the
// embedded web UI with Playwright - desktop on loopback (admin), remote and
// view through the machine's LAN address (PIN login, server-side roles).
//
//   cargo build -p showmaster && node ui/e2e/real.mjs
import { createRequire } from "node:module";
import { execSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const SHOTS = join(HERE, "screenshots");
const PORT = Number(process.env.SM_E2E_PORT ?? 7462);
const BIN = process.env.SM_BIN ?? join(ROOT, "target", "debug", process.platform === "win32" ? "showmaster.exe" : "showmaster");

function loadPlaywright() {
  const require = createRequire(import.meta.url);
  try {
    return require("playwright");
  } catch {
    return require(join(execSync("npm root -g", { encoding: "utf8" }).trim(), "playwright"));
  }
}

let failures = 0;
async function step(name, fn) {
  try {
    const detail = await fn();
    console.log(`  ok   ${name}${typeof detail === "string" ? `  (${detail})` : ""}`);
  } catch (e) {
    failures++;
    console.log(` FAIL  ${name}  (${String(e?.message ?? e).split("\n")[0]})`);
  }
}

if (!existsSync(BIN)) {
  console.error(`${BIN} is missing - run \`cargo build -p showmaster\` (after \`npm run build\` in ui/).`);
  process.exit(2);
}
mkdirSync(SHOTS, { recursive: true });
const work = mkdtempSync(join(tmpdir(), "sm-e2e-"));
const app = spawn(BIN, ["--headless", "--make-demo", join(work, "show"), "--port", String(PORT), "--log-dir", join(work, "logs")], { stdio: "ignore" });
const LOCAL = `http://127.0.0.1:${PORT}`;
const api = async (path, init) => (await fetch(LOCAL + path, init)).json();
const command = (cmd) => api("/api/v1/command", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(cmd) });

let browser;
try {
  for (let i = 0; i < 100; i++) {
    try {
      await api("/api/session");
      break;
    } catch {
      await new Promise((r) => setTimeout(r, 100));
    }
  }
  const state = await api("/api/v1/state");
  const LAN = state.remote.urls[0];
  const { chromium } = loadPlaywright();
  // Talk to the app directly: a system HTTP proxy must not sit between the
  // browser and the local server (some proxies do not pass WebSocket upgrades).
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/proxy/i.test(k)));
  browser = await chromium.launch({ args: ["--no-proxy-server", "--proxy-bypass-list=*"], env });
  const foreign = new Set();
  const watch = (page) => page.on("request", (r) => {
    const origin = new URL(r.url()).origin;
    if (origin !== LOCAL && origin !== LAN && !r.url().startsWith("data:") && !r.url().startsWith("blob:")) foreign.add(origin);
  });
  const nextNumber = (page) => page.locator('[data-testid="next-number"]').first().innerText();

  // ---- desktop (loopback = admin) ----------------------------------------
  const desk = await browser.newContext({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });
  const page = await desk.newPage();
  watch(page);
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.goto(LOCAL + "/");

  await step("desktop loads the demo show from the real engine", async () => {
    await page.waitForSelector('[data-testid="cuetable"] [data-cue]');
    const n = await page.locator('[data-testid="cuetable"] [data-cue]').count();
    if (n < 11) throw new Error(`only ${n} rows`);
    await page.getByText("Intro pad").first().waitFor();
    return `${n} cue rows`;
  });

  await step("inspector edit round-trips through the server and can be undone", async () => {
    await page.getByText("Intro pad").first().click();
    await page.waitForSelector('[data-testid="insp-name"]');
    await page.fill('[data-testid="insp-name"]', "Intro pad (edited)");
    await page.keyboard.press("Enter");
    await page.getByText("Intro pad (edited)").first().waitFor();
    const s = await api("/api/v1/state");
    if (s.show.cue_lists[0].cues[0].name !== "Intro pad (edited)") throw new Error("server did not get the edit");
    if (!s.dirty || !s.can_undo) throw new Error("dirty/undo flags not set");
    await page.click('[data-testid="tb-undo"]');
    await page.waitForFunction(() => !document.body.innerText.includes("Intro pad (edited)"));
  });

  await step("audio inspector shows the waveform of the real file", async () => {
    await page.click('[data-testid="insp-tab-kind"]');
    await page.waitForSelector("canvas");
    const wave = await api("/api/waveform?file=media/intro.wav&buckets=100");
    if (wave.peaks.length !== 100 || !(wave.duration > 11.9)) throw new Error("waveform endpoint");
    await page.screenshot({ path: join(SHOTS, "real-desktop-edit.png") });
  });

  await step("Delete button and Backspace delete the selected cue on the real engine; undo restores it", async () => {
    const count = async () => (await api("/api/v1/state")).show.cue_lists[0].cues.length;
    const before = await count();
    const id = (await api("/api/v1/state")).show.cue_lists[0].cues.at(-1).id;
    await page.click(`[data-cue="${id}"] .t-name`);
    await page.click('[data-testid="tb-delete"]');
    await page.waitForSelector(`[data-cue="${id}"]`, { state: "detached" });
    if ((await count()) !== before - 1) throw new Error("server did not delete the cue");
    await page.waitForSelector('[data-testid="toast"]:has-text("Deleted 1 cue")');
    await page.click('[data-testid="tb-undo"]');
    await page.waitForSelector(`[data-cue="${id}"]`);
    await page.click(`[data-cue="${id}"] .t-name`);
    await page.keyboard.press("Backspace");
    await page.waitForSelector(`[data-cue="${id}"]`, { state: "detached" });
    if ((await count()) !== before - 1) throw new Error("Backspace did not delete the cue");
    await page.keyboard.press("Control+KeyZ");
    await page.waitForSelector(`[data-cue="${id}"]`);
    if ((await count()) !== before) throw new Error("undo did not restore the cue");
  });

  await step("setup dialog opens with real devices and the QR code", async () => {
    await page.click('[data-testid="tb-setup"]');
    await page.waitForFunction(() => document.body.textContent.includes("Null output (no sound)"));
    await page.click('[data-testid="setup-tab-remote"]');
    await page.waitForSelector('img[src*="qr.svg"]');
    const ok = await page.evaluate(async () => {
      const img = document.querySelector('img[src*="qr.svg"]');
      await img.decode();
      return img.naturalWidth > 0;
    });
    if (!ok) throw new Error("QR image did not load");
    await page.screenshot({ path: join(SHOTS, "real-setup-remote.png") });
    await page.keyboard.press("Escape");
  });

  await step("Edit -> Live shows the pre-flight result, then the dashboard", async () => {
    await page.click('[data-testid="mode-live"]');
    await page.getByText(/No audio device is open/).first().waitFor();
    await page.click('[data-testid="preflight-go"]');
    await page.waitForSelector('[data-testid="go"]');
    await page.waitForFunction(() => !document.querySelector('[data-testid="inspector"]'), null, { timeout: 5000 });
    const r = await command({ cmd: "delete_cues", cues: [2] });
    if (r.ok !== false || r.error !== "live_mode") throw new Error("server accepted an edit in live mode");
  });

  await step("GO fires on the engine: playhead moves, cue runs, clocks tick", async () => {
    const before = await nextNumber(page);
    await page.click('[data-testid="go"]');
    await page.waitForFunction((b) => document.querySelector('[data-testid="next-number"]')?.textContent !== b, before);
    await page.waitForSelector('[data-state="running"]');
    const s = await api("/api/v1/state");
    if (!s.tick.active.some((a) => a.phase === "running")) throw new Error("engine reports no running cue");
    if (!(s.tick.show_elapsed >= 0)) throw new Error("show clock not started");
    await page.waitForTimeout(700);
    await page.screenshot({ path: join(SHOTS, "real-desktop-live.png") });
    return `next ${before} -> ${await nextNumber(page)}, voices ${s.tick.voices}`;
  });

  await step("Space is GO, Escape is PANIC (hotkeys from the show)", async () => {
    await page.waitForTimeout(400); // GO lock
    const before = await nextNumber(page);
    await page.locator("body").click({ position: { x: 5, y: 5 } });
    await page.keyboard.press("Space");
    await page.waitForFunction((b) => document.querySelector('[data-testid="next-number"]')?.textContent !== b, before);
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => !document.querySelector('[data-state="running"]'));
    const s = await api("/api/v1/state");
    if (s.tick.active.length !== 0) throw new Error("cues still active after panic");
  });

  // ---- remote + view through the LAN address (not loopback => PIN) --------
  if (!LAN) {
    console.log(" skip  remote/view tests: this machine has no LAN address");
  } else {
    const tablet = await browser.newContext({ viewport: { width: 1024, height: 768 }, deviceScaleFactor: 1, hasTouch: true });
    const rp = await tablet.newPage();
    watch(rp);
    await rp.goto(LAN + "/remote");
    await step("/remote from another address needs the PIN; a wrong one is refused", async () => {
      await rp.waitForSelector('[data-testid="pinpad"]');
      if (await rp.locator('[data-testid="go"]').count()) throw new Error("GO visible before login");
      for (const d of "9999") await rp.tap(`[data-testid="pin-${d}"]`);
      await rp.tap('[data-testid="pin-ok"]');
      await rp.waitForFunction(() => /Wrong PIN/.test(document.querySelector('[data-testid="pin-display"]')?.textContent ?? ""));
    });
    await step("right PIN: remote GO drives the engine", async () => {
      for (const d of "1234") await rp.tap(`[data-testid="pin-${d}"]`);
      await rp.tap('[data-testid="pin-ok"]');
      await rp.waitForSelector('[data-testid="remote-app"] [data-cue]', { timeout: 8000 }).catch(async (e) => {
        await rp.screenshot({ path: join(SHOTS, "real-remote-fail.png") });
        throw e;
      });
      const before = await nextNumber(rp);
      await rp.tap('[data-testid="go"]');
      await rp.waitForFunction((b) => document.querySelector('[data-testid="next-number"]')?.textContent !== b, before);
      await rp.waitForTimeout(500);
      await rp.screenshot({ path: join(SHOTS, "real-remote.png") });
      const s = await api("/api/v1/state");
      const roles = s.remote.clients.map((c) => c.role).sort().join(",");
      if (roles !== "admin,control") throw new Error(`clients: ${roles}`);
      return `clients: ${roles}`;
    });
    await step("the desktop sees what the remote did", async () => {
      const a = await nextNumber(page);
      const b = await nextNumber(rp);
      if (a !== b) throw new Error(`desktop next ${a}, remote next ${b}`);
    });
    await step("control role cannot edit (enforced by the server)", async () => {
      const token = await rp.evaluate(() => Object.values(sessionStorage).map((v) => { try { return JSON.parse(v); } catch { return v; } }).map((v) => (typeof v === "string" ? v : v?.token)).find((t) => typeof t === "string" && t.length >= 16));
      if (!token) throw new Error("no token in sessionStorage");
      const res = await fetch(`${LAN}/api/v1/command?token=${token}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ cmd: "update_show", patch: { name: "x" } }) });
      const body = await res.json();
      if (res.status !== 403 || body.error !== "forbidden") throw new Error(`status ${res.status} ${JSON.stringify(body)}`);
      const fs = await fetch(`${LAN}/api/fs?token=${token}`);
      if (fs.status !== 401) throw new Error(`file browser open to control: ${fs.status}`);
    });

    const viewer = await browser.newContext({ viewport: { width: 1024, height: 768 }, deviceScaleFactor: 1 });
    const vp = await viewer.newPage();
    watch(vp);
    await vp.goto(LAN + "/view");
    await step("/view shows the list and has no controls", async () => {
      await vp.waitForSelector("[data-cue]");
      for (const id of ["go", "panic", "pause", "stop-all", "set-next"]) {
        if (await vp.locator(`[data-testid="${id}"]`).count()) throw new Error(`${id} present on /view`);
      }
      await vp.screenshot({ path: join(SHOTS, "real-view.png") });
    });

    const phone = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 1, hasTouch: true, isMobile: true });
    const pp = await phone.newPage();
    watch(pp);
    await step("bare address opens the view; Control asks for the PIN, View goes back", async () => {
      await pp.goto(LAN + "/");
      await pp.waitForSelector('[data-testid="view-app"]');
      if (!pp.url().endsWith("/view")) throw new Error(`landed on ${pp.url()}`);
      await pp.tap('[data-testid="to-remote"]');
      await pp.waitForSelector('[data-testid="pinpad"]');
      for (const d of "1234") await pp.tap(`[data-testid="pin-${d}"]`);
      await pp.tap('[data-testid="pin-ok"]');
      await pp.waitForSelector('[data-testid="remote-app"]');
      await pp.tap('[data-testid="to-view"]');
      await pp.waitForSelector('[data-testid="view-app"]');
      await pp.screenshot({ path: join(SHOTS, "real-phone-view.png") });
    });
  }

  await step("no request left the machine / local network", async () => {
    if (foreign.size) throw new Error([...foreign].join(", "));
  });
  await step("no uncaught errors in the page", async () => {
    if (errors.length) throw new Error(errors[0]);
  });
} finally {
  await browser?.close();
  app.kill("SIGINT");
  await new Promise((r) => setTimeout(r, 300));
  rmSync(work, { recursive: true, force: true });
}
console.log(failures ? `\n${failures} step(s) FAILED` : "\nall steps passed");
process.exit(failures ? 1 : 0);
