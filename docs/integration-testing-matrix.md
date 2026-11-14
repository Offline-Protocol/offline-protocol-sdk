# Integration testing matrix

**Purpose:** Standard QA for integrators and release owners (friction **#28**).
Simulators and emulators are fine for **app and SDK wiring**; they do **not**
faithfully reproduce BLE, Wi‑Fi Direct, or browser mesh on real networks. This
page says what to run where, and which repo paths are the **golden**
smoke tests.

**Start elsewhere for day-one setup:** [integrator map](integrator-map.md).

---

## Golden paths (use these first)

Run these before you treat mesh as “done” on a platform.

| Goal | Golden path | Proves |
|------|-------------|--------|
| React Native mesh between two phones | [React Native example app](../examples/react-native-app/README.md) on **two physical devices** (distinct `appId` if both apps on one phone) | Discovery, MLS, send/receive over real radios |
| Host toolchain / linking | `npx --no-install mesh-sdk-doctor` | Native module, pods/Gradle, host compatibility ([react-native-host-doctor.md](react-native-host-doctor.md)) |
| Local API / gateway delivery | [`offline-protocol-verify`](local-api.md#checking-what-the-network-did-offline-protocol-verify) against a running service | Event-backed proof of send, relay, receive, deliver ([examples/offline-first](../examples/offline-first/README.md)) |
| Browser binding (two devices) | [web-devices checklist](bridges/web-devices.md) with [web-lobby](../examples/web-lobby/README.md) | Secure context, OPFS, WebRTC on hardware |
| Browser binding (one machine) | `bash bindings/web/scripts/test-browser.sh` | Regression harness in three engines (not a substitute for two-device QA) |

---

## What belongs on simulators vs hardware

| Surface | Simulator / emulator | Physical device | Notes |
|---------|---------------------|-----------------|-------|
| **React Native** — UI, config, lifecycle, JS harness | Yes | Recommended for first mesh milestone | `mesh-sdk-doctor` + `npm run test:js` in the package; no BLE mesh fidelity on sim |
| **React Native** — BLE discovery & encrypted DM | No | **Required** | See [mobile-permissions § Verify before release](mobile-permissions.md#verify-before-release) |
| **React Native** — Wi‑Fi Direct | No | **Required** | Budget **1–2+ minutes** for group formation; [troubleshooting.md](troubleshooting.md) |
| **Native iOS / Android** — compile, unit tests | Yes (where XCTest/JUnit run) | Required before ship for transports | Native integration guides; [storage adapters](../examples/storage-adapters/README.md) if custom storage |
| **Python service / local API** | N/A (host OS) | **Two hosts or containers** for true carry | `offline-protocol-verify`; Docker path in [python-distribution.md](python-distribution.md) |
| **Web** — engine unit/conformance | CI (headless, loopback) | **Required** before web release | [web-devices.md](bridges/web-devices.md); tab mesh in [web-two-tabs](../examples/web-two-tabs/README.md) is dev-only |
| **Desktop C# / Java** | N/A | Optional second machine | Release natives + binding consumer-check tests; mesh not the primary story |

**Rule of thumb:** green **unit/CI** plus green **simulator** builds mean the binding compiles and basic APIs work. They do **not** replace a **two-node** test on hardware (or two hosts for Python).

---

## Suggested integration milestones

### Milestone A — Wiring (simulator or one device OK)

1. Install SDK for your platform ([integrator map](integrator-map.md)).
2. Follow lifecycle ordering (listeners → `start()` → send).
3. React Native: `mesh-sdk-doctor` passes.
4. Run package or repo tests relevant to your surface (see below).

### Milestone B — Mesh smoke (hardware or two hosts)

1. Two peers discover each other (BLE or configured transports).
2. `secure_session_established` (or equivalent) on both sides.
3. Round-trip message with encryption enabled (default).
4. For local API: `offline-protocol-verify pair` / `send` / `await` as in [local-api.md](local-api.md).

### Milestone C — Release QA (your product)

1. All rows in [mobile-permissions § Verify before release](mobile-permissions.md#verify-before-release) that apply to your transports.
2. Upgrade path: [UPGRADING §13 checklist](UPGRADING.md#13-upgrade-test-checklist) for your version span.
3. Platform-specific checklists: [web-devices.md](bridges/web-devices.md) if you ship browser nodes.

---

## Automated tests in this repository (by layer)

| Layer | Command / location | What it validates |
|-------|-------------------|-------------------|
| Rust protocol | `cargo test --workspace` (CI) | Core behaviour, many bridge guards |
| React Native JS | `cd bindings/react-native && npm run test:js` | TS API, lifecycle hints, config scenarios (no native mesh) |
| Web browser | `bash bindings/web/scripts/test-browser.sh` | Pages in Chromium, Firefox, WebKit on one host |
| Web OPFS conformance | `bash bindings/web/scripts/test-conformance.sh` | Storage suites in real browsers |
| Python verify CLI | Exercised in service image / offline-first examples | Service event proofs |
| Mobile native packages | Swift/Android CI jobs | Compile, bridge tests on sim where applicable |

None of the rows above replaces **Milestone B** on physical radios or two real browsers on a LAN.

---

## Related docs

| Document | Role |
|----------|------|
| [mobile-permissions.md](mobile-permissions.md) | Runtime grants and release verify table |
| [troubleshooting.md](troubleshooting.md) | Wi‑Fi Direct timing, multi-app `appId` |
| [react-native-integration.md](react-native-integration.md) | RN API and transports |
| [local-api.md](local-api.md) | `offline-protocol-verify` reference |
| [bridges/web-devices.md](bridges/web-devices.md) | Two-device browser checklist |
