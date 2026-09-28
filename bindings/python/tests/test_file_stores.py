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
import time
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
from offline_protocol_sdk import protocol_manager as manager_module
from offline_protocol_sdk.protocol_manager import (
    ProtocolManager,
    _decode_store_key_text,
    _run_here,
)
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


@_TRANSPORTS
@pytest.mark.asyncio
async def test_consecutive_context_managers_reopen_the_stores(
    tmp_path: Path, transports: dict
):
    """The `as` name outlives the block, and nothing here drops it: leaving
    the block closes the stores, so the next block opens them with every
    earlier manager still alive."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    managers = []
    for _ in range(3):
        async with ProtocolManager(
            _config(**transports), store_key=KEY, **roots
        ) as pm:
            managers.append(pm)
            assert pm.local_address == managers[0].local_address
    assert managers[0].local_address is not None


class _Service:
    """The usual shape: the owner of a manager handles its events."""

    def __init__(self, config: ProtocolConfig | None = None, **kwargs) -> None:
        self.pm = ProtocolManager(
            config if config is not None else _config(),
            event_handler=self.on_event,
            **kwargs,
        )

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
    """A stopped manager is freed by `del` alone, with nothing in between,
    even when no hand-off in `stop()` suspends.

    On Python 3.13+ a hand-off whose worker already finished completes in
    place, which was intermittent; a hand-off that runs here makes it
    certain. `stop()` holds no exception whose traceback names the manager,
    and it takes a loop turn of its own, so nothing the caller's task step
    holds outlives it.
    """
    monkeypatch.setattr(manager_module, "_hand_off", _run_here)
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


@pytest.mark.parametrize("holder", ["ble", "handler"])
@pytest.mark.asyncio
async def test_a_start_refused_by_the_stores_frees_the_core(tmp_path: Path, holder: str):
    """The callbacks are registered before the stores open. A refusal there
    (a wrong key, a held directory) must release them, or every refused
    attempt leaks a core, and a handler that reaches the manager leaks it too."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    await _run_once(store_key=KEY, **roots)

    wrong = bytes([0x33] * 32)
    if holder == "ble":
        pm = ProtocolManager(_config(ble_enabled=True), store_key=wrong, **roots)
    else:
        pm = _Service(store_key=wrong, **roots).pm
    with pytest.raises(ProtocolError.InvalidConfiguration):
        await pm.start()
    manager, core = weakref.ref(pm), weakref.ref(pm.protocol)
    del pm
    gc.collect()

    assert core() is None, "a refused start must not leave the core registered"
    assert manager() is None


@pytest.mark.asyncio
async def test_a_state_root_kept_from_the_keyring_is_refused(
    tmp_path: Path, in_memory_storage
):
    """Moving a deployment onto the file stores mints a new identity. Kept
    state from the old one cannot be unsealed and would be deleted by the
    first restore, so the start is refused and nothing changes."""
    state_root = tmp_path / "state"
    old = ProtocolManager(_config(), storage=in_memory_storage, state_root=state_root)
    await old.start()
    old_address = old.local_address
    await old.stop()
    del old
    gc.collect()
    before = sorted(p.relative_to(state_root) for p in state_root.rglob("*"))

    pm = ProtocolManager(
        _config(), store_key=KEY, mls_root=tmp_path / "mls", state_root=state_root
    )
    with pytest.raises(ProtocolError.InvalidConfiguration, match="fresh directory"):
        await pm.start()
    assert not (tmp_path / "mls").exists() or not any((tmp_path / "mls").iterdir())
    assert sorted(p.relative_to(state_root) for p in state_root.rglob("*")) == before
    del pm
    gc.collect()

    # A fresh state root is the documented way over, with a new address.
    fresh = await _run_once(
        store_key=KEY, mls_root=tmp_path / "mls", state_root=tmp_path / "fresh-state"
    )
    assert fresh is not None and fresh != old_address


@pytest.mark.parametrize(
    "mls, state",
    [("one", "one"), ("one/mls", "one"), ("one", "one/state"), ("one", "two/../one")],
    ids=["equal", "mls-inside-state", "state-inside-mls", "dot-dot"],
)
@pytest.mark.asyncio
async def test_overlapping_roots_are_refused(tmp_path: Path, mls: str, state: str):
    pm = ProtocolManager(
        _config(), store_key=KEY, mls_root=tmp_path / mls, state_root=tmp_path / state
    )
    with pytest.raises(ProtocolError.InvalidArgument, match="different directories"):
        await pm.start()
    assert not any(tmp_path.iterdir()), "a refused start creates nothing"


# -- close(): release that does not wait for the interpreter -----------------


