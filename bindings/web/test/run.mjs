// Opens every test page in each browser engine and waits for the page's title
// to say PASS or FAIL. Playwright drives Chromium, Firefox and WebKit through
// one API, so a page runs unchanged in all three.
//
// Usage: node test/run.mjs <base-url> <relay-url> <timeout-ms> <page>...
// BROWSERS narrows the engines (default: chromium firefox webkit).

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, firefox, webkit } from "playwright";

const [base, relay, timeoutArg, ...pages] = process.argv.slice(2);
const timeout = Number(timeoutArg);
const engines = { chromium, firefox, webkit };
const names = (process.env.BROWSERS ?? "chromium firefox webkit").split(/\s+/).filter(Boolean);
for (const name of names) {
  if (!engines[name]) {
    console.error(`unknown browser ${name}: one of ${Object.keys(engines).join(", ")}`);
    process.exit(2);
  }
}

// What each engine needs so two pages on one machine can link over WebRTC
// with no mDNS responder: host candidates as addresses rather than .local
// names. A real deployment keeps every engine's default. WebKit has no such
// switch. Linux CI sandboxes need Chromium's --no-sandbox, which Playwright
// passes itself.
const options = {
  chromium: { args: ["--disable-features=WebRtcHideLocalIpsWithMdns"] },
  firefox: {
    firefoxUserPrefs: {
      "media.peerconnection.ice.obfuscate_host_addresses": false,
    },
  },
  webkit: {},
};

const results = [];
for (const name of names) {
  // A persistent profile, as a person's browser is: OPFS on disk, and the
  // restart page's identity surviving its worker within the run.
  const profile = mkdtempSync(join(tmpdir(), `offline-web-${name}-`));
  const context = await engines[name].launchPersistentContext(profile, {
    headless: true,
    ...options[name],
  });
  try {
    for (const path of pages) {
      console.log(`== ${name} ${path}`);
      const page = await context.newPage();
      page.on("console", (message) => console.log("[page]", message.text()));
      page.on("pageerror", (error) => console.log("[page exception]", error.message));
      let title = "TIMEOUT";
      try {
        await page.goto(`${base}/${path}?relay=${encodeURIComponent(relay)}`);
        await page.waitForFunction(
          () => document.title === "PASS" || document.title.startsWith("FAIL"),
          null,
          { timeout, polling: 250 },
        );
        title = await page.title();
      } catch (error) {
        console.log(`[runner] ${error.message.split("\n")[0]}`);
      }
      console.log(await page.locator("#log").textContent().catch(() => ""));
      console.log(title);
      results.push({ name, path, title });
      await page.close();
    }
  } finally {
    await context.close();
    rmSync(profile, { recursive: true, force: true });
  }
}

console.log("\n== results");
for (const { name, path, title } of results) {
  console.log(`${title === "PASS" ? "PASS" : "FAIL"}  ${name.padEnd(8)} ${path}  ${title === "PASS" ? "" : title}`);
}
process.exit(results.every(({ title }) => title === "PASS") ? 0 : 1);
