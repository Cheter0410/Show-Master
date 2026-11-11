// Playwright smoke test against the mock backend serving ui/dist.
//   npm run build && npm run e2e
// Uses the globally installed Playwright (no dependency in package.json).
import { createRequire } from "node:module";
import { execSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { startMock } from "../mock/server.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SHOTS = join(HERE, "screenshots");
const DIST = join(HERE, "..", "dist");
const PORT = Number(process.env.SM_E2E_PORT ?? 7461);

function loadPlaywright() {
  const require = createRequire(import.meta.url);
  try {
    return require("playwright");
  } catch {
    const root = execSync("npm root -g", { encoding: "utf8" }).trim();
    return require(join(root, "playwright"));
  }
}

let failures = 0;
const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok });
  console.log(`${ok ? "  ok  " : " FAIL "} ${name}${detail ? `  (${detail})` : ""}`);
  if (!ok) failures++;
}
async function step(name, fn) {
  try {
    const detail = await fn();
    check(name, true, typeof detail === "string" ? detail : "");
  } catch (e) {
    check(name, false, String(e?.message ?? e).split("\n")[0]);
  }
}

if (!existsSync(join(DIST, "index.html"))) {
  console.error("ui/dist is missing – run `npm run build` first.");
  process.exit(2);
}
mkdirSync(SHOTS, { recursive: true });

const { chromium } = loadPlaywright();
const mock = await startMock({ port: PORT, dist: DIST, quiet: true });
const ADMIN = `http://127.0.0.1:${PORT}`; // loopback => admin
const REMOTE = `http://localhost:${PORT}`; // mock treats this host as a remote device
const allowedOrigins = new Set([ADMIN, REMOTE, `ws://127.0.0.1:${PORT}`, `ws://localhost:${PORT}`]);
const foreign = [];
const pageErrors = [];

const browser = await chromium.launch();
function watch(page) {
  page.on("request", (r) => {
    const u = r.url();
    if (u.startsWith("data:") || u.startsWith("blob:") || u.startsWith("about:")) return;
    if (!allowedOrigins.has(new URL(u).origin)) foreign.push(u);
  });
  page.on("websocket", (w) => {
    if (!allowedOrigins.has(new URL(w.url()).origin)) foreign.push(w.url());
  });
  page.on("pageerror", (e) => pageErrors.push(String(e)));
  page.on("console", (m) => {
    if (m.type() === "error" && !/Failed to load resource|WebSocket connection/.test(m.text())) pageErrors.push(m.text());
  });
}
const shot = (page, name) => page.screenshot({ path: join(SHOTS, name) });
const noHScroll = (page) => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth && document.body.scrollWidth <= window.innerWidth);
const nextNumber = (page) => page.locator('[data-testid="next-number"]').first().innerText();

