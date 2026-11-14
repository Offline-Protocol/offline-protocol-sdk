# Web Vite quickstart

Opinionated dev template for the **experimental** browser binding (friction
#25). Cloneable tab-to-tab mesh with Vite — not a published npm package.

## Prerequisites

1. **Rust toolchain** and this repository checkout.
2. **wasm-bindgen CLI** pinned to `Cargo.lock` (currently `0.2.105`):

   ```bash
   cargo install wasm-bindgen-cli --version 0.2.105 --locked
   ```

3. **Build the WASM module** (writes `bindings/web/pkg/`):

   ```bash
   bash bindings/web/scripts/build.sh
   ```

## Run

From this directory:

```bash
npm ci
npm run dev
```

Open the printed local URL (default `http://127.0.0.1:5173/`) in **two tabs**.
Each tab shows its `off1…` address, discovers the other, establishes an MLS
session, and can send encrypted messages. Storage is **memory**: every tab is a
separate device (same model as [web-two-tabs](../../../examples/web-two-tabs/README.md)).

## What this template wires

| Piece | Where |
|-------|--------|
| Page API | `@offline-protocol/web` → `bindings/web/src/index.js` (Vite alias) |
| Worker | `engine-worker.js` imports `bindings/web/src/worker.js` |
| WASM glue | `bindings/web/pkg/` (from `build.sh`) |

Read [docs/bridges/web.md](../../../docs/bridges/web.md) before adding WebRTC,
OPFS durable storage, Nostr, or Bluetooth.

## Go / no-go (pick another platform when…)

| Use the browser binding when… | Choose React Native / native instead when… |
|-------------------------------|--------------------------------------------|
| You need a **full mesh node in the browser** (tabs, LAN WebRTC, optional Nostr) | You want **`npm install` + one native module** with minimal WASM/worker setup |
| Your team can own **worker lifecycle, carriers, and secure context** (HTTPS) | You need **background mesh on iOS/Android** with system transports |
| You accept **experimental** API and manual WASM builds in CI | You need **telemetry pipe, file-store, and release wheels** out of the box |

This starter intentionally does **not** hide the worker or the WASM build step.
If the table's right column matches your timeline, start from the
[integrator map](../../../docs/integrator-map.md) React Native or Flutter rows.

## Production shape

`npm run build` produces static assets for inspection; shipping a PWA with
durable storage, service-worker caching, and cross-device WebRTC follows
[examples/web-lobby](../../../examples/web-lobby/README.md) (W15), not this
minimal template.
