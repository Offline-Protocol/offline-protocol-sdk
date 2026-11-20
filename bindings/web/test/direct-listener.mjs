// The runner's half of direct.html: starts a native listener on this
// machine's private address, and lets the page drive it. By default that is
// the crate's example listener (a full native engine behind
// offline-protocol-webrtc-direct); with DIRECT_LISTENER=python it is the
// Python host (test/direct_python_host.py: the Python package's
// PeerStreamManager and its WebRTC Direct bridge). Both print the same lines
// and take the same commands, so the page does not know which it dialled.
//
// The listener binds a private IPv4 address of this machine, never loopback:
// a dial code refuses loopback, and Firefox does not dial it. DIRECT_HOST
// names the address; otherwise it is en0's on macOS, else the first private
// IPv4 address of an interface that is up. LISTENER names the example's
// binary (test-browser.sh builds it and sets it); PYTHON names the
// interpreter for the Python host (default: the Python package's .venv,
// which needs bindings/python/scripts/build-desktop.sh run first).
//
// The page sees four functions:
// - directListen(appId): starts a listener (ending any earlier one) and
//   resolves to its `listening` line: { code, address, host, port }.
// - directLines(): every line the current listener has printed, parsed.
// - directCommand(line): one command on its standard input.
// - directStop(): ends it.

import { spawn, execFileSync } from "node:child_process";
import { networkInterfaces, platform } from "node:os";
import { fileURLToPath } from "node:url";

const PRIVATE_V4 = /^(10\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.)/;

/// This machine's private IPv4 address, or an Error saying why there is none.
export function privateAddress() {
  if (process.env.DIRECT_HOST) return process.env.DIRECT_HOST;
  if (platform() === "darwin") {
    try {
      const en0 = execFileSync("ipconfig", ["getifaddr", "en0"], { encoding: "utf8" }).trim();
      if (PRIVATE_V4.test(en0)) return en0;
    } catch {
      // No address on en0: fall through to the interface list.
    }
  }
  for (const addresses of Object.values(networkInterfaces())) {
    for (const { family, internal, address } of addresses ?? []) {
      if (!internal && (family === "IPv4" || family === 4) && PRIVATE_V4.test(address)) return address;
    }
  }
  return new Error(
    "no private IPv4 address on this machine: direct.html dials a listener on one (set DIRECT_HOST to choose it)",
  );
}

/// The command that starts the listener DIRECT_LISTENER names, without the
/// address and app id every listener takes after it.
export function listenerCommand() {
  const kind = process.env.DIRECT_LISTENER ?? "example";
  if (kind === "python") {
    const python =
      process.env.PYTHON ?? fileURLToPath(new URL("../../python/.venv/bin/python", import.meta.url));
    return [python, fileURLToPath(new URL("./direct_python_host.py", import.meta.url))];
  }
  if (kind === "example") {
    return [process.env.LISTENER ?? fileURLToPath(new URL("../../../target/debug/examples/listener", import.meta.url))];
  }
  throw new Error(`DIRECT_LISTENER is "example" or "python", not ${JSON.stringify(kind)}`);
}

/// Exposes the listener functions on `page`. `command` starts the listener
/// (see listenerCommand). Resolves to a cleanup that ends the listener.
export async function attachListener(page, { command, host, verbose = false }) {
  let child = null;
  let lines = [];

  function stop() {
    if (child === null) return;
    child.stdin.end();
    child.kill();
    child = null;
  }

  await page.exposeFunction("directListen", async (appId) => {
    if (host instanceof Error) throw host;
    stop();
    lines = [];
    const [program, ...prefix] = command;
    const proc = spawn(program, [...prefix, host, "--app-id", appId], { stdio: ["pipe", "pipe", "pipe"] });
    child = proc;
    const own = lines;
    proc.stderr.on("data", (d) => verbose && process.stderr.write(`[listener] ${d}`));
    let buffered = "";
    let listening;
    const first = new Promise((resolve, reject) => {
      listening = { resolve, reject };
      proc.on("error", reject);
      proc.on("exit", (code) => reject(new Error(`the listener exited (${code}) before listening`)));
    });
    proc.stdout.on("data", (chunk) => {
      buffered += chunk;
      let newline;
      while ((newline = buffered.indexOf("\n")) !== -1) {
        const text = buffered.slice(0, newline);
        buffered = buffered.slice(newline + 1);
        if (!text.trim()) continue;
        let line;
        try {
          line = JSON.parse(text);
        } catch {
          continue;
        }
        if (verbose) console.log(`[listener] ${text}`);
        own.push(line);
        if (line.type === "listening") listening.resolve(line);
      }
    });
    return first;
  });
  await page.exposeFunction("directLines", () => lines);
  await page.exposeFunction("directCommand", (line) => {
    if (child === null) throw new Error("no listener is running");
    child.stdin.write(`${line}\n`);
  });
  await page.exposeFunction("directStop", () => stop());
  return stop;
}