@pytest.mark.parametrize("holder", ["name", "protocol", "handler", "exception"])
@_TRANSPORTS
@pytest.mark.asyncio
async def test_close_releases_the_stores_whatever_still_holds_the_manager(
    tmp_path: Path, transports: dict, holder: str
):
    """Every way a stopped manager was once kept alive, all held on purpose,
    with no `del`, no collector pass and no loop turn before the reopen."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    kept: list = []
    if holder == "handler":
        service = _Service(_config(**transports), store_key=KEY, **roots)
        pm = service.pm
        kept.append(service)
    else:
        pm = ProtocolManager(_config(**transports), store_key=KEY, **roots)
    await pm.start()
    address = pm.local_address
    if holder == "protocol":
        kept.append(pm.protocol)
    if holder == "exception":

        def fails_with_the_manager_in_its_frame(manager: ProtocolManager) -> None:
            raise RuntimeError(f"raised beside {type(manager).__name__}")

        try:
            fails_with_the_manager_in_its_frame(pm)
        except RuntimeError as err:
            kept.append(err)
        assert kept[0].__traceback__.tb_next.tb_frame.f_locals["manager"] is pm

    await pm.close()

    second = ProtocolManager(_config(**transports), store_key=KEY, **roots)
    await second.start()
    try:
        assert second.local_address == address
        assert kept is not None and pm is not None
    finally:
        await second.close()


@pytest.mark.asyncio
async def test_a_closed_manager_cannot_be_started(tmp_path: Path):
    """The core gave up its stores, so a restart would run an engine that
    cannot write. Refused by name instead, and closing again is harmless."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    await pm.start()
    await pm.close()
    await pm.close()
    with pytest.raises(RuntimeError, match="closed"):
        await pm.start()
    with pytest.raises(RuntimeError, match="closed"):
        async with pm:
            pass
    with pytest.raises(ProtocolError.InvalidState, match="close_file_stores"):
        pm.protocol.start()


@pytest.mark.asyncio
async def test_close_before_start_is_harmless(tmp_path: Path):
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    await pm.close()
    assert not any(tmp_path.iterdir()), "nothing was opened, so nothing is created"
    assert await _run_once(store_key=KEY, **roots) is not None


@pytest.mark.asyncio
async def test_close_finishes_a_stop_cancelled_inside_a_transport(tmp_path: Path):
    """A shutdown deadline, then `close()`: the teardown is finished and the
    directories are free, in the same task step that caught the timeout."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = await _listening_peer_stream(roots)
    address = pm.local_address
    close = pm.peer_stream._close_everything

    async def a_peer_that_does_not_let_go() -> None:
        await asyncio.sleep(30)
        await close()

    pm.peer_stream._close_everything = a_peer_that_does_not_let_go
    with pytest.raises(asyncio.TimeoutError):
        await asyncio.wait_for(pm.close(), 0.1)
    pm.peer_stream._close_everything = close

    await pm.close()
    assert pm.peer_stream.state.value == "stopped"

    second = ProtocolManager(_config(), store_key=KEY, **roots)
    await second.start()
    try:
        assert second.local_address == address
    finally:
        await second.close()


@pytest.mark.asyncio
async def test_close_releases_the_stores_after_the_engine_refused_to_start(
    tmp_path: Path,
):
    """The stores are open by the time the engine starts. A manager whose
    engine refused never ran, and `close()` still has to give them up."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(ble_enabled=True), store_key=KEY, **roots)
    real_start = pm._protocol.start

    def refuse() -> None:
        raise RuntimeError("engine start refused")

    pm._protocol.start = refuse
    with pytest.raises(RuntimeError, match="engine start refused"):
        await pm.start()
    pm._protocol.start = real_start
    assert pm.protocol.is_mls_initialized()

    await pm.close()
    assert await _run_once(store_key=KEY, **roots) is not None
    assert pm is not None


