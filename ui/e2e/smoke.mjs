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
const hasCue = (id) => {
  const walk = (cues) => cues.some((c) => c.id === id || (c.kind.type === "group" && walk(c.kind.children)));
  return mock.getState().show.cue_lists.some((l) => walk(l.cues));
};
/** testids that are not fully inside the viewport (or missing / hidden) */
const outside = (page, ids) =>
  page.evaluate((ids) => {
    const bad = [];
    for (const id of ids) {
      const el = [...document.querySelectorAll(`[data-testid="${id}"]`)].find((e) => e.getClientRects().length > 0);
      const r = el?.getBoundingClientRect();
      if (!r || r.width === 0 || r.left < 0 || r.top < 0 || r.right > innerWidth + 0.5 || r.bottom > innerHeight + 0.5) bad.push(id);
    }
    return bad;
  }, ids);
/** visible buttons / tabs smaller than `min` px in either direction */
const tinyTargets = (page, selector, min) =>
  page.evaluate(
    ([sel, min]) =>
      [...document.querySelectorAll(sel)]
        .filter((b) => b.getClientRects().length > 0)
        .map((b) => ({ r: b.getBoundingClientRect(), t: (b.getAttribute("data-testid") || b.getAttribute("aria-label") || b.textContent || "").trim().slice(0, 20) }))
        .filter((x) => x.r.height < min - 0.5 || x.r.width < min - 0.5)
        .map((x) => `${x.t}:${Math.round(x.r.width)}x${Math.round(x.r.height)}`),
    [selector, min],
  );
