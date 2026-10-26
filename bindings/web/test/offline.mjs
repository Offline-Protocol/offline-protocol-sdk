// The lobby, opened once online, loads and starts its node with no network
// (W15): the same address after an offline reload, and a second profile's
// node from the cached page under another query (the cache holds none).
// Then, online, a new build with two tabs open: pressing Update in one must
// bring both onto the new build, each as the same address, because the new
// build takes over every page of the scope at once. A third tab whose node
// is refused (its account is open in the second, W8) must still be offered
// the update, and must not reload on it: reloading, it would take the
// account from the second tab while that tab stops its node. Nor must a
// fourth tab on that account still waiting for its lock when the new build
// takes over, which would take the lock as the second tab released it.
//
// Removing the page's workerUrl makes this fail: the engine's worker is then
// outside the service worker's scope and its script comes from the network.
// So does listening for controllerchange only in the tab that pressed
// Update: the other tab keeps running the old build.
//
// Usage: node test/offline.mjs <base-url> <timeout-ms>, where the base URL
// serves the repository root. BROWSERS narrows the engines, as in run.mjs.

import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
// The update phase writes a new build id and puts the real one back; a run
// killed in between leaves it changed until build.sh writes it again.
const buildId = new URL("../pkg/build-id.js", import.meta.url);
const realBuildId = readFileSync(buildId);

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

    if (verdict === "PASS") {
      await context.setOffline(false);
      const other = await context.newPage();
      other.on("pageerror", (error) => console.log("[page exception]", error.message));
      await other.goto(`${lobby}?profile=a-${run}`);
      const tabs = [page, other];
      const before = await Promise.all(tabs.map(address));
      await Promise.all(tabs.map((tab) => tab.evaluate(() => (window.oldBuild = true))));
      const refused = await context.newPage();
      const refusal = new Promise((resolve) => refused.once("pageerror", (error) => resolve(error.message)));
      await refused.goto(`${lobby}?profile=a-${run}`);
      const refusedWith = await Promise.race([refusal, new Promise((resolve) => setTimeout(resolve, timeout, null))]);
      console.log(`a third tab on the second's account: ${refusedWith}`);
      if (!refusedWith?.includes("another node already runs this account")) {
        throw new Error(`the third tab was not refused: ${refusedWith ?? "no error before the deadline"}`);
      }
      await refused.evaluate(() => (window.oldBuild = true));
      const id = `update${name}${run}`;
      writeFileSync(buildId, `self.BUILD_ID = "${id}";\n`);
      try {
        await page.evaluate(async () => (await navigator.serviceWorker.getRegistration()).update());
        await page.locator("#update").waitFor({ state: "visible", timeout });
        await refused.locator("#update").waitFor({ state: "visible", timeout });
        // In its lock wait (two seconds, W8) when the takeover comes.
        const waiting = await context.newPage();
        const waitingErrors = [];
        waiting.on("pageerror", (error) => waitingErrors.push(error.message));
        await waiting.goto(`${lobby}?profile=a-${run}`, { waitUntil: "domcontentloaded" });
        await waiting.evaluate(() => (window.oldBuild = true));
        // Queued for the account's lock behind the second tab, as the
        // origin's lock manager shows, before the takeover: an abort that
        // landed earlier would never test what letting go of the queue does.
        // A poll of evaluate, which awaits the query: waitForFunction takes a
        // returned promise as a truthy value and would not wait at all.
        const queuedBy = performance.now() + 1500;
        while (!(await waiting.evaluate(async () =>
          (await navigator.locks.query()).pending.some((l) => l.name.startsWith("offline-protocol/"))))) {
          if (performance.now() > queuedBy) throw new Error("the fourth tab never queued for the account's lock");
          await new Promise((resolve) => setTimeout(resolve, 10));
        }
        await page.click("#reload");
        for (const tab of tabs) {
          await tab.waitForFunction(
            () => !window.oldBuild && document.getElementById("me").textContent.startsWith("off1"),
            null,
            { timeout, polling: 250 },
          );
        }
        // Read before the real id goes back: each reload is a navigation,
        // whose update check, run after the restore, would install the real
        // build as a third cache.
        await refused.locator("#status", { hasText: "A new version of this page is running" }).waitFor({ timeout });
        await waiting.locator("#status", { hasText: "A new version of this page is running" }).waitFor({ timeout });
        const refusedReloaded = await refused.evaluate(() => !window.oldBuild);
        const waitingReloaded = await waiting.evaluate(() => !window.oldBuild);
        // Its start was aborted, not refused: a refusal would mean its lock
        // wait ran out before the takeover and the abort was never exercised.
        const waitingAborted = waitingErrors.some((message) => message.includes("start was abandoned for a new version"));
        const after = await Promise.all(tabs.map((tab) => tab.locator("#me").textContent()));
        const caches = await page.evaluate(() => caches.keys());
        console.log(`updated both tabs as ${after.join(", ")}, caches ${caches.join(", ")}`);
        if (refusedReloaded) verdict = "FAIL: the refused tab reloaded on the update";
        else if (waitingReloaded) verdict = "FAIL: the tab waiting for its lock reloaded on the update";
        else if (!waitingAborted) verdict = `FAIL: the waiting tab's start was not aborted (${waitingErrors.join("; ") || "no error"})`;
        else if (after.some((value, i) => value !== before[i])) verdict = "FAIL: a tab changed address across the update";
        else if (caches.length !== 1 || caches[0] !== `web-lobby-${id}`) verdict = `FAIL: caches after the update ${caches}`;
      } finally {
        writeFileSync(buildId, realBuildId);
      }
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
