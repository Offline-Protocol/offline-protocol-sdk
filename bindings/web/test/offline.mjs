// The lobby, opened once online, loads and starts its node with no network
// (W15): the same address after an offline reload, and a second profile's
// node from the cached page under another query (the cache holds none).
//
// Removing the page's workerUrl makes this fail: the engine's worker is then
// outside the service worker's scope and its script comes from the network.
//
// Usage: node test/offline.mjs <base-url> <timeout-ms>, where the base URL
// serves the repository root. BROWSERS narrows the engines, as in run.mjs.

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, firefox, webkit } from "playwright";

const [base, timeoutArg] = process.argv.slice(2);
const timeout = Number(timeoutArg);
const engines = { chromium, firefox, webkit };
const names = (process.env.BROWSERS ?? "chromium firefox webkit").split(/\s+/).filter(Boolean);
const lobby = `${base}/examples/web-lobby/`;
// Accounts of their own each run: Playwright's WebKit keeps OPFS outside
// the profile directory, so a fixed account finds the last run's log with no
// wrapping key beside it, and its node rightly refuses to start (W8).
const run = crypto.randomUUID().slice(0, 8);

const address = (page) =>
  page.waitForFunction(() => document.getElementById("me").textContent.startsWith("off1"), null, {
    timeout,
    polling: 250,
  }).then(() => page.locator("#me").textContent());

const results = [];
for (const name of names) {
  console.log(`== ${name} offline lobby`);
  const profile = mkdtempSync(join(tmpdir(), `offline-web-sw-${name}-`));
  const context = await engines[name].launchPersistentContext(profile, { headless: true });
  let verdict = "PASS";
  try {
    const page = await context.newPage();
    page.on("pageerror", (error) => console.log("[page exception]", error.message));
    await page.goto(`${lobby}?profile=a-${run}`);
    const online = await address(page);
    const state = await page.evaluate(async () => (await navigator.serviceWorker.ready).active.state);
    console.log(`online as ${online}, service worker ${state}`);

    await context.setOffline(true);
    await page.reload();
    const offline = await address(page);
    const controlled = await page.evaluate(() => navigator.serviceWorker.controller !== null);
    console.log(`offline as ${offline}, controlled ${controlled}`);
    if (offline !== online) verdict = `FAIL: came back as ${offline}, not ${online}`;
    else if (!controlled) verdict = "FAIL: the reloaded page is not controlled";

    if (verdict === "PASS") {
      await page.goto(`${lobby}?profile=b-${run}`);
      const second = await address(page);
      console.log(`offline second profile as ${second}`);
      if (second === online) verdict = "FAIL: a second profile came back as the first";
    }
  } catch (error) {
    verdict = `FAIL: ${error.message.split("\n")[0]}`;
  } finally {
    await context.close();
    rmSync(profile, { recursive: true, force: true });
  }
  console.log(verdict);
  results.push({ name, verdict });
}

console.log("\n== offline results");
for (const { name, verdict } of results) console.log(`${verdict === "PASS" ? "PASS" : "FAIL"}  ${name.padEnd(8)} ${verdict === "PASS" ? "" : verdict}`);
process.exit(results.every(({ verdict }) => verdict === "PASS") ? 0 : 1);