async function longPress(ctx, page, selector, ms = 800) {
  const box = await page.locator(selector).first().boundingBox();
  const cdp = await ctx.newCDPSession(page);
  const pt = { x: box.x + Math.min(box.width / 2, 160), y: box.y + box.height / 2 };
  await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [pt] });
  await page.waitForTimeout(ms);
  await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  await cdp.detach();
}

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

  await step("Delete button: disabled without selection, deletes the selection, toast offers undo", async () => {
    await page.evaluate(() => document.querySelector('[data-testid="cuetable"] .tbody')?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    if (!(await page.locator('[data-testid="tb-delete"]').isDisabled())) throw new Error("Delete enabled without a selection");
    await page.click('[data-cue="8"] .t-name');
    await page.click('[data-testid="tb-delete"]');
    await page.waitForSelector('[data-cue="8"]', { state: "detached" });
    if (hasCue(8)) throw new Error("cue 8 still on the server");
    const t = await page.locator('[data-testid="toast"]', { hasText: "Deleted 1 cue" }).innerText();
    if (!/undo with Ctrl\/Cmd\+Z/.test(t)) throw new Error(`toast: ${t}`);
    await page.click('[data-testid="toast-action"]');
    await page.waitForSelector('[data-cue="8"]');
    if (!hasCue(8)) throw new Error("undo from the toast did not restore cue 8");
  });

  await step("Backspace deletes the selected cue (Mac delete key); Cmd+Z restores it", async () => {
    await page.click('[data-cue="10"] .t-name');
    await page.keyboard.press("Backspace");
    await page.waitForSelector('[data-cue="10"]', { state: "detached" });
    if (hasCue(10)) throw new Error("cue 10 still on the server");
    await page.keyboard.press("Meta+KeyZ");
    await page.waitForSelector('[data-cue="10"]');
  });

  await step("Backspace while typing in a field edits text, it does not delete the cue", async () => {
    await page.click('[data-cue="5"] .t-name');
    await page.click('[data-testid="insp-name"]');
    await page.keyboard.press("End");
    await page.keyboard.press("Backspace");
    await page.waitForTimeout(100);
    if (!hasCue(5)) throw new Error("cue 5 was deleted while typing");
    await page.keyboard.type("e");
    await page.keyboard.press("Tab");
  });

  await step("right-click opens the row context menu (Delete / Duplicate / Move); Escape closes it without panic", async () => {
    await page.click('[data-cue="6"] .t-name', { button: "right" });
    await page.waitForSelector('[data-testid="row-menu"] [data-testid="ctx-delete"]');
    await page.waitForSelector('[data-testid="row-menu"] [data-testid="ctx-duplicate"]');
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="row-menu"]', { state: "detached" });
    if (mock.getState().mode !== "edit") throw new Error("mode changed");
    await page.click('[data-cue="5"] .t-name');
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

  // ---- desktop web view on touch devices (phone / tablet), still in Edit mode ----
  const DEVICE_SIZES = [[360, 640], [390, 844], [844, 390], [768, 1024], [1024, 768], [1180, 820]];
  const touchDesk = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 1, hasTouch: true, isMobile: true });
  const tp = await touchDesk.newPage();
  watch(tp);
  await tp.goto(ADMIN + "/");
  await tp.waitForSelector('[data-testid="cuetable"] [data-cue]');
  for (const [w, h] of DEVICE_SIZES) {
    await tp.setViewportSize({ width: w, height: h });
    await tp.waitForTimeout(150);
    await step(`edit view ${w}×${h} (touch): no horizontal scroll; GO, PANIC, Add, Delete reachable; targets ≥ 44 px`, async () => {
      if (!(await noHScroll(tp))) throw new Error("page scrolls horizontally");
      const menu = w < 600 || h <= 500 ? "tb-more" : "tb-file-menu";
      const bad = await outside(tp, ["go", "panic", "tb-add-menu", "tb-delete", "tb-duplicate", "tb-undo", "mode-live", menu]);
      if (bad.length) throw new Error(`outside the viewport: ${bad.join(", ")}`);
      const tiny = await tinyTargets(tp, ".toolbar button, .insp-tabs button, .trow .t-gutter", 44);
      if (tiny.length) throw new Error(`too small: ${tiny.join(", ")}`);
    });
  }
  await tp.setViewportSize({ width: 390, height: 844 });
  await tp.waitForTimeout(150);
  await shot(tp, "desktop-phone-edit.png");
  await step("phone: long-press on a cue opens the context menu; Delete there deletes; toast Undo restores", async () => {
    await longPress(touchDesk, tp, '[data-cue="9"] .t-name');
    await tp.waitForSelector('[data-testid="row-menu"] [data-testid="ctx-delete"]');
    await shot(tp, "desktop-phone-context-menu.png");
    await tp.tap('[data-testid="ctx-delete"]');
    await tp.waitForSelector('[data-cue="9"]', { state: "detached" });
    if (hasCue(9)) throw new Error("cue 9 still on the server");
    await tp.tap('[data-testid="toast-action"]');
    await tp.waitForSelector('[data-cue="9"]');
  });
  await step("phone: tap selects; inspector Delete is visible; Move down via the context menu reorders", async () => {
    await tp.tap('[data-cue="2"] .t-name');
    await tp.waitForSelector('[data-testid="insp-delete"]');
    if ((await outside(tp, ["insp-delete", "insp-duplicate", "tb-delete"])).length) throw new Error("delete buttons outside the viewport");
    await shot(tp, "desktop-phone-inspector.png");
    await longPress(touchDesk, tp, '[data-cue="2"] .t-name');
    await tp.tap('[data-testid="ctx-down"]');
    await tp.waitForFunction(() => document.querySelectorAll("[data-cue]")[2]?.getAttribute("data-cue") === "2");
    await longPress(touchDesk, tp, '[data-cue="2"] .t-name');
    await tp.tap('[data-testid="ctx-up"]');
    await tp.waitForFunction(() => document.querySelectorAll("[data-cue]")[1]?.getAttribute("data-cue") === "2");
  });
  await step("phone: the ⋯ menu opens as a sheet and reaches Setup (full screen)", async () => {
    await tp.tap('[data-testid="tb-more"]');
    await tp.waitForSelector('[data-testid="menu-setup"]');
    await shot(tp, "desktop-phone-menu.png");
    await tp.tap('[data-testid="menu-setup"]');
    await tp.waitForSelector('[data-testid="setup"]');
    const box = await tp.locator('[data-testid="setup"]').boundingBox();
    if (box.width < 389 || box.height < 843) throw new Error(`setup is ${Math.round(box.width)}×${Math.round(box.height)}`);
    if (!(await noHScroll(tp))) throw new Error("setup scrolls horizontally");
    await shot(tp, "desktop-phone-setup.png");
    await tp.tap('[data-testid="setup"] [aria-label="Close"]');
    await tp.waitForSelector('[data-testid="setup"]', { state: "detached" });
  });
  await tp.setViewportSize({ width: 768, height: 1024 });
  await tp.tap('[data-cue="5"] .t-name');
  await tp.waitForTimeout(200);
  await shot(tp, "desktop-tablet-edit.png");
  await step("tablet: Add cue menu adds a memo cue; Duplicate button copies it", async () => {
    const count = () => tp.locator('[data-testid="cuetable"] [data-cue]').count();
    const before = await count();
    await tp.tap('[data-testid="tb-add-menu"]');
    await tp.tap('[data-testid="menu-add-memo"]');
    await tp.waitForFunction((n) => document.querySelectorAll('[data-testid="cuetable"] [data-cue]').length === n + 1, before);
    await tp.tap('[data-testid="tb-duplicate"]');
    await tp.waitForFunction((n) => document.querySelectorAll('[data-testid="cuetable"] [data-cue]').length === n + 2, before);
    await tp.tap('[data-testid="tb-undo"]');
    await tp.waitForFunction((n) => document.querySelectorAll('[data-testid="cuetable"] [data-cue]').length === n + 1, before);
    await tp.tap('[data-testid="tb-undo"]');
    await tp.waitForFunction((n) => document.querySelectorAll('[data-testid="cuetable"] [data-cue]').length === n, before);
  });

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

  for (const [w, h] of [[360, 640], [390, 844], [844, 390], [768, 1024]]) {
    await tp.setViewportSize({ width: w, height: h });
    await tp.waitForTimeout(200);
    await step(`live dashboard ${w}×${h} (touch): no horizontal scroll; GO, PANIC, NEXT, list inside the viewport`, async () => {
      await tp.waitForSelector('[data-testid="live-dashboard"]');
      if (!(await noHScroll(tp))) throw new Error("page scrolls horizontally");
      const bad = await outside(tp, ["go", "panic", "pause", "stop-all", "next-panel", "mode-edit"]);
      if (bad.length) throw new Error(`outside the viewport: ${bad.join(", ")}`);
      if ((await tp.locator('.live-cues [data-cue]').count()) === 0) throw new Error("no cue rows");
      const tiny = await tinyTargets(tp, '[data-testid="transport"] button, .live-tabs button', 44);
      if (tiny.length) throw new Error(`too small: ${tiny.join(", ")}`);
    });
    await shot(tp, `desktop-live-${w}x${h}.png`);
  }
  await step("live dashboard on a phone: PANIC needs press-and-hold on touch", async () => {
    await tp.setViewportSize({ width: 390, height: 844 });
    if (!/hold/.test(await tp.locator('[data-testid="panic"]').innerText())) throw new Error("panic is not hold-to-fire");
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
  await step("remote 360 px wide: no horizontal scroll; transport ≥ 56 px, every other target ≥ 44 px; GO/PANIC visible", async () => {
    if (!(await noHScroll(rp))) throw new Error("page scrolls horizontally");
    const small = [...(await tinyTargets(rp, '[data-testid="transport"] button', 56)), ...(await tinyTargets(rp, "button", 44))];
    if (small.length) throw new Error(`too small: ${small.join(", ")}`);
    const bad = await outside(rp, ["go", "panic", "next-panel", "cuelist"]);
    if (bad.length) throw new Error(`outside the viewport: ${bad.join(", ")}`);
  });
  await shot(rp, "remote-360x640.png");
  await step("remote phone: selecting a cue shows the selection bar with Set NEXT", async () => {
    await rp.tap('[data-cue="3"]');
    await rp.waitForSelector('[data-testid="set-next"]');
    if ((await outside(rp, ["set-next", "fire-selected", "go", "panic"])).length) throw new Error("selection bar pushed controls out");
  });
  await rp.setViewportSize({ width: 844, height: 390 });
  await rp.waitForTimeout(300);
  await step("remote landscape phone 844×390: no horizontal scroll; GO, PANIC, NEXT, list visible", async () => {
    if (!(await noHScroll(rp))) throw new Error("page scrolls horizontally");
    const bad = await outside(rp, ["go", "panic", "pause", "stop-all", "next", "prev", "next-panel", "tab-cues", "set-next"]);
    if (bad.length) throw new Error(`outside the viewport: ${bad.join(", ")}`);
    if ((await rp.locator('[data-testid="cuelist"] [data-cue]').count()) === 0) throw new Error("no cue rows");
    const tiny = await tinyTargets(rp, "button", 44);
    if (tiny.length) throw new Error(`too small: ${tiny.join(", ")}`);
  });
  await shot(rp, "remote-844x390.png");

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
  for (const [w, h] of [[390, 844], [360, 640], [844, 390], [768, 1024], [1920, 1080]]) {
    await vp.setViewportSize({ width: w, height: h });
    await vp.waitForTimeout(200);
    await step(`/view ${w}×${h}: no horizontal scroll; NEXT, clocks and ≥ 3 list rows visible`, async () => {
      if (!(await noHScroll(vp))) throw new Error("page scrolls horizontally");
      const bad = await outside(vp, ["next-panel", "clocks", "cuelist"]);
      if (bad.length) throw new Error(`outside the viewport: ${bad.join(", ")}`);
      const rows = await vp.evaluate(() => {
        const box = document.querySelector('[data-testid="cuelist"]').getBoundingClientRect();
        return [...document.querySelectorAll('[data-testid="cuelist"] [data-cue]')].filter((r) => {
          const b = r.getBoundingClientRect();
          return b.top >= box.top - 1 && b.bottom <= box.bottom + 1;
        }).length;
      });
      if (rows < 3) throw new Error(`only ${rows} rows visible`);
    });
    await shot(vp, `view-${w}x${h}.png`);
  }

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
