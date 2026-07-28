# Offline Protocol SDK — Python

Python bindings for the Offline Protocol SDK: offline-first mesh networking with MLS end-to-end encryption. Supports macOS, Linux, and Windows.

## Quick Start

### 1. Build the native library

```bash
cd bindings/python
bash scripts/build-desktop.sh
```

This compiles the Rust core for your platform and generates the Python FFI bindings.

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
    pending_ttl_ms=60000,
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

Restartable message-plane state is kept separately by `AppStateStorage`, outside
the credential store. The built-in stores derive an opaque account namespace
from both `app_id` and `user_id`, so multiple `ProtocolManager` instances do
not share keys, outboxes, or retry state. The new layout does not migrate data
from pre-split storage.

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
    def store(self, key_type: str, key_id: str, data: list[int]) -> None: ...
    def load(self, key_type: str, key_id: str) -> list[int] | None: ...
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