@pytest.mark.asyncio
async def test_close_reports_an_engine_that_would_not_stop(tmp_path: Path):
    """A running engine writes to its stores, so they are not released under
    it. The refusal is raised, nothing is released, and the call can be
    repeated once the engine stops."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    await pm.start()
    real_stop = pm._protocol.stop

    def refuse() -> None:
        raise RuntimeError("engine stop refused")

    pm._protocol.stop = refuse
    with pytest.raises(ProtocolError.InvalidState, match="after stop"):
        await pm.close()
    assert not pm._closed, "a manager whose stores were not released is not closed"
    other = ProtocolManager(_config(), store_key=KEY, **roots)
    with pytest.raises(ProtocolError.InvalidState, match="already open"):
        await other.start()

    pm._protocol.stop = real_stop
    await pm.close()
    await other.start()
    await other.close()


@pytest.mark.asyncio
async def test_a_keyring_manager_is_stopped_by_the_block_and_can_be_entered_again(
    tmp_path: Path, in_memory_storage
):
    """Only the file stores need closing, so every other manager keeps the
    exit it always had. `close()` is still final for it."""
    pm = ProtocolManager(
        _config(), storage=in_memory_storage, state_root=tmp_path / "state"
    )
    async with pm:
        address = pm.local_address
    async with pm:
        assert pm.local_address == address

    await pm.close()
    with pytest.raises(RuntimeError, match="closed"):
        await pm.start()


# -- stop() always yields -----------------------------------------------------


@pytest.mark.parametrize("retries_with", ["stop", "close"])
@pytest.mark.asyncio
async def test_a_retry_that_never_suspends_still_frees_the_manager(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, retries_with: str
):
    """A caller retrying after a cancelled `stop()` is inside the task step
    that threw the CancelledError, and that step holds the exception, whose
    traceback holds the manager, until the task next suspends. On Python
    3.13+ the telemetry hand-off can finish in place, so nothing in the retry
    suspended and the manager outlived `del`: the next manager was refused
    its directories about one run in fifty. A hand-off that never suspends
    makes that certain, on every Python."""
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
    del pm.peer_stream._close_everything, close, a_peer_that_does_not_let_go

    # A context of its own: `undo()` on the fixture would also take back
    # the stub that keeps the suite off the real keychain.
    with monkeypatch.context() as patch:
        patch.setattr(manager_module, "_hand_off", _run_here)
        await getattr(pm, retries_with)()

    manager = weakref.ref(pm)
    del pm
    assert manager() is None, "the retry must not leave the manager pinned"
    assert await _run_once(store_key=KEY, **roots) == address


@pytest.mark.asyncio
async def test_no_tick_runs_after_stop_was_called(tmp_path: Path, in_memory_storage):
    """The caller is woken in the loop iteration in which the process loop's
    timer fires, as any caller woken by a timer or by I/O can be. The process
    loop is cancelled before `stop()` first suspends, so it does not run once
    more, and deliver one more message, to a caller that has started tearing
    its own side down."""
    pm = ProtocolManager(
        _config(), storage=in_memory_storage, state_root=tmp_path / "state"
    )
    await pm.start()
    await asyncio.sleep(0)
    ticks_after_stop: list[bool] = []
    stop_called = False
    real = pm._protocol.process

    def counting() -> None:
        ticks_after_stop.append(stop_called)
        real()

    pm._protocol.process = counting
    time.sleep(0.12)  # the process loop's 100 ms timer is now overdue
    loop = asyncio.get_running_loop()
    woken = loop.create_future()
    loop.call_soon(woken.set_result, None)
    await woken
    stop_called = True
    await pm.stop()
    del pm._protocol.process

    assert not any(ticks_after_stop)


@pytest.mark.asyncio
async def test_a_cancel_while_stop_waits_for_the_process_loop_is_the_callers(
    tmp_path: Path,
):
    """`stop()` cancels the process loop and waits for it. A cancel that
    arrives during that wait is a shutdown deadline, and must reach the
    caller: awaiting the cancelled task instead would raise for both, and a
    handler that swallowed the task's cancel would swallow the deadline."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    await pm.start()
    pm._process_task.cancel()
    await asyncio.wait({pm._process_task})

    async def slow_to_finish() -> None:
        try:
            await asyncio.sleep(30)
        except asyncio.CancelledError:
            await asyncio.sleep(0.3)

    pm._process_task = asyncio.ensure_future(slow_to_finish())
    await asyncio.sleep(0)
    with pytest.raises(asyncio.TimeoutError):
        await asyncio.wait_for(pm.stop(), 0.05)
    await pm.close()


