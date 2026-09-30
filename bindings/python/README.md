# Offline Protocol SDK — Python

Python bindings for the Offline Protocol SDK: offline-first mesh networking with MLS end-to-end encryption. Supports macOS, Linux, and Windows.

**Dual-licensed:** use it under [AGPL-3.0-only](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE), or buy a [commercial license](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md), your call. The package metadata can only name `AGPL-3.0-only` because SPDX has no identifier for the commercial offer — see [License](#license) for the full breakdown.

> **Upgrading an existing install?** Read [docs/UPGRADING.md](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/UPGRADING.md)
> first — `ProtocolManager` now requires an explicit `state_root`, and this
> release is not safely downgradable.

## Quick Start

### 1. Build the native library

```bash
cd bindings/python
bash scripts/build-desktop.sh
```

This compiles the Rust core for your platform and generates the FFI bindings. It
regenerates Swift and Kotlin alongside Python by delegating to the repo-root
`scripts/generate-bindings.sh` — the three are one artifact set off one UDL, so
they are never refreshed apart.

**Prerequisites:** Rust toolchain, `uniffi-bindgen` (`cargo install uniffi --version 0.30.0 --features cli --locked`).

### 2. Install the package

```bash
pip install -e .
```

### 3. Use it

```python
import asyncio
from pathlib import Path

from offline_protocol_sdk import ProtocolManager
from offline_protocol_sdk.offline_protocol import ProtocolConfig, OverflowPolicy

config = ProtocolConfig(
    app_id="my-app",
    user_id="alice",
    ble_enabled=False,
    wifi_direct_enabled=False,
    internet_enabled=True,
    reticulum_enabled=False,
    nostr_enabled=False,
    prefer_online=True,
    initial_ttl=3,
    encryption_enabled=True,
    auto_key_exchange=True,
    store_pending=True,
    require_encryption=False,
    max_pending_per_peer=100,
    max_pending_global=1000,
    pending_ttl_ms=86_400_000,  # 24 h (the SDK default)
    overflow_policy=OverflowPolicy.DROP_OLDEST,
)

async def main():
    # The installer must remove this application-owned directory on uninstall.
    state_root = Path("/app/install-owned-data/offline-protocol")
    async with ProtocolManager(
        config,
        event_handler=print,
        state_root=state_root,
    ) as pm:
        pm.internet.configure(server_url="ws://relay.example.com")
        await pm.internet.start()

        msg_id = pm.send_message("bob", "Hello from Python!")
        print(f"Sent: {msg_id}")

        # Keep running to receive messages
        await asyncio.sleep(30)

asyncio.run(main())
```

### Two hosts without a relay

Set `wifi_direct_enabled=True` and `ProtocolManager` builds a
`PeerStreamManager` as `pm.peer_stream`. Configure and start it after
`pm.start()`, once this device has an address to prove:

```python
pm.peer_stream.configure(
    listen_host="0.0.0.0",            # every interface; the default
    listen_port=7878,                 # 0 = ephemeral, None = connect out only
    peers=["off1...@10.0.0.7:7878"],  # or plain "host:port"; the address is then learned
    advertise=True, discover=True,    # DNS-SD, needs: pip install 'offline-protocol-sdk[lan]'
)
await pm.peer_stream.start()
```

A peer is announced only under the address its preamble proves, and
`pm.stop()` stops the stream layer with everything else.

### Services, and services on the LAN

`Services` wraps the generated `MeshServices` and keeps the one thing the
engine cannot give back: the list of what this node registered. `respond`
refuses a status the engine would refuse (`ok`, `not_found` and `error` are
the whole set) with the reason instead of an opaque core error.

```python
from offline_protocol_sdk.services import Services
from offline_protocol_sdk.dnssd_bridge import DnsSdBridge

services = Services(pm.protocol)
services.register("weather.v1", "2.0", {"format": "json"})   # before or after pm.start()

# Publish this node's registrations on the LAN and import the neighbours'.
# Needs: pip install 'offline-protocol-sdk[lan]'
bridge = DnsSdBridge(services, on_event=handle_event)
await bridge.start(address=pm.local_address, port=pm.peer_stream.listen_port)
...
await bridge.stop()   # before pm.stop()
```

A LAN import arrives at `handle_event` as a `service_discovered` event with
`source: "lan"` and is kept in `bridge.lan_services()`; it is an unsigned
claim by whoever answered on the segment, never a discovery, and the bridge
never registers it with the engine. Registrations that do not fit the record
(a service id over 200 bytes, a capability key DNS-SD cannot carry, a record
over 1300 bytes) are kept on the mesh and not published, with a warning. The
mapping is [docs/spec/dns-sd-mapping.md](../../docs/spec/dns-sd-mapping.md).

## Architecture

```
offline_protocol_sdk/
├── offline_protocol.py      # Auto-generated UniFFI bindings (DO NOT EDIT)
├── protocol_manager.py      # High-level wrapper (processing loop, lifecycle)
├── internet_manager.py      # WebSocket transport (websockets library)
├── peer_stream_manager.py   # TCP peer streams + DNS-SD (the wifi_direct slot)
├── gateway_manager.py       # Gateway-daemon client over TCP (the reticulum slot)
├── gateway_attach_policy.py # Its decisions and frame shapes, socket-free
├── gateway_verdict_tracker.py, presence_watch_policy.py
├── services.py              # Service registry wrappers with the copy the engine lacks
├── dnssd_bridge.py          # Services on the LAN: publish own, import neighbours' (optional extra `lan`)
├── ble_manager.py           # BLE transport (bleak library)
├── secure_storage.py        # MLS key storage (keyring library)
├── state_storage.py         # Restartable protocol state (application data)
└── transport_manager.py     # Base transport abstraction
```

### Transport Managers

| Transport | Library | Platforms | Notes |
|-----------|---------|-----------|-------|
| Internet/WebSocket | `websockets` | All | Primary transport for desktop |
| BLE | `bleak` | All | Central (scanner) role only; peripheral/GATT server requires `bless` |
| Peer stream (the `wifi_direct` slot) | `asyncio` sockets; `zeroconf` for LAN discovery (optional extra `lan`) | All | `PeerStreamManager`: TCP streams to configured `host:port` peers or hosts found over DNS-SD, each proved by the identity-assertion preamble ([spec](../../docs/spec/stream-framing.md)). Start it after `ProtocolManager.start()`; binds every interface unless `listen_host` narrows it |
| Reticulum (a gateway daemon) | `asyncio` sockets | All | `GatewayManager` as `pm.gateway` when `reticulum_enabled=True`: the [gateway-daemon contract](../../docs/spec/gateway-contract.md) over TCP to a daemon on local IP (`configure(daemon_address="localhost:4242")`, then `await pm.gateway.start()` after `pm.start()`). Attaches with a signed address declaration, settles each send on the gateway's verdict, watches presence. What answers is a daemon built to the contract; this package ships the device half. Apps driving the slot themselves replace the callback via `protocol.set_reticulum_transport_callback(...)` |
| Nostr | Built-in | All | Handled in Rust core (BIP-340 signing); `ProtocolManager` wires a stub callback when `nostr_enabled=True` — apps driving Nostr themselves replace it via `protocol.set_nostr_transport_callback(...)` |

### Secure Storage

MLS cryptographic key material is stored using the `keyring` library:

| Platform | Backend |
|----------|---------|
| macOS | Keychain |
| Linux | Secret Service (GNOME Keyring / KWallet) |
| Windows | Windows Credential Locker |

On a host with none of those available, `keyring` falls back to a null or
plaintext backend. `SecureStorage` logs a warning when it detects one, but it
does not refuse to run — and on Python that warning is louder than it looks.
The credential store also holds `protocol_state_record_key`, the per-install key
that seals pending messages, outbox entries, and media descriptors before they
reach `AppStateStorage`. On a plaintext backend that key sits in a readable
file, so the sealing gives you separation of *lifecycle* but not of
*confidentiality*: anyone who can read the credential store can open every
sealed protocol-state record. Install a real secret service (gnome-keyring,
kwallet) for any deployment where that matters, supply your own
`MlsStorageProvider`, or use the built-in file stores below.

### Run as a service: the local API

One process can own the engine and serve several local applications at once,
over JSON-RPC 2.0 on a WebSocket. The contract is
[the local API chapter](../../docs/spec/local-api.md); the reference server
ships in this package as `offline_protocol_sdk.local_api` and as the
`offline-protocol-service` command:

```bash
export OFFLINE_PROTOCOL_STORE_KEY="$(openssl rand -hex 32)"   # once; keep it
offline-protocol-service --config config.json \
    --mls-root /var/lib/example/keys --state-root /var/lib/example/state \
    --socket /run/example/api.sock --listen 0.0.0.0:7878 --peer 10.0.0.2:7878
```

`config.json` holds the `ProtocolConfig` fields by name; the socket is
created owner-only. A client opens the socket, sends `hello` with its
application id, and calls the engine's own methods by name:

```python
import asyncio, json
from websockets.asyncio.client import unix_connect

async def main():
    async with unix_connect("/run/example/api.sock", uri="ws://localhost/") as ws:
        await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "hello",
                                  "params": {"app_id": "notes"}}))
        print(json.loads(await ws.recv())["result"]["local_address"])

asyncio.run(main())
```

Every message a `notes` client sends is stamped with that id, and a
`message_received` for `notes` reaches only `notes` clients; what the server
holds for an application whose client is away, and what it never puts on the
wire, is the chapter's. `--policy policy.json` adds the optional rules
(`spaces`, `denied`); `--tcp PORT --token-file PATH` serves loopback TCP with
a per-launch token instead of the socket. See
[the bridge contract](../../docs/bridges/local-api.md) for what the server
owes.

The guide is [docs/local-api.md](../../docs/local-api.md). Two clients ship
as examples and are run by the test suite against an in-process server:
[`examples/local_api_client.py`](examples/local_api_client.py) (this package's
`websockets` dependency, Unix socket or TCP) and
[`examples/local-api/client.mjs`](../../examples/local-api/client.mjs) at the
repository root (Node 22 or later, no dependencies, TCP with the token).

### Headless hosts: the built-in file stores

A server or container usually has no secret service at all. Pass a store key
and `ProtocolManager` uses the SDK's built-in file stores instead of
`SecureStorage` and `AppStateStorage`: every MLS record is sealed on disk under
that key, and protocol state is written in the same format `AppStateStorage`
writes.

```python
pm = ProtocolManager(
    config,
    store_key_env="OFFLINE_PROTOCOL_STORE_KEY",  # 64 hex digits or base64
    mls_root="/var/lib/example/keys",            # or OFFLINE_PROTOCOL_MLS_ROOT
    state_root="/var/lib/example/state",         # or OFFLINE_PROTOCOL_STATE_ROOT
)
await pm.start()
```

- **The store key is 32 random bytes, generated once and kept.** Generate it
  with `openssl rand -hex 32`, and hand it to the process the way the service
  manager hands it any secret. Pass `store_key=<bytes>` instead if the host
  already holds it. Never derive it from a password or a host name.
- **`mls_root` holds this device's identity.** It must survive an upgrade,
  and losing it gives the device a new address. `state_root` is scoped to
  the installation. Keep them apart: the same directory for both, or one
  inside the other, is refused with `ProtocolError.InvalidArgument`. That
  includes two spellings of one directory on a volume that folds case
  (`keys` and `Keys` on macOS or Windows). That is found only once the
  directory exists, so the refusal leaves the directory it created, with
  no record written.
- **Moving an existing deployment onto the file stores starts a new
  identity.** Nothing is carried over from the keyring: the device gets a
  new address, and its sessions and queued messages stay with the old one.
  Point `state_root` at a fresh directory. Both modes read
  `OFFLINE_PROTOCOL_STATE_ROOT`, so a kept one is the easy mistake, and it
  is refused with `ProtocolError.InvalidConfiguration`: the new identity
  cannot unseal the old records, and the first restore would delete the
  messages it could not read. The same refusal covers a lost `mls_root`
  next to a surviving `state_root`, and it still applies after an attempt
  that failed part-way. It also covers an `mls_root` that holds an
  identity, but not the one that wrote the state: the first run over a pair
  binds the two directories to each other, so a `state_root` put back after
  a run over a fresh one is refused. A damaged key inside the store is not
  refused, then or on any later start over the same `mls_root`: the SDK
  replaces it and reports the
  messages it lost as failed. A directory the process cannot read is
  refused rather than taken for empty. No refusal changes or deletes a
  record.
- **`start()` raises instead of starting without the identity.** A wrong key
  raises `ProtocolError.InvalidConfiguration` and changes no record. A
  `state_root` that cannot be created also raises it, after the MLS store
  has created its own directory; fixing the root and retrying is safe. A
  directory another process holds raises `ProtocolError.InvalidState`. The
  keyring path logs its own initialisation failures and carries on; this one
  does not, because an operator who supplied a key asked for the sealed store.
- **One manager per account directory, and `close()` gives the directory
  up.** Each store holds a lock on its directory while open, in this
  process or another. `await pm.close()` stops the manager and releases
  both directories before it returns, whatever still refers to the manager
  or to `pm.protocol`, so a new manager can open them at once. A closed
  manager cannot be started again. Leaving
  `async with ProtocolManager(...) as pm` closes it, so two blocks over the
  same directories need nothing between them, and a block whose entry
  failed closes what that entry had opened. `close()` raises
  `ProtocolError.InvalidState` when the engine could not be stopped:
  nothing was released, and the call can be repeated.
- **`stop()` keeps the directories, for a restart in place.** Call `stop()`
  then `start()` on the same manager. A `stop()` or `close()` that was
  cancelled or raised part-way can be called again to finish; a `close()`
  cancelled while the core was already releasing the stores finishes by
  itself a moment later. Do not rely
  on dropping a stopped manager to release the directories: that happens
  when the interpreter frees the core object, which a kept `pm.protocol`,
  an event handler that refers back to the manager, or an exception whose
  traceback names it can each put off. A `start()` that raises releases
  the callbacks itself, whether the stores refused or the engine did; call
  `close()` if the manager is not going to be started again.
- **A forked child holds the directories.** A child made with a bare
  `os.fork()` while the stores are open inherits their locks and holds
  them until it exits, whatever the parent closes; start subprocesses with
  `subprocess` or the `spawn` method instead.

What the stores guarantee, and what a copied directory reveals, is in the
[MLS integration guide](../../docs/mls-integration.md#built-in-file-stores).

Restartable message-plane state is kept separately by `AppStateStorage`, outside
the credential store. The built-in stores derive an opaque account namespace
from both `app_id` and `user_id`, so multiple `ProtocolManager` instances do
not share keys, outboxes, or retry state.

Upgrading an install that predates namespacing keeps both halves of its state,
by two different mechanisms.

Its **MLS identity** is adopted by read-through: the first account to launch
claims the old, unscoped keyring service and inherits its identity, sessions,
and TOFU pins on demand. That service was shared by every account on the
install, so only one can inherit it — a second account starts from a fresh
identity and logs an error saying so. Inspect the outcome with
`SecureStorage(...).legacy_adoption`, or opt out with `adopt_legacy_store=False`.

Its **restartable delivery state** — outbox, pending queue, session and Welcome
lifecycles, peer key packages and capabilities, media descriptors, blocked
users, the Lamport clock — is swept out of the credential store into
`state_root` on the first launch of this release, and the credential-store copy
is deleted once the move is durable. The sweep is resumable and one-shot, and it
reads *through* the secure provider, so a `SecureStorage` built without a
namespace (which cannot claim the legacy service) finds nothing to sweep.

An account that loses the claim above gets neither: no identity *and* no
delivery state, so it comes up with an empty outbox and an empty block list,
every previously blocked peer unblocked.

Because the sweep deletes the credential-store copy, **downgrading is not a
rollback** — an older build reads the old location and finds none of it. Roll
forward, not back.

Python has no portable app container: `Application Support`, `LOCALAPPDATA`,
and XDG data directories commonly survive package removal. The SDK therefore
does not guess a persistent default. Pass `state_root=` or set
`OFFLINE_PROTOCOL_STATE_ROOT` to a directory owned by the installed
application, and make the installer remove that directory on uninstall:

```python
pm = ProtocolManager(
    config,
    state_root="/app/install-owned-data/offline-protocol",
)
```

Passing a custom `state_storage` bypasses `state_root`; the custom provider
then owns both account isolation and uninstall cleanup.

Custom integrations must provide both lifecycle-separated, account-isolated
interfaces:

```python
from offline_protocol_sdk.offline_protocol import (
    MlsStorageProvider,
    ProtocolStateStorageProvider,
)

class MySecureStorage(MlsStorageProvider):
    def store(self, key_type: str, key_id: str, data: list[int]) -> None: ...
    def load(self, key_type: str, key_id: str) -> list[int] | None: ...
    def delete(self, key_type: str, key_id: str) -> None: ...
    def list_keys(self, key_type: str) -> list[str]: ...

class MyStateStorage(ProtocolStateStorageProvider):
    def store(self, key_type: str, key_id: str, data: bytes) -> None: ...
    def load(self, key_type: str, key_id: str) -> bytes | None: ...
    def delete(self, key_type: str, key_id: str) -> None: ...
    def list_keys(self, key_type: str) -> list[str]: ...

pm = ProtocolManager(
    config,
    storage=MySecureStorage(),
    state_storage=MyStateStorage(),
)
```

`ProtocolManager` keeps both callback objects alive for the protocol lifetime.
Protocol state must live in application data rather than Keychain, Secret
Service, or Windows Credential Locker. A custom provider shared by multiple
accounts must apply the same `(app_id, user_id)` isolation itself.

## Telemetry

Opt-in by key. Once `enable_telemetry` has the key and app id the developer
portal issued, the SDK collects, batches and uploads accepted events to the
Offline Protocol ingest on its own background thread; the host platform is
filled in from `platform`, and the batch queue is durable from the first
batch because `start()` attaches the state store before anything is emitted.

```python
pm = ProtocolManager(config)
await pm.start()
pm.enable_telemetry("mp_…", "app_…", app_version="1.4.2")
print(pm.telemetry_stats())   # sent, accepted, dropped, buffered, last error
pm.disable_telemetry()        # final flush, then stop; stop() does this too
```

`flush_telemetry`, `end_telemetry_session`, `set_telemetry_enabled` and
`notify_app_state` complete the surface. What leaves the device, when, and how
to switch it off are in [docs/telemetry.md](../../docs/telemetry.md).

## Running the Example

```bash
# Terminal 1
python examples/basic_messaging.py --user alice --server ws://localhost:8080

# Terminal 2
python examples/basic_messaging.py --user bob --server ws://localhost:8080
```

## Development

```bash
# Install dev dependencies
pip install -e ".[dev]"

# Run tests
pytest tests/ -v

# Audit dependencies for CVEs
pip-audit --strict
```

## Platform Support

| Target | Architecture | Library |
|--------|-------------|---------|
| macOS | Apple Silicon (arm64) | `.dylib` |
| macOS | Intel (x86_64) | `.dylib` |
| Linux | x86_64 | `.so` |
| Linux | aarch64 | `.so` |
| Windows | x86_64 | `.dll` |

## License

Copyright © 2025-2026 Offline Protocol, Inc.

The Offline Protocol SDK is **dual-licensed**:

- **AGPL-3.0-only** — see [LICENSE](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE).
  Free for use in projects that comply with AGPL-3.0, including its network-use
  source-disclosure requirement (section 13).
- **Commercial License** — for proprietary applications that cannot or do not wish to
  comply with the AGPL. See [LICENSE-COMMERCIAL.md](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/LICENSE-COMMERCIAL.md)
  (contact legal@offlineprotocol.com).

You may use the SDK under **either** license; you do not need both.

Both license texts, along with `THIRD-PARTY-NOTICES.md`, are also installed with the
package under `offline_protocol_sdk-<version>.dist-info/licenses/`. The links above are
absolute because this file is the PyPI long description, and PyPI does not resolve
repository-relative links.

### Optional dependencies

`THIRD-PARTY-NOTICES.md` covers the Rust crates compiled into the native
library. The `lan` extra (`pip install 'offline-protocol-sdk[lan]'`) adds two
runtime dependencies that pip installs from PyPI and that are never
redistributed in this wheel: [python-zeroconf](https://pypi.org/project/zeroconf/)
(LGPL-2.1-or-later), used by `peer_stream_manager.py` and `dnssd_bridge.py`
for DNS-SD, and [ifaddr](https://pypi.org/project/ifaddr/) (MIT), used to
list the interface addresses a record publishes. Both are imported only when a
manager or bridge is asked to advertise or discover; the base install imports
neither.
