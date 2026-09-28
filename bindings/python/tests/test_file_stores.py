"""ProtocolManager over the SDK's built-in file stores.

Selected by ``store_key`` or ``store_key_env``; the core opens the stores in
``start()``. A headless host has no keyring, so these stores are how its
identity survives a restart, and a wrong key must stop the service rather
than start it without that identity.
"""

from __future__ import annotations

import asyncio
import base64
import gc
import weakref
from pathlib import Path

import pytest

from offline_protocol_sdk.offline_protocol import (
    NostrTransportCallback,
    OverflowPolicy,
    ProtocolConfig,
    ProtocolError,
    ReticulumTransportCallback,
)
from offline_protocol_sdk.protocol_manager import ProtocolManager, _decode_store_key_text
from offline_protocol_sdk.storage_namespace import account_storage_namespace

KEY = bytes(range(1, 33))


def _config(
    profile: str = "file-store-user",
    *,
    ble_enabled: bool = False,
    wifi_direct_enabled: bool = False,
) -> ProtocolConfig:
    return ProtocolConfig(
        app_id="test-app",
        profile=profile,
        ble_enabled=ble_enabled,
        wifi_direct_enabled=wifi_direct_enabled,
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


#: Every transport whose manager the core's callback holds. With BLE or the
#: peer stream on, a stopped manager used to stay alive through a reference
#: cycle that runs through Rust, holding the directory locks for good.
_TRANSPORTS = pytest.mark.parametrize(
    "transports",
    [{}, {"wifi_direct_enabled": True}, {"ble_enabled": True}],
    ids=["internet", "peer-stream", "ble"],
)


@_TRANSPORTS
@pytest.mark.asyncio
async def test_a_stopped_and_dropped_manager_releases_the_stores(
    tmp_path: Path, transports: dict
):
    """No `gc.collect()`: dropping the last reference must be enough."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    first = ProtocolManager(_config(**transports), store_key=KEY, **roots)
    await first.start()
    address = first.local_address
    await first.stop()
    del first

    second = ProtocolManager(_config(**transports), store_key=KEY, **roots)
    await second.start()
    try:
        assert second.local_address == address
    finally:
        await second.stop()


@pytest.mark.asyncio
async def test_consecutive_context_managers_reopen_the_stores(tmp_path: Path):
    """The `as` name outlives the block, so it has to be dropped between them."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    addresses = []
    for _ in range(2):
        async with ProtocolManager(
            _config(wifi_direct_enabled=True), store_key=KEY, **roots
        ) as pm:
            addresses.append(pm.local_address)
        del pm
    assert addresses[0] is not None and addresses[0] == addresses[1]


class _Service:
    """The usual shape: the owner of a manager handles its events."""

    def __init__(self, **kwargs) -> None:
        self.pm = ProtocolManager(_config(), event_handler=self.on_event, **kwargs)

    def on_event(self, event: dict) -> None:
        pass


@pytest.mark.asyncio
async def test_a_handler_that_reaches_the_manager_does_not_pin_the_stores(
    tmp_path: Path,
):
    """The registered event callback holds the handler, and the handler holds
    the manager: a cycle through Rust that the collector could not break."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    service = _Service(store_key=KEY, **roots)
    await service.pm.start()
    address = service.pm.local_address
    await service.pm.stop()
    del service
    gc.collect()

    assert await _run_once(store_key=KEY, **roots) == address


@_TRANSPORTS
@pytest.mark.asyncio
async def test_a_start_that_fails_after_the_stores_opened_releases_them(
    tmp_path: Path, transports: dict
):
    """`stop()` does nothing for a manager that never ran, so `start()` has to
    release the callbacks itself when the engine refuses to start."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(**transports), store_key=KEY, **roots)

    def refuse() -> None:
        raise RuntimeError("engine start refused")

    pm._protocol.start = refuse
    with pytest.raises(RuntimeError, match="engine start refused"):
        await pm.start()
    assert pm.protocol.is_mls_initialized()
    del pm._protocol.start, pm

    assert await _run_once(store_key=KEY, **roots) is not None


@pytest.mark.asyncio
async def test_a_stopped_manager_is_freed_without_a_loop_turn(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    """`stop()` must not leave a caught exception whose traceback holds the
    manager: it would pin the stores until the caller next yields.

    On Python 3.13+ the telemetry hand-off can complete without suspending,
    so `stop()` finishes in the same task step that cancelled the process
    loop. That was intermittent; a hand-off that never suspends makes it
    certain.
    """

    async def without_suspending(func, /, *args, **kwargs):
        return func(*args, **kwargs)

    monkeypatch.setattr(asyncio, "to_thread", without_suspending)
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    await pm.start()
    await pm.stop()
    ref = weakref.ref(pm)
    del pm
    assert ref() is None, "a stopped manager must be freed by `del` alone"


class _OwnNostr(NostrTransportCallback):
    """An application driving Nostr itself, as the stub's docstring asks."""

    def __init__(self, core) -> None:
        self.core = core

    def on_messages_available(self) -> None:
        pass


class _OwnReticulum(ReticulumTransportCallback):
    def __init__(self, core) -> None:
        self.core = core

    def on_messages_available(self) -> None:
        pass


@pytest.mark.parametrize(
    "callback, setter",
    [
        (_OwnNostr, "set_nostr_transport_callback"),
        (_OwnReticulum, "set_reticulum_transport_callback"),
    ],
    ids=["nostr", "reticulum"],
)
@pytest.mark.asyncio
async def test_a_callback_the_application_registered_does_not_pin_the_stores(
    tmp_path: Path, callback, setter: str
):
    """It needs only the core to drain its transport, and that is enough to
    pin the core, and with it the stores, through Rust."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    await pm.start()
    address = pm.local_address
    getattr(pm.protocol, setter)(callback(pm.protocol))
    await pm.stop()
    del pm
    gc.collect()

    assert await _run_once(store_key=KEY, **roots) == address


@pytest.mark.parametrize("interruption", ["cancelled", "engine-refused"])
@pytest.mark.asyncio
async def test_an_interrupted_stop_can_be_finished(tmp_path: Path, interruption: str):
    """`_running` is cleared first, so a retry must still see the teardown owed."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(ble_enabled=True), store_key=KEY, **roots)
    await pm.start()
    address = pm.local_address

    if interruption == "cancelled":
        release = asyncio.Event()

        async def slow_ble_stop() -> None:
            await release.wait()

        pm.ble.stop = slow_ble_stop
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(pm.stop(), 0.1)
        del pm.ble.stop
    else:
        real_stop = pm._protocol.stop

        def refuse_once() -> None:
            del pm._protocol.stop
            raise RuntimeError("engine stop refused")

        pm._protocol.stop = refuse_once
        await pm.stop()
        del real_stop

    await pm.stop()
    del pm
    gc.collect()

    assert await _run_once(store_key=KEY, **roots) == address


async def _listening_peer_stream(roots: dict) -> ProtocolManager:
    pm = ProtocolManager(_config(wifi_direct_enabled=True), store_key=KEY, **roots)
    await pm.start()
    pm.peer_stream.configure(listen_host="127.0.0.1", listen_port=0)
    await pm.peer_stream.start()
    return pm


@pytest.mark.asyncio
async def test_a_stop_cancelled_inside_a_transport_can_be_finished(tmp_path: Path):
    """The deadline lands where the waiting is: inside a transport's own
    `stop()`, which has already moved to STOPPING. The retry has to enter it
    again, or the listener stays open and holds the manager, and the stores."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = await _listening_peer_stream(roots)
    address = pm.local_address

    close = pm.peer_stream._close_everything

    async def a_peer_that_does_not_let_go() -> None:
        await asyncio.sleep(30)
        await close()

    pm.peer_stream._close_everything = a_peer_that_does_not_let_go
    with pytest.raises(asyncio.TimeoutError):
        await asyncio.wait_for(pm.stop(), 0.1)
    assert pm.peer_stream.state.value == "stopping"
    del pm.peer_stream._close_everything, close, a_peer_that_does_not_let_go

    await pm.stop()
    assert pm.peer_stream.state.value == "stopped"
    del pm
    gc.collect()

    assert await _run_once(store_key=KEY, **roots) == address


@pytest.mark.asyncio
async def test_overlapping_stops_tear_down_once_in_order(tmp_path: Path):
    """A second `stop()` waits for the first rather than stopping the engine
    while a transport is still closing."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = await _listening_peer_stream(roots)
    order: list[str] = []

    close = pm.peer_stream._close_everything

    async def slow_close() -> None:
        await asyncio.sleep(0.2)
        await close()
        order.append("transport closed")

    engine_stop = pm._protocol.stop

    def recording_engine_stop() -> None:
        order.append(f"engine stopped (transport {pm.peer_stream.state.value})")
        engine_stop()

    pm.peer_stream._close_everything = slow_close
    pm._protocol.stop = recording_engine_stop
    first = asyncio.ensure_future(pm.stop())
    await asyncio.sleep(0.02)
    await pm.stop()
    assert first.done()
    await first

    assert order == ["transport closed", "engine stopped (transport stopped)"]
    del pm.peer_stream._close_everything, pm._protocol.stop, pm, first
    gc.collect()


@pytest.mark.asyncio
async def test_the_key_is_taken_when_the_manager_is_built(tmp_path: Path):
    """A caller that reuses its buffer before `start()` must not change the key."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    buffer = bytearray(KEY)
    pm = ProtocolManager(_config(), store_key=buffer, **roots)
    buffer[:] = bytes([0x77] * 32)
    await pm.start()
    first = pm.local_address
    await pm.stop()
    del pm

    assert await _run_once(store_key=KEY, **roots) == first


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


@_TRANSPORTS
@pytest.mark.asyncio
async def test_stop_then_start_reuses_the_open_stores(tmp_path: Path, transports: dict):
    pm = ProtocolManager(
        _config(**transports),
        store_key=KEY,
        mls_root=tmp_path / "mls",
        state_root=tmp_path / "state",
    )
    await pm.start()
    address = pm.local_address
    await pm.stop()
    await pm.start()
    assert pm.local_address == address
    await pm.stop()


def test_roots_fall_back_to_the_environment(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    # A service manager's environment file can leave a trailing newline.
    monkeypatch.setenv("OFFLINE_PROTOCOL_MLS_ROOT", f"{tmp_path / 'mls'}\n")
    # conftest sets OFFLINE_PROTOCOL_STATE_ROOT for every test.
    pm = ProtocolManager(_config(), store_key=KEY)
    assert pm._file_stores is not None
    assert pm._file_stores._mls_root == str(tmp_path / "mls")

    monkeypatch.delenv("OFFLINE_PROTOCOL_MLS_ROOT")
    with pytest.raises(ValueError, match="OFFLINE_PROTOCOL_MLS_ROOT"):
        ProtocolManager(_config(), store_key=KEY)

    monkeypatch.setenv("OFFLINE_PROTOCOL_MLS_ROOT", "   ")
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
        (dict(store_key=bytes(32)), ValueError),
        (dict(store_key_env=""), ValueError),
        (dict(store_key_env="  "), ValueError),
        (dict(store_key=KEY, storage=object()), ValueError),
        (dict(store_key=KEY, state_storage=object()), ValueError),
    ],
    ids=[
        "both-keys",
        "short",
        "long",
        "text",
        "all-zero",
        "empty-env-name",
        "blank-env-name",
        "with-storage",
        "with-state-storage",
    ],
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