@pytest.mark.asyncio
async def test_a_block_whose_entry_failed_releases_the_stores(tmp_path: Path):
    """`__aexit__` does not run for an entry that failed, and the inline
    form leaves no name to close. The stores are open by the time the engine
    starts, so a start refused there has to give them up itself: here the
    exception is kept, and the manager with it, and the next block opens."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(ble_enabled=True), store_key=KEY, **roots)

    def refuse() -> None:
        raise RuntimeError("engine start refused")

    pm._protocol.start = refuse
    kept = None
    try:
        async with pm:
            pytest.fail("the block must not be entered")
    except RuntimeError as err:
        kept = err
        async with ProtocolManager(_config(), store_key=KEY, **roots) as second:
            assert second.local_address is not None
    assert kept is not None and pm._closed


@pytest.mark.asyncio
async def test_a_close_cancelled_while_the_core_releases_still_closes(tmp_path: Path):
    """A cancel does not stop the core: the stores are released a moment
    later whether or not anyone waits. The manager has to end up closed to
    match, or it reports itself startable over stores it no longer has."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    await pm.start()
    real = pm._protocol.close_file_stores

    def slow_release() -> None:
        time.sleep(0.3)
        real()

    pm._protocol.close_file_stores = slow_release
    with pytest.raises(asyncio.TimeoutError):
        await asyncio.wait_for(pm.close(), 0.05)
    assert not pm._closed, "not closed before the core has released the stores"

    await asyncio.sleep(0.5)
    del pm._protocol.close_file_stores
    assert pm._closed
    with pytest.raises(RuntimeError, match="closed"):
        await pm.start()
    assert await _run_once(store_key=KEY, **roots) is not None


@pytest.mark.asyncio
async def test_close_releases_the_stores_off_the_loop(tmp_path: Path):
    """The core's release can block for the uploader's final flush, up to
    three seconds. Everything else on the loop keeps running meanwhile."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    await pm.start()
    real = pm._protocol.close_file_stores

    def slow_release() -> None:
        time.sleep(0.3)
        real()

    pm._protocol.close_file_stores = slow_release
    turns = 0

    async def something_else() -> None:
        nonlocal turns
        while True:
            await asyncio.sleep(0.02)
            turns += 1

    other = asyncio.ensure_future(something_else())
    await pm.stop()
    before = turns
    await pm.close()
    other.cancel()
    await asyncio.wait({other})
    del pm._protocol.close_file_stores

    assert turns - before >= 5, "the loop was blocked while the core released"


@pytest.mark.asyncio
async def test_close_works_on_a_loop_whose_executor_was_shut_down(tmp_path: Path):
    """A teardown must be able to finish on any loop it is asked to finish
    on. A loop that has shut its executor down refuses every hand-off, and
    the stores would otherwise stay held through every retry."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(ble_enabled=True), store_key=KEY, **roots)
    await pm.start()
    address = pm.local_address
    await asyncio.get_running_loop().shutdown_default_executor()

    await pm.close()
    assert pm._closed and not pm._teardown_pending

    second = ProtocolManager(_config(), store_key=KEY, **roots)
    await second.start()
    try:
        assert second.local_address == address
    finally:
        await second.close()


@pytest.mark.asyncio
async def test_close_finishes_a_teardown_that_stop_could_not(tmp_path: Path):
    """`stop()` holds the callbacks back when a teardown step raised, since
    a running engine may still call them. Once the core has released the
    stores the engine runs no more, so a closed manager holds nothing: it
    is freed by `del` alone, with a transport that pins it enabled."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(ble_enabled=True), store_key=KEY, **roots)
    await pm.start()

    def refuse() -> None:
        raise RuntimeError("telemetry would not disable")

    pm.disable_telemetry = refuse
    await pm.stop()
    assert pm._teardown_pending

    await pm.close()
    del pm.disable_telemetry
    assert pm._closed and not pm._teardown_pending
    manager = weakref.ref(pm)
    del pm
    assert manager() is None


@pytest.mark.asyncio
async def test_a_keyring_close_reports_an_engine_that_would_not_stop(
    tmp_path: Path, in_memory_storage
):
    """Without the file stores the core has nothing to refuse to release,
    so the manager reports the engine itself. Not closed, and not silent."""
    pm = ProtocolManager(
        _config(), storage=in_memory_storage, state_root=tmp_path / "state"
    )
    await pm.start()
    real_stop = pm._protocol.stop

    def refuse() -> None:
        raise RuntimeError("engine stop refused")

    pm._protocol.stop = refuse
    with pytest.raises(ProtocolError.InvalidState, match="could not stop the engine"):
        await pm.close()
    assert not pm._closed

    pm._protocol.stop = real_stop
    await pm.close()
    assert pm._closed


@pytest.mark.parametrize("started", [False, True], ids=["never-started", "started"])
@pytest.mark.asyncio
async def test_a_closed_manager_keeps_no_store_key(tmp_path: Path, started: bool):
    """The key is held until the core takes it. A manager closed before that
    will never hand it over, so it does not keep it either."""
    roots = dict(mls_root=tmp_path / "mls", state_root=tmp_path / "state")
    pm = ProtocolManager(_config(), store_key=KEY, **roots)
    assert pm._file_stores._store_key == KEY
    if started:
        await pm.start()
    await pm.close()
    assert pm._file_stores._store_key is None
