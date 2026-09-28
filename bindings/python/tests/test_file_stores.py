"""ProtocolManager over the SDK's built-in file stores.

Selected by ``store_key`` or ``store_key_env``; the core opens the stores in
``start()``. A headless host has no keyring, so these stores are how its
identity survives a restart, and a wrong key must stop the service rather
than start it without that identity.
"""

from __future__ import annotations

import base64
import gc
from pathlib import Path

import pytest

from offline_protocol_sdk.offline_protocol import OverflowPolicy, ProtocolConfig, ProtocolError
from offline_protocol_sdk.protocol_manager import ProtocolManager, _decode_store_key_text
from offline_protocol_sdk.storage_namespace import account_storage_namespace

KEY = bytes(range(1, 33))


def _config(profile: str = "file-store-user") -> ProtocolConfig:
    return ProtocolConfig(
        app_id="test-app",
        profile=profile,
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=True,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=True,
        initial_ttl=3,
        encryption_enabled=True,
        auto_key_exchange=False,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )


async def _run_once(**kwargs) -> str | None:
    """Starts a manager, returns its address, and releases its stores."""
    pm = ProtocolManager(_config(), **kwargs)
    try:
        await pm.start()
        return pm.local_address
    finally:
        await pm.stop()
        # The stores hold a directory lock until the core object is freed.
        del pm
        gc.collect()


@pytest.mark.asyncio
async def test_the_identity_survives_a_restart(tmp_path: Path):
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    first = await _run_once(store_key=KEY, **roots)
    assert first is not None and first.startswith("off1")
    namespace = account_storage_namespace("test-app", "file-store-user")
    assert [p.name for p in (tmp_path / "mls").iterdir()] == [namespace]
    assert [p.name for p in (tmp_path / "state").iterdir()] == [namespace]

    assert await _run_once(store_key=KEY, **roots) == first


@pytest.mark.asyncio
async def test_a_key_from_the_environment_opens_the_same_store(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    first = await _run_once(store_key=KEY, **roots)
    for text in (KEY.hex(), KEY.hex().upper(), base64.b64encode(KEY).decode()):
        monkeypatch.setenv("OP_TEST_STORE_KEY", f" {text}\n")
        assert await _run_once(store_key_env="OP_TEST_STORE_KEY", **roots) == first, text


@pytest.mark.asyncio
async def test_a_wrong_key_fails_start_instead_of_running_without_the_identity(
    tmp_path: Path,
):
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    await _run_once(store_key=KEY, **roots)

    pm = ProtocolManager(_config(), store_key=bytes([0x33] * 32), **roots)
    with pytest.raises(ProtocolError.InvalidConfiguration, match="store key"):
        await pm.start()
    assert pm._running is False
    assert pm.local_address is None
    assert not pm.protocol.is_mls_initialized()


@pytest.mark.asyncio
async def test_a_second_manager_over_the_same_directory_fails_start(tmp_path: Path):
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    first = ProtocolManager(_config(), store_key=KEY, **roots)
    await first.start()
    try:
        second = ProtocolManager(_config(), store_key=KEY, **roots)
        with pytest.raises(ProtocolError.InvalidState, match="already open"):
            await second.start()
        assert second._running is False
    finally:
        await first.stop()


@pytest.mark.asyncio
async def test_stop_then_start_reuses_the_open_stores(tmp_path: Path):
    pm = ProtocolManager(
        _config(), store_key=KEY, mls_root=tmp_path / "mls", state_root=tmp_path / "state"
    )
    await pm.start()
    address = pm.local_address
    await pm.stop()
    await pm.start()
    assert pm.local_address == address
    await pm.stop()


def test_roots_fall_back_to_the_environment(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("OFFLINE_PROTOCOL_MLS_ROOT", str(tmp_path / "mls"))
    # conftest sets OFFLINE_PROTOCOL_STATE_ROOT for every test.
    pm = ProtocolManager(_config(), store_key=KEY)
    assert pm._file_stores is not None
    assert pm._file_stores._mls_root == str(tmp_path / "mls")

    monkeypatch.delenv("OFFLINE_PROTOCOL_MLS_ROOT")
    with pytest.raises(ValueError, match="OFFLINE_PROTOCOL_MLS_ROOT"):
        ProtocolManager(_config(), store_key=KEY)


def test_the_file_stores_replace_the_keyring_stores(tmp_path: Path):
    pm = ProtocolManager(
        _config(), store_key=KEY, mls_root=tmp_path / "mls", state_root=tmp_path / "state"
    )
    assert pm._storage is None and pm._state_storage is None


@pytest.mark.parametrize(
    "kwargs, error",
    [
        (dict(store_key=KEY, store_key_env="X"), ValueError),
        (dict(store_key=KEY[:31]), ValueError),
        (dict(store_key=KEY + b"\x00"), ValueError),
        (dict(store_key=KEY.hex()), TypeError),
        (dict(store_key=KEY, storage=object()), ValueError),
        (dict(store_key=KEY, state_storage=object()), ValueError),
    ],
    ids=["both-keys", "short", "long", "text", "with-storage", "with-state-storage"],
)
def test_conflicting_or_malformed_arguments_are_refused(tmp_path: Path, kwargs, error):
    kwargs.setdefault("mls_root", tmp_path / "mls")
    with pytest.raises(error):
        ProtocolManager(_config(), **kwargs)


def test_key_text_accepts_what_the_rust_provider_accepts():
    b64 = base64.b64encode(KEY).decode()
    for text in (KEY.hex(), KEY.hex().upper(), b64, b64.rstrip("="), f"  {KEY.hex()}\n"):
        assert _decode_store_key_text("V", text) == KEY, text
    last = b64.rstrip("=")[-1]
    # 32 bytes leave two unused bits in the last base64 digit; the Rust
    # decoder refuses a digit that sets them, so this side must as well.
    non_canonical = b64.rstrip("=")[:-1] + chr(ord(last) + 1)
    for text in (
        bytes(31).hex(),
        (b"\x07" * 33).hex(),
        "",
        "correct horse battery staple",
        base64.b64encode(KEY[:31]).decode(),
        non_canonical,
    ):
        with pytest.raises(ValueError):
            _decode_store_key_text("V", text)
