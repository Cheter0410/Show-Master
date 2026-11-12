// Failover end to end: two real Show Master processes (main + backup), a
// tablet logged in on the main; the main is killed hard. The backup must take
// over by itself and the tablet must move there and keep working - no PIN.
//
//   cargo build -p showmaster && node ui/e2e/failover.mjs
import { createRequire } from "node:module";
import { execSync, spawn } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const BIN = process.env.SM_BIN ?? join(ROOT, "target", "debug", process.platform === "win32" ? "showmaster.exe" : "showmaster");
const require = createRequire(import.meta.url);
let pw;
try {
  pw = require("playwright");
} catch {
  pw = require(join(execSync("npm root -g", { encoding: "utf8" }).trim(), "playwright"));
}

const work = mkdtempSync(join(tmpdir(), "sm-failover-"));
const start = (name, port) => {
  const home = join(work, name);
  return spawn(BIN, ["--headless", "--no-update-check", "--make-demo", join(home, "show"), "--port", String(port), "--log-dir", join(home, "logs")], {
    stdio: "ignore",
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, "cfg"), APPDATA: join(home, "cfg") },
  });
};
const MAIN = 7471, BACKUP = 7472;
const main = start("main", MAIN);
const backup = start("backup", BACKUP);
const api = async (port, path, body) =>
  (await fetch(`http://127.0.0.1:${port}${path}`, body ? { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) } : undefined)).json();
const state = (port) => api(port, "/api/v1/state");
const until = async (what, f, ms = 15000) => {
  const end = Date.now() + ms;
  for (;;) {
    try {
      if (await f()) return;
    } catch {
      /* not up yet */
    }
    if (Date.now() > end) throw new Error(`timed out: ${what}`);
    await new Promise((r) => setTimeout(r, 100));
  }
};
let failures = 0;
const step = async (name, fn) => {
  try {
    await fn();
    console.log(`  ok   ${name}`);
  } catch (e) {
    failures++;
    console.log(` FAIL  ${name}  (${String(e?.message ?? e).split("\n")[0]})`);
  }
};

let browser;
try {
  await until("both up", async () => (await state(MAIN)).show && (await state(BACKUP)).show);
  const lan = (await state(MAIN)).remote.urls[0]?.replace(/:\d+$/, "").replace(/^http:\/\//, "");
  if (!lan) throw new Error("no LAN address");
  await api(BACKUP, "/api/v1/command", { cmd: "set_link", role: "backup", key: "e2e-key" });
  await api(MAIN, "/api/v1/command", { cmd: "set_link", role: "main", peer: `${lan}:${BACKUP}`, key: "e2e-key" });

  await step("main and backup linked, backup follows in standby", async () => {
    await until("linked", async () => (await state(BACKUP)).remote.link.connected === true);
    const l = (await state(BACKUP)).remote.link;
    if (!l.standby) throw new Error(JSON.stringify(l));
  });

  browser = await pw.chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1024, height: 768 }, hasTouch: true });
  const page = await ctx.newPage();
  await step("tablet logs in on the main with the PIN", async () => {
    await page.goto(`http://${lan}:${MAIN}/remote`);
    await page.waitForSelector('[data-testid="pinpad"]');
    for (const d of "1234") await page.tap(`[data-testid="pin-${d}"]`);
    await page.tap('[data-testid="pin-ok"]');
    await page.waitForSelector('[data-testid="remote-app"]');
    await page.tap('[data-testid="go"]');
    await until("backup followed the GO", async () => JSON.stringify((await state(BACKUP)).playheads) === JSON.stringify((await state(MAIN)).playheads));
  });

  await step("main dies: backup takes over, tablet moves there without a PIN", async () => {
    // Give the session a moment to be mirrored, then kill the main hard.
    await new Promise((r) => setTimeout(r, 1200));
    main.kill("SIGKILL");
    await until("backup took over", async () => (await state(BACKUP)).remote.link.taken_over === true, 8000);
    await page.waitForURL(new RegExp(`:${BACKUP}/remote`), { timeout: 15000 });
    await page.waitForSelector('[data-testid="remote-app"]', { timeout: 8000 });
    if (await page.locator('[data-testid="pinpad"]').count()) throw new Error("asked for the PIN again");
    const before = JSON.stringify((await state(BACKUP)).playheads);
    await page.tap('[data-testid="go"]');
    await until("GO on the backup", async () => JSON.stringify((await state(BACKUP)).playheads) !== before, 5000);
  });
  await step("the backup answers to the plain name showmaster.local again", async () => {
    const m = (await state(BACKUP)).remote.mdns;
    console.log(`       (backup mDNS: active=${m?.active} host=${m?.host})`);
    if (!m || !m.active) return; // no multicast here: nothing to check
    await until("name reclaimed", async () => (await state(BACKUP)).remote.mdns.host === "showmaster.local", 15000);
  });
} finally {
  await browser?.close();
  main.kill("SIGKILL");
  backup.kill("SIGINT");
  await new Promise((r) => setTimeout(r, 300));
  rmSync(work, { recursive: true, force: true });
}
console.log(failures ? `\n${failures} step(s) FAILED` : "\nall steps passed");
process.exit(failures ? 1 : 0);