try {
  // ------------------------------------------------------------------ desktop
  const desk = await browser.newContext({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });
  const page = await desk.newPage();
  watch(page);
  await page.goto(ADMIN + "/");

  await step("desktop edit loads and lists cues", async () => {
    await page.waitForSelector('[data-testid="cuetable"] [data-cue]');
    const n = await page.locator('[data-testid="cuetable"] [data-cue]').count();
    if (n < 10) throw new Error(`only ${n} rows`);
    return `${n} rows`;
  });

  await step("selecting a cue shows the inspector; an edit round-trips", async () => {
    await page.click('[data-cue="5"] .t-name');
    await page.waitForSelector('[data-testid="insp-name"]');
    await page.fill('[data-testid="insp-name"]', "Overture (edited)");
    await page.press('[data-testid="insp-name"]', "Enter");
    await page.waitForFunction(() => document.querySelector('[data-cue="5"] .t-name')?.textContent?.includes("Overture (edited)"));
    const cue = mock.getState().show.cue_lists[0].cues.find((c) => c.id === 5);
    if (cue.name !== "Overture (edited)") throw new Error("server state not updated");
    await page.waitForSelector('[data-testid="dirty"]');
  });

  await step("invalid timecode input is rejected in the inspector", async () => {
    await page.click('[data-testid="insp-tab-timing"]');
    await page.fill('[data-testid="insp-tc"]', "01:00:00:99");
    await page.press('[data-testid="insp-tc"]', "Enter");
    await page.waitForSelector(".field-error");
    if (mock.getState().show.cue_lists[0].cues.find((c) => c.id === 5).tc_trigger !== null) throw new Error("invalid TC was sent");
    await page.press('[data-testid="insp-tc"]', "Escape");
    await page.click('[data-testid="insp-tab-basic"]');
  });

  await step("undo reverts the edit", async () => {
    await page.click('[data-testid="tb-undo"]');
    await page.waitForFunction(() => !document.querySelector('[data-cue="5"] .t-name')?.textContent?.includes("(edited)"));
  });

  await shot(page, "desktop-edit.png");
  await page.click('[data-testid="insp-tab-kind"]');
  await page.waitForSelector('[data-testid="waveform"]');
  await page.waitForTimeout(400);
  await shot(page, "desktop-edit-audio.png");
  await page.click('[data-cue="9"] .t-name');
  await page.click('[data-testid="insp-tab-routing"]');
  await page.waitForSelector('[data-testid="routing"]');
  await shot(page, "desktop-edit-routing.png");

  await step("setup dialog opens; hotkey conflict is refused", async () => {
    await page.click('[data-testid="tb-setup"]');
    await page.waitForSelector('[data-testid="setup"]');
    await shot(page, "desktop-setup-audio.png");
    await page.click('[data-testid="setup-tab-patch"]');
    await shot(page, "desktop-setup-patch.png");
    await page.click('[data-testid="setup-tab-remote"]');
    await page.waitForTimeout(200);
    await shot(page, "desktop-setup-remote.png");
    await page.click('[data-testid="setup-tab-hotkeys"]');
    await page.click('[data-testid="hk-set-stop_all"]');
    await page.keyboard.press("Space");
    await page.waitForSelector('[data-testid="hk-warning"]');
    if (mock.getState().show.hotkeys.Space !== "go") throw new Error("Space was re-assigned");
    await shot(page, "desktop-setup-hotkeys.png");
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="setup"]', { state: "detached" });
  });

  await step("switching to Live shows the pre-flight result, then the dashboard", async () => {
    await page.click('[data-testid="mode-live"]');
    await page.waitForSelector('[data-testid="preflight"] .pf-error');
    await shot(page, "desktop-preflight.png");
    if (mock.getState().mode !== "edit") throw new Error("went live before confirmation");
    await page.click('[data-testid="preflight-go"]');
    await page.waitForSelector('[data-testid="live-dashboard"]');
    if (await page.locator('[data-testid="inspector"]').count()) throw new Error("inspector visible in live mode");
  });

  await step("GO advances the playhead and the fired cue shows progress", async () => {
    const before = await nextNumber(page);
    await page.click('[data-testid="go"]');
    if (!(await page.locator('[data-testid="go"]').isDisabled())) throw new Error("GO not locked after press");
    await page.waitForFunction((b) => document.querySelector('[data-testid="next-number"]')?.textContent !== b, before);
    await page.waitForSelector('[data-cue="1"][data-state="running"] .lrow-progress');
    const after = await nextNumber(page);
    if (before !== "1" || after !== "2") throw new Error(`playhead ${before} -> ${after}`);
    await page.waitForFunction(() => !document.querySelector('[data-testid="go"]')?.hasAttribute("disabled"));
  });

  await step("Space hotkey fires GO; double press inside the debounce fires once", async () => {
    mock.exec({ cmd: "set_playhead", cue: 5 });
    await page.waitForFunction(() => document.querySelector('[data-testid="next-number"]')?.textContent === "5");
    await page.keyboard.press("Space");
    await page.keyboard.press("Space");
    await page.waitForSelector('[data-cue="5"][data-state="running"]');
    await page.waitForTimeout(350);
    const n = await nextNumber(page);
    if (n !== "6") throw new Error(`playhead at ${n}, expected 6`);
  });

  await step("Space never clicks a focused button", async () => {
    await page.click('[data-testid="pause"]'); // focus stays on the Pause button, show is paused
    await page.waitForFunction(() => /Resume/.test(document.querySelector('[data-testid="pause"]')?.textContent ?? ""));
    await page.keyboard.press("Space"); // must GO (cue 6), not toggle pause again
    await page.waitForFunction(() => document.querySelector('[data-testid="next-number"]')?.textContent === "7");
    if (!mock.getState().paused_all) throw new Error("Space clicked the focused Pause button");
    await page.click('[data-testid="pause"]');
    mock.exec({ cmd: "set_playhead", cue: 6 });
  });

  await page.waitForTimeout(1200);
  await shot(page, "desktop-live.png");

  await step("editing is rejected in live mode (server side)", async () => {
    try {
      mock.exec({ cmd: "add_cue", kind: "memo" });
    } catch (e) {
      if (e.message === "live_mode") return;
    }
    throw new Error("add_cue was accepted in live mode");
  });

  await step("connection-lost banner appears when the WebSocket dies and recovers", async () => {
    mock.dropSockets(1500);
    await page.waitForSelector('[data-testid="conn-banner"]', { timeout: 4000 });
    const text = await page.locator('[data-testid="conn-banner"]').innerText();
    if (!/CONNECTION LOST/.test(text) || !/keeps running/.test(text)) throw new Error("banner text");
    await shot(page, "desktop-connection-lost.png");
    await page.waitForSelector('[data-testid="conn-banner"]', { state: "detached", timeout: 8000 });
    await page.waitForSelector('[data-testid="latency"].sb-ok, [data-testid="latency"].sb-warn');
  });

  await step("Live→Edit asks for confirmation while cues run", async () => {
    await page.click('[data-testid="mode-edit"]');
    await page.waitForSelector('[data-testid="confirm"]');
    if (mock.getState().mode !== "live") throw new Error("left live without confirmation");
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="confirm"]', { state: "detached" });
  });

  // ------------------------------------------------------------------ remote
  const ipad = await browser.newContext({ viewport: { width: 1024, height: 768 }, deviceScaleFactor: 1, hasTouch: true });
  const rp = await ipad.newPage();
  watch(rp);
  await rp.goto(REMOTE + "/remote");

  await step("/remote requires a PIN", async () => {
    await rp.waitForSelector('[data-testid="pinpad"]');
    if (await rp.locator('[data-testid="go"]').count()) throw new Error("GO visible before login");
  });
  await shot(rp, "remote-pin.png");

  await step("wrong PIN is rejected", async () => {
    for (const d of "9999") await rp.tap(`[data-testid="pin-${d}"]`);
    await rp.tap('[data-testid="pin-ok"]');
    await rp.waitForFunction(() => /Wrong PIN/.test(document.querySelector('[data-testid="pin-display"]')?.textContent ?? ""));
    if (await rp.locator('[data-testid="remote-app"]').count()) throw new Error("logged in with a wrong PIN");
  });

  await step("right PIN works and GO works on the remote", async () => {
    for (const d of "1234") await rp.tap(`[data-testid="pin-${d}"]`);
    await rp.tap('[data-testid="pin-ok"]');
    await rp.waitForSelector('[data-testid="remote-app"] [data-cue]');
    const before = await nextNumber(rp);
    await rp.tap('[data-testid="go"]');
    await rp.waitForFunction((b) => document.querySelector('[data-testid="next-number"]')?.textContent !== b, before);
    return `playhead ${before} -> ${await nextNumber(rp)}`;
  });

  await step("remote: select a cue and Set NEXT", async () => {
    await rp.tap('[data-cue="10"]');
    await rp.tap('[data-testid="set-next"]');
    await rp.waitForFunction(() => document.querySelector('[data-testid="next-number"]')?.textContent === "8");
  });

  await step("remote: PANIC needs press-and-hold", async () => {
    const box = await rp.locator('[data-testid="panic"]').boundingBox();
    const running = () => rp.locator('[data-state="running"]').count();
    if ((await running()) === 0) throw new Error("nothing running before the test");
    await rp.tap('[data-testid="panic"]');
    await rp.waitForTimeout(300);
    if ((await running()) === 0) throw new Error("a short tap triggered PANIC");
    const cdp = await ipad.newCDPSession(rp);
    const pt = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [pt] });
    await rp.waitForTimeout(750);
    await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
    await rp.waitForFunction(() => document.querySelectorAll('[data-state="running"]').length === 0);
  });

  await step("remote (control role) cannot edit – server answers forbidden", async () => {
    const token = await rp.evaluate(() => JSON.parse(sessionStorage.getItem("sm.session")).token);
    const res = await rp.evaluate(async (t) => (await fetch(`/api/v1/command?token=${t}`, { method: "POST", body: JSON.stringify({ cmd: "delete_cues", cues: [1] }) })).json(), token);
    if (res.ok !== false || res.error !== "forbidden") throw new Error(JSON.stringify(res));
  });

  // make the screenshots lively again
  mock.exec({ cmd: "set_playhead", cue: 1 });
  await rp.tap('[data-testid="go"]');
  mock.exec({ cmd: "set_playhead", cue: 7 });
  await rp.waitForTimeout(400);
  await rp.tap('[data-testid="go"]');
  await rp.waitForTimeout(2200);
  await step("remote 1024×768: no horizontal scroll", async () => {
    if (!(await noHScroll(rp))) throw new Error("page scrolls horizontally");
  });
  await shot(rp, "remote-1024x768.png");
  await rp.setViewportSize({ width: 768, height: 1024 });
  await rp.waitForTimeout(300);
  await step("remote iPad portrait: no horizontal scroll", async () => {
    if (!(await noHScroll(rp))) throw new Error("page scrolls horizontally");
  });
  await shot(rp, "remote-768x1024.png");
  await rp.setViewportSize({ width: 390, height: 844 });
  await rp.waitForTimeout(300);
  await step("remote 390×844: no horizontal scroll, GO visible", async () => {
    if (!(await noHScroll(rp))) throw new Error("page scrolls horizontally");
    const box = await rp.locator('[data-testid="go"]').boundingBox();
    if (!box || box.y + box.height > 844 || box.height < 56) throw new Error("GO not fully visible");
  });
  await shot(rp, "remote-390x844.png");
  await rp.tap('[data-testid="tab-mix"]');
  await rp.waitForTimeout(500);
  await shot(rp, "remote-390x844-meters.png");
  await rp.tap('[data-testid="tab-cues"]');
  await rp.setViewportSize({ width: 360, height: 640 });
  await rp.waitForTimeout(300);
  await step("remote 360 px wide: no horizontal scroll; touch targets ≥ 56 px", async () => {
    if (!(await noHScroll(rp))) throw new Error("page scrolls horizontally");
    const small = await rp.evaluate(() =>
      [...document.querySelectorAll('[data-testid="transport"] button, .r-tab, .r-selbar button')]
        .map((b) => ({ r: b.getBoundingClientRect(), t: b.textContent }))
        .filter((x) => x.r.height < 55.5)
        .map((x) => `${x.t}:${Math.round(x.r.height)}`),
    );
    if (small.length) throw new Error(`too small: ${small.join(", ")}`);
  });
  await shot(rp, "remote-360x640.png");

  // ------------------------------------------------------------------ view
  const viewCtx = await browser.newContext({ viewport: { width: 1024, height: 768 }, deviceScaleFactor: 1, hasTouch: true });
  const vp = await viewCtx.newPage();
  watch(vp);
  await vp.goto(REMOTE + "/view");
  await step("/view shows the list and contains no controls", async () => {
    await vp.waitForSelector('[data-testid="view-app"] [data-cue]');
    await vp.waitForSelector('[data-testid="next-panel"]');
    const buttons = await vp.locator("button, input, select, textarea, [role=button]").count();
    if (buttons !== 0) throw new Error(`${buttons} interactive controls on /view`);
    for (const id of ["go", "panic", "pause", "stop-all", "transport", "master"]) {
      if (await vp.locator(`[data-testid="${id}"]`).count()) throw new Error(`${id} present on /view`);
    }
  });
  await step("/view (view role) cannot send commands – server answers forbidden", async () => {
    const token = await vp.evaluate(() => JSON.parse(sessionStorage.getItem("sm.session")).token);
    const res = await vp.evaluate(async (t) => (await fetch(`/api/v1/command?token=${t}`, { method: "POST", body: JSON.stringify({ cmd: "go" }) })).json(), token);
    if (res.ok !== false || res.error !== "forbidden") throw new Error(JSON.stringify(res));
  });
  await vp.waitForTimeout(300);
  await shot(vp, "view.png");
  await vp.setViewportSize({ width: 390, height: 844 });
  await vp.waitForTimeout(300);
  await step("/view on a phone: no horizontal scroll", async () => {
    if (!(await noHScroll(vp))) throw new Error("page scrolls horizontally");
  });
  await shot(vp, "view-390x844.png");

  // ------------------------------------------------------------------ extra: kick + high contrast
  await step("kick_client disconnects the remote and tells the user", async () => {
    const id = mock.getState().remote.clients.find((c) => c.role === "control")?.id;
    mock.exec({ cmd: "kick_client", id });
    await rp.waitForFunction(() => /DISCONNECTED BY THE OPERATOR/.test(document.querySelector('[data-testid="conn-banner"]')?.textContent ?? ""));
  });
  await step("high contrast setting switches the theme", async () => {
    mock.exec({ cmd: "set_mode", mode: "edit", force: true });
    mock.exec({ cmd: "update_show", patch: { settings: { high_contrast: true } } });
    await page.waitForFunction(() => document.documentElement.getAttribute("data-contrast") === "high");
    mock.exec({ cmd: "set_mode", mode: "live" });
    await page.waitForSelector('[data-testid="live-dashboard"]');
    await page.waitForTimeout(300);
    await shot(page, "desktop-live-high-contrast.png");
  });

  // ------------------------------------------------------------------ global assertions
  check("no request to any origin other than the mock", foreign.length === 0, foreign.slice(0, 3).join(" "));
  check("no page errors", pageErrors.length === 0, pageErrors.slice(0, 3).join(" | "));
} finally {
  await browser.close();
  await mock.close();
}

console.log(`\n${results.length - failures}/${results.length} checks passed. Screenshots: ${SHOTS}`);
process.exit(failures ? 1 : 0);
