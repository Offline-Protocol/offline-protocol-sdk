// Opens a page in headless Chrome over the DevTools protocol and waits for its
// title to say PASS or FAIL. Node 22's built-in WebSocket and fetch only: no
// dependencies to install.
//
// Usage: node test/run.mjs <devtools-port> <page-url> [timeout-ms]

const [port, url, timeoutArg] = process.argv.slice(2);
const timeout = Number(timeoutArg ?? 60000);

const target = await (await fetch(`http://127.0.0.1:${port}/json/new?${encodeURI(url)}`, { method: "PUT" })).json();
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.onopen = resolve;
  socket.onerror = reject;
});

let id = 0;
const waiting = new Map();
socket.onmessage = ({ data }) => {
  const message = JSON.parse(data);
  if (message.id && waiting.has(message.id)) {
    waiting.get(message.id)(message.result);
    waiting.delete(message.id);
  }
  if (message.method === "Runtime.consoleAPICalled") {
    console.log("[page]", message.params.args.map((a) => a.value ?? a.description).join(" "));
  }
  if (message.method === "Runtime.exceptionThrown") {
    console.log("[page exception]", message.params.exceptionDetails.exception?.description);
  }
};
const send = (method, params = {}) =>
  new Promise((resolve) => {
    waiting.set(++id, resolve);
    socket.send(JSON.stringify({ id, method, params }));
  });

await send("Runtime.enable");
const deadline = Date.now() + timeout;
let title = "";
while (Date.now() < deadline) {
  const result = await send("Runtime.evaluate", { expression: "document.title" });
  title = result?.result?.value ?? "";
  if (title === "PASS" || title.startsWith("FAIL")) break;
  await new Promise((r) => setTimeout(r, 250));
}
const log = await send("Runtime.evaluate", {
  expression: "document.getElementById('log')?.textContent ?? ''",
});
console.log(log?.result?.value ?? "");
console.log(title || "TIMEOUT");
socket.close();
process.exit(title === "PASS" ? 0 : 1);
