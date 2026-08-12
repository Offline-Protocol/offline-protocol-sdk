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

**Prerequisites:** Rust toolchain, `uniffi-bindgen` (`cargo install uniffi --version 0.30.0 --features cli`).

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
    pending_ttl_ms=1_800_000,  # 30 min (the SDK default)
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

## Architecture

```
offline_protocol_sdk/
├── offline_protocol.py      # Auto-generated UniFFI bindings (DO NOT EDIT)
├── protocol_manager.py      # High-level wrapper (processing loop, lifecycle)
├── internet_manager.py      # WebSocket transport (websockets library)
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
| WiFi Direct | — | — | Not implemented on desktop |
| Reticulum | Built-in | All | Handled in Rust core; `ProtocolManager` wires a stub callback when `reticulum_enabled=True` — apps driving Reticulum themselves replace it via `protocol.set_reticulum_transport_callback(...)` |
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
kwallet) for any deployment where that matters, or supply your own
`MlsStorageProvider`.

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
