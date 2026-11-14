import { OfflineNode } from "@offline-protocol/web";

const $ = (id) => document.getElementById(id);
const short = (address) => `${address.slice(0, 10)}…${address.slice(-6)}`;
const neighbours = new Map();

function render() {
  const list = $("neighbours");
  list.replaceChildren();
  if (neighbours.size === 0) {
    list.innerHTML = '<li class="muted">none yet</li>';
  }
  const select = $("to");
  const chosen = select.value;
  select.replaceChildren();
  for (const [address, { secure }] of neighbours) {
    const item = document.createElement("li");
    item.textContent = `${short(address)} ${secure ? "· encrypted session" : "· establishing…"}`;
    list.append(item);
    select.append(new Option(short(address), address, false, address === chosen));
  }
}

function note(text) {
  const item = document.createElement("li");
  item.textContent = text;
  $("messages").prepend(item);
}

async function main() {
  const workerUrl = new URL("../engine-worker.js", import.meta.url);

  const node = await OfflineNode.start({
    appId: "web-vite-starter",
    storage: "memory",
    workerUrl,
  });

  $("me").textContent = node.address;

  node.addEventListener("neighbor_discovered", ({ detail }) => {
    if (!neighbours.has(detail.peer_id)) neighbours.set(detail.peer_id, { secure: false });
    render();
  });
  node.addEventListener("neighbor_lost", ({ detail }) => {
    neighbours.delete(detail.peer_id);
    render();
  });
  node.addEventListener("secure_session_established", ({ detail }) => {
    neighbours.set(detail.peer_id, { secure: true });
    render();
  });
  node.addEventListener("message_received", ({ detail }) => {
    note(`${short(detail.sender)}: ${detail.content}${detail.encrypted ? "  🔒" : ""}`);
  });
  node.addEventListener("fatal", ({ detail }) => {
    note(`fatal: ${detail?.message ?? "worker ended"}`);
  });

  $("send").addEventListener("submit", async (event) => {
    event.preventDefault();
    const to = $("to").value;
    const text = $("text").value;
    if (!to) return;
    await node.sendMessage(to, text);
    note(`you → ${short(to)}: ${text}`);
    $("text").value = "";
  });

  addEventListener("pagehide", () => node.stop());
}

main().catch((error) => {
  console.error(error);
  $("me").textContent =
    "failed to start — run bash bindings/web/scripts/build.sh from the repo root";
});
