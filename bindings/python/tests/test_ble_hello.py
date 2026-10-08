"""The Bluetooth LE hello, and the link attribution it exists for.

A peripheral attributes each Message write to a link, and hands the core the
link's id as the fragment's transport identity. The core refuses a hop-0
control frame (a key package) whose sender is not that identity, so a link
known only by a central UUID, or by the ``ble-peer`` placeholder when several
centrals are connected, can never start a session. These tests pin the
pieces that fix it: the peripheral binds a central to the address its hello
proves and feeds its fragments under that address; the central writes its
hello; a second link to one rotating peer is refused; a link CoreBluetooth
keeps after its peer is gone is found and dropped; and a Bluetooth stack that
never comes up fails the start instead of wedging the service.

All tests mock bless, bleak and the core, so they run without Bluetooth.
"""

from __future__ import annotations

import asyncio
import threading
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

import offline_protocol_sdk.ble_manager as ble_manager_module
import offline_protocol_sdk.ble_peripheral as peripheral_module
from offline_protocol_sdk.ble_manager import (
    DEVICE_ID_CHAR_UUID,
    HELLO_CHAR_UUID,
    HELLO_MAX_LEN,
    HELLO_MIN_LEN,
    IDENTITY_CHAR_UUID,
    MESSAGE_CHAR_UUID,
    BleManager,
)
from offline_protocol_sdk.ble_peripheral import BlePeripheral
from offline_protocol_sdk.transport_manager import TransportError

LOCAL_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
PHONE = "off1q8g49e9g4s5umugk0jnwhhl6m6z8ghvp9gc2wu9p"
OTHER = "off1q83uk7kfcky8ve832a04zauh0q4lxzr8auylht52"
HELLO = bytes(range(96))
PHONE_CENTRAL = "437D0AC9-6337-D5F4-D74A-9F5FF0F3717C"
MAC_CENTRAL = "BAD89907-37BE-47DC-D8CA-3F4F23E62D97"


@pytest.fixture
def protocol():
    p = MagicMock()
    p.local_address = MagicMock(return_value=LOCAL_ADDRESS)
    p.identity_assertion = MagicMock(return_value=list(HELLO))
    return p


@pytest.fixture
def peripheral(protocol):
    return BlePeripheral(protocol, device_id="coordinator")


@pytest.fixture
def verifier(monkeypatch):
    """The core's one verifier, answering PHONE for every value."""
    verify = MagicMock(return_value=PHONE)
    monkeypatch.setattr(peripheral_module, "verify_identity_assertion", verify)
    return verify


def write(peripheral, central, uuid, value, offset=0):
    return peripheral._on_attributed_write(central, uuid.upper(), value, offset)


# ---------------------------------------------------------------------------
# The peripheral binds a link to its hello
# ---------------------------------------------------------------------------


class TestPeripheralHello:
    def test_a_bound_central_feeds_the_core_under_the_address_it_proved(
        self, peripheral, protocol, verifier
    ):
        """The bug: with two centrals connected the phone's key package went
        to the core as ``ble-peer`` and was refused as a transport identity
        mismatch. Bound by its hello, it goes under the phone's address."""
        peripheral._connected_centrals.update({PHONE_CENTRAL: 1.0, MAC_CENTRAL: 2.0})
        assert write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO) == 0
        write(peripheral, PHONE_CENTRAL, MESSAGE_CHAR_UUID, b"\x03\x00kp")

        protocol.ble_fragment_received.assert_called_once_with(
            sender_id=PHONE, fragment=list(b"\x03\x00kp")
        )
        verifier.assert_called_once_with(list(HELLO))

    def test_an_unbound_central_keeps_its_own_id(self, peripheral, protocol, verifier):
        write(peripheral, MAC_CENTRAL, MESSAGE_CHAR_UUID, b"\x03\x00x")
        assert protocol.ble_fragment_received.call_args.kwargs["sender_id"] == MAC_CENTRAL

    def test_a_hello_announces_the_peer_once(self, peripheral, protocol, verifier):
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        write(peripheral, "SECOND-LINK", HELLO_CHAR_UUID, HELLO)
        protocol.ble_peer_discovered.assert_called_once_with(peer_id=PHONE, rssi=-50)

    def test_a_peer_our_central_already_proved_is_not_announced_again(
        self, peripheral, protocol, verifier
    ):
        peripheral.peer_linked_elsewhere = lambda address: address == PHONE
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        protocol.ble_peer_discovered.assert_not_called()
        # Still bound: its writes must carry its address either way.
        write(peripheral, PHONE_CENTRAL, MESSAGE_CHAR_UUID, b"\x03\x00x")
        assert protocol.ble_fragment_received.call_args.kwargs["sender_id"] == PHONE

    @pytest.mark.parametrize("length", [HELLO_MIN_LEN - 1, HELLO_MAX_LEN + 1, 0])
    def test_a_value_out_of_bounds_is_refused_before_verifying(
        self, peripheral, verifier, length
    ):
        assert write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, bytes(length)) == 0x0D
        verifier.assert_not_called()
        assert PHONE_CENTRAL not in peripheral._hello_bindings

    def test_a_write_at_an_offset_is_refused_before_verifying(self, peripheral, verifier):
        assert write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO, offset=4) == 0x07
        verifier.assert_not_called()

    def test_a_hello_that_does_not_verify_is_ignored(
        self, peripheral, protocol, monkeypatch
    ):
        monkeypatch.setattr(
            peripheral_module,
            "verify_identity_assertion",
            MagicMock(side_effect=ValueError("bad signature")),
        )
        assert write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO) == 0
        assert PHONE_CENTRAL not in peripheral._hello_bindings
        protocol.ble_peer_discovered.assert_not_called()

    def test_a_hello_never_rebinds_a_link(self, peripheral, protocol, monkeypatch):
        verify = MagicMock(side_effect=[PHONE, OTHER])
        monkeypatch.setattr(peripheral_module, "verify_identity_assertion", verify)
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        assert peripheral._hello_bindings[PHONE_CENTRAL] == PHONE

    def test_an_unnamed_writer_binds_nothing(self, peripheral, verifier):
        """Without the attributing delegate (or on BlueZ before the tracker
        saw the writer) the writer is ``ble-peer``, and there is no link."""
        peripheral._connected_centrals.update({"A": 1.0, "B": 2.0})
        peripheral._on_write(HELLO_CHAR_UUID, bytearray(HELLO))
        assert peripheral._hello_bindings == {}
        verifier.assert_not_called()

    def test_a_message_sender_never_replaces_a_hello_binding(
        self, peripheral, verifier
    ):
        peripheral._connected_centrals[PHONE_CENTRAL] = 1.0
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        peripheral.resolve_sender_identity(OTHER)
        assert peripheral._central_to_user_id[PHONE_CENTRAL] == PHONE

    def test_writes_from_the_bless_callback_are_attributed_too(
        self, peripheral, protocol, verifier
    ):
        """BlueZ: bless's own callback, with the tracker naming the writer."""
        peripheral._bluez_centrals = MagicMock(current="AA:BB:CC:DD:EE:FF")
        peripheral._on_write(HELLO_CHAR_UUID, bytearray(HELLO))
        peripheral._on_write(MESSAGE_CHAR_UUID, bytearray(b"\x03\x00x"))
        assert protocol.ble_fragment_received.call_args.kwargs["sender_id"] == PHONE


class TestPeripheralCentralLeaving:
    @pytest.mark.asyncio
    async def test_a_bound_central_leaving_reports_its_peer_lost(
        self, peripheral, protocol, verifier
    ):
        await self._leave(peripheral, linked_elsewhere=False)
        lost = [c.kwargs["peer_id"] for c in protocol.ble_peer_lost.call_args_list]
        assert lost == [PHONE_CENTRAL, PHONE]
        assert peripheral._hello_bindings == {}

    @pytest.mark.asyncio
    async def test_a_peer_our_client_link_still_reaches_is_not_reported_lost(
        self, peripheral, protocol, verifier
    ):
        await self._leave(peripheral, linked_elsewhere=True)
        lost = [c.kwargs["peer_id"] for c in protocol.ble_peer_lost.call_args_list]
        assert lost == [PHONE_CENTRAL]

    @pytest.mark.asyncio
    async def test_a_hello_from_a_central_that_never_subscribes_is_dropped(
        self, peripheral, protocol, verifier, monkeypatch
    ):
        """The monitor tracks subscribers only, so a central that said hello
        and left without subscribing kept its binding, and the core a route
        to its address, for the life of the process."""
        monkeypatch.setattr(peripheral_module, "_UNSUBSCRIBED_HELLO_GRACE", 0.01)
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        await self._poll(peripheral, lambda: PHONE_CENTRAL not in peripheral._hello_bindings)
        assert PHONE_CENTRAL not in peripheral._hello_bindings
        assert PHONE_CENTRAL not in peripheral._central_to_user_id
        protocol.ble_peer_lost.assert_called_once_with(peer_id=PHONE)

    @pytest.mark.asyncio
    async def test_a_hello_that_subscribes_within_the_grace_is_kept(
        self, peripheral, protocol, verifier, monkeypatch
    ):
        monkeypatch.setattr(peripheral_module, "_UNSUBSCRIBED_HELLO_GRACE", 5.0)
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        # A hello lands before the subscription does: unsubscribed for the
        # first polls, subscribed after.
        polls = iter([set()] * 5)
        await self._poll(
            peripheral, lambda: False, current=lambda: next(polls, {PHONE_CENTRAL}), ticks=60
        )
        assert peripheral._hello_bindings == {PHONE_CENTRAL: PHONE}
        protocol.ble_peer_lost.assert_not_called()

    @pytest.mark.asyncio
    async def test_a_dropped_hello_another_link_still_reaches_is_not_lost(
        self, peripheral, protocol, verifier, monkeypatch
    ):
        monkeypatch.setattr(peripheral_module, "_UNSUBSCRIBED_HELLO_GRACE", 0.01)
        peripheral.peer_linked_elsewhere = lambda address: True
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        await self._poll(peripheral, lambda: PHONE_CENTRAL not in peripheral._hello_bindings)
        assert PHONE_CENTRAL not in peripheral._hello_bindings
        protocol.ble_peer_lost.assert_not_called()

    async def _poll(self, peripheral, done, current=lambda: set(), ticks=200):
        peripheral._server = MagicMock()
        peripheral._bluez_centrals = None

        async def centrals():
            return current()

        peripheral._current_centrals = centrals
        with patch.object(peripheral_module, "_PEER_MONITOR_INTERVAL", 0.001):
            task = asyncio.ensure_future(peripheral._peer_monitor_loop())
            for _ in range(ticks):
                await asyncio.sleep(0.002)
                if done():
                    break
            task.cancel()
            await asyncio.wait({task})

    async def _leave(self, peripheral, linked_elsewhere):
        peripheral.peer_linked_elsewhere = lambda address: linked_elsewhere
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        peripheral._server = MagicMock()
        peripheral._bluez_centrals = None
        seen = [{PHONE_CENTRAL}, set()]

        async def current():
            return seen.pop(0) if seen else set()

        peripheral._current_centrals = current
        with patch.object(peripheral_module, "_PEER_MONITOR_INTERVAL", 0.001):
            task = asyncio.ensure_future(peripheral._peer_monitor_loop())
            for _ in range(200):
                await asyncio.sleep(0.002)
                if not seen and PHONE_CENTRAL not in peripheral._hello_bindings:
                    break
            task.cancel()
            await asyncio.wait({task})


class TestPeripheralGatt:
    @pytest.mark.asyncio
    async def test_the_service_offers_a_writable_hello(self, peripheral, monkeypatch):
        monkeypatch.setattr(
            peripheral_module,
            "GATTCharacteristicProperties",
            MagicMock(read=1, write=2, write_without_response=4, notify=8),
        )
        monkeypatch.setattr(
            peripheral_module,
            "GATTAttributePermissions",
            MagicMock(readable=1, writeable=2),
        )
        peripheral._server = MagicMock()
        peripheral._server.add_new_service = AsyncMock()
        peripheral._server.add_new_characteristic = AsyncMock()
        peripheral._identity_bytes = HELLO
        await peripheral._setup_gatt_server()
        added = {
            c.args[1]: (c.args[2], c.args[4])
            for c in peripheral._server.add_new_characteristic.call_args_list
        }
        assert added[HELLO_CHAR_UUID] == (2, 2)


# ---------------------------------------------------------------------------
# Notifications go to one central, and survive a full transmit queue
# ---------------------------------------------------------------------------


class TestPeripheralNotify:
    def test_a_fragment_for_a_bound_peer_targets_its_central(self, peripheral, verifier):
        central_obj = object()
        peripheral._subscribed_centrals = {PHONE_CENTRAL: central_obj, MAC_CENTRAL: object()}
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)
        assert peripheral._centrals_for(PHONE) == [central_obj]
        assert peripheral._centrals_for(OTHER) is None
        assert peripheral._centrals_for(None) is None

    def test_a_central_that_unsubscribes_mid_lookup_falls_back_to_all(
        self, peripheral, verifier
    ):
        """CoreBluetooth's queue removes an unsubscribing central between a
        membership test and an index; the lookup must not raise."""
        write(peripheral, PHONE_CENTRAL, HELLO_CHAR_UUID, HELLO)

        class Vanishing(dict):
            def __contains__(self, key):
                return True  # present at the test, gone at the index

        peripheral._subscribed_centrals = Vanishing()
        assert peripheral._centrals_for(PHONE) is None

    @pytest.mark.asyncio
    async def test_a_refused_notification_is_sent_once_the_stack_is_ready(self, peripheral):
        peripheral._notify_ready = asyncio.Event()
        answers = [False, True]
        sent = []

        def update(value, char, centrals):
            sent.append(value)
            ok = answers.pop(0)
            if not ok:
                asyncio.get_running_loop().call_later(0.01, peripheral._notify_ready.set)
            return ok

        server = MagicMock()
        server.peripheral_manager_delegate.peripheral_manager.updateValue_forCharacteristic_onSubscribedCentrals_ = update
        await peripheral._notify_corebluetooth(server, MagicMock(), b"frag", None)
        assert sent == [b"frag", b"frag"]

    @pytest.mark.asyncio
    async def test_a_notification_refused_every_time_gives_up(self, peripheral, monkeypatch):
        monkeypatch.setattr(peripheral_module, "_NOTIFY_READY_TIMEOUT", 0.001)
        peripheral._notify_ready = asyncio.Event()
        server = MagicMock()
        manager = server.peripheral_manager_delegate.peripheral_manager
        manager.updateValue_forCharacteristic_onSubscribedCentrals_ = MagicMock(return_value=False)
        await peripheral._notify_corebluetooth(server, MagicMock(), b"frag", None)
        assert manager.updateValue_forCharacteristic_onSubscribedCentrals_.call_count == (
            peripheral_module._NOTIFY_RETRIES + 1
        )


class TestPreparedHello:
    """CoreBluetooth hands a long write over as one batch at rising offsets.
    The chapter refuses a prepared hello before verifying anything, so the
    batch is judged whole, ahead of the per-request handler."""

    def test_a_hello_in_a_batch_is_refused(self):
        hello = HELLO_CHAR_UUID.upper()
        assert peripheral_module._prepared_hello_refusal([(hello, 0), (hello, 96)]) == 0x07

    def test_a_hello_batched_with_another_write_is_refused_at_offset_zero(self):
        """A reliable write puts several requests at offset 0 in one batch;
        a hello among them is still not one write."""
        assert peripheral_module._prepared_hello_refusal(
            [(HELLO_CHAR_UUID, 0), (MESSAGE_CHAR_UUID, 0)]
        ) == 0x07

    def test_a_hello_at_an_offset_is_refused(self):
        assert peripheral_module._prepared_hello_refusal([(HELLO_CHAR_UUID, 4)]) == 0x07

    def test_a_single_hello_at_offset_zero_passes_to_the_handler(self):
        assert peripheral_module._prepared_hello_refusal([(HELLO_CHAR_UUID, 0)]) is None

    def test_message_writes_are_not_judged(self):
        assert peripheral_module._prepared_hello_refusal(
            [(MESSAGE_CHAR_UUID, 0), (MESSAGE_CHAR_UUID, 0)]
        ) is None


# ---------------------------------------------------------------------------
# A Bluetooth stack that never comes up fails the start, it does not wedge
# ---------------------------------------------------------------------------


class TestPeripheralStart:
    @pytest.fixture(autouse=True)
    def on_macos(self, monkeypatch):
        """The off-loop construction is macOS's: BlueZ's never blocks."""
        monkeypatch.setattr(peripheral_module.sys, "platform", "darwin")

    @pytest.mark.asyncio
    async def test_a_constructor_that_never_returns_times_out(self, peripheral, monkeypatch):
        """bless's CoreBluetooth constructor waits, without a deadline, for a
        power-on that never comes when Bluetooth is not authorised. On the
        loop thread that froze the whole service; now it is a failed start."""
        monkeypatch.setattr(peripheral_module, "_SERVER_INIT_TIMEOUT", 0.05)
        release = threading.Event()
        monkeypatch.setattr(
            BlePeripheral, "_construct_server", staticmethod(lambda name, loop: release.wait(5))
        )
        try:
            with pytest.raises(TransportError, match="did not come up"):
                await peripheral._build_server("coordinator")
        finally:
            release.set()

    @pytest.mark.asyncio
    async def test_the_loop_keeps_running_while_the_stack_decides(self, peripheral, monkeypatch):
        release = threading.Event()
        monkeypatch.setattr(
            BlePeripheral, "_construct_server",
            staticmethod(lambda name, loop: (release.wait(5), ("server", loop))[1]),
        )
        build = asyncio.ensure_future(peripheral._build_server("coordinator"))
        await asyncio.sleep(0.01)
        assert not build.done()
        release.set()
        # bless is handed the running loop: in the init thread it would look
        # one up and find none.
        assert await build == ("server", asyncio.get_running_loop())

    @pytest.mark.asyncio
    async def test_bluez_builds_on_the_loop_thread(self, peripheral, monkeypatch):
        monkeypatch.setattr(peripheral_module.sys, "platform", "linux")
        seen = {}

        def construct(name, loop):
            seen["thread"] = threading.current_thread()
            return "server"

        monkeypatch.setattr(BlePeripheral, "_construct_server", staticmethod(construct))
        assert await peripheral._build_server("coordinator") == "server"
        assert seen["thread"] is threading.main_thread()

    @pytest.mark.asyncio
    async def test_a_denied_process_is_refused_before_touching_the_stack(
        self, peripheral, protocol, monkeypatch
    ):
        monkeypatch.setattr(peripheral, "is_available", lambda: True)
        monkeypatch.setattr(
            peripheral_module,
            "_bluetooth_authorization_refusal",
            lambda: "Bluetooth is denied to this process",
        )
        construct = MagicMock()
        monkeypatch.setattr(BlePeripheral, "_construct_server", staticmethod(construct))
        with pytest.raises(TransportError, match="denied"):
            await peripheral.start()
        construct.assert_not_called()


def test_the_attributing_delegate_is_swapped_in_for_one_construction_only():
    cb_server = pytest.importorskip("bless.backends.corebluetooth.server")
    original = cb_server.PeripheralManagerDelegate
    marker = MagicMock(name="delegate subclass")
    seen = {}

    def fake_server(name, loop=None):
        seen["delegate"] = cb_server.PeripheralManagerDelegate
        return "server"

    with patch.object(peripheral_module, "_attributing_delegate_class", return_value=marker), \
            patch.object(peripheral_module, "BlessServer", fake_server), \
            patch.object(peripheral_module.sys, "platform", "darwin"):
        assert BlePeripheral._construct_server("coordinator") == "server"
    assert seen["delegate"] is marker
    assert cb_server.PeripheralManagerDelegate is original


def test_a_construction_behind_a_wedged_one_says_so(monkeypatch):
    """A constructor that never returned keeps the delegate swapped and the
    lock held. A later start reports that, rather than waiting out the init
    deadline and blaming the adapter."""
    pytest.importorskip("bless.backends.corebluetooth.server")
    monkeypatch.setattr(peripheral_module, "_DELEGATE_SWAP_WAIT", 0.01)
    monkeypatch.setattr(peripheral_module.sys, "platform", "darwin")
    monkeypatch.setattr(peripheral_module, "_attributing_delegate_class", lambda: MagicMock())
    construct = MagicMock()
    monkeypatch.setattr(peripheral_module, "BlessServer", construct)
    assert peripheral_module._DELEGATE_SWAP_LOCK.acquire(timeout=1)
    try:
        with pytest.raises(TransportError, match="never returned"):
            BlePeripheral._construct_server("coordinator")
    finally:
        peripheral_module._DELEGATE_SWAP_LOCK.release()
    construct.assert_not_called()
    # Released on the way out of a normal construction too.
    BlePeripheral._construct_server("coordinator")
    assert peripheral_module._DELEGATE_SWAP_LOCK.acquire(timeout=0)
    peripheral_module._DELEGATE_SWAP_LOCK.release()


# ---------------------------------------------------------------------------
# The central says hello, keeps one link per peer, and drops dead links
# ---------------------------------------------------------------------------


def _client(reads, *, hello=True, mtu=247, connected=True):
    client = MagicMock()
    client.connect = AsyncMock()
    client.disconnect = AsyncMock()
    client.start_notify = AsyncMock()
    client.write_gatt_char = AsyncMock()
    client.is_connected = connected
    client.mtu_size = mtu
    client.services.get_characteristic = MagicMock(
        side_effect=lambda uuid: object() if (uuid == HELLO_CHAR_UUID and hello) else None
    )

    async def read_gatt_char(uuid):
        if uuid in reads:
            return bytearray(reads[uuid])
        raise RuntimeError(f"no such characteristic {uuid}")

    client.read_gatt_char = AsyncMock(side_effect=read_gatt_char)
    return client


@pytest.fixture
def manager(protocol, monkeypatch):
    monkeypatch.setattr(
        ble_manager_module, "verify_identity_assertion", MagicMock(return_value=PHONE)
    )
    return BleManager(protocol, device_id="desktop-1")


PEER_READS = {DEVICE_ID_CHAR_UUID: PHONE.encode(), IDENTITY_CHAR_UUID: HELLO}


async def _connect(manager, client, addr):
    device = MagicMock()
    device.address = addr
    manager._connecting.add(addr)
    with patch("offline_protocol_sdk.ble_manager.BleakClient", return_value=client):
        await manager._connect_to_peer(device)


class TestCentralHello:
    @pytest.mark.asyncio
    async def test_the_central_writes_its_assertion_with_response(self, manager):
        client = _client(PEER_READS)
        await _connect(manager, client, "AA:01")
        client.write_gatt_char.assert_awaited_once_with(HELLO_CHAR_UUID, HELLO, response=True)

    @pytest.mark.asyncio
    async def test_no_hello_characteristic_no_write(self, manager):
        client = _client(PEER_READS, hello=False)
        await _connect(manager, client, "AA:01")
        client.write_gatt_char.assert_not_awaited()
        client.disconnect.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_a_hello_that_does_not_fit_the_mtu_is_skipped(self, manager):
        client = _client(PEER_READS, mtu=23)
        await _connect(manager, client, "AA:01")
        client.write_gatt_char.assert_not_awaited()
        client.disconnect.assert_not_awaited()

    @pytest.mark.asyncio
    async def test_a_refused_hello_keeps_the_link(self, manager, protocol):
        client = _client(PEER_READS)
        client.write_gatt_char = AsyncMock(side_effect=RuntimeError("refused"))
        await _connect(manager, client, "AA:01")
        client.disconnect.assert_not_awaited()
        protocol.ble_peer_discovered.assert_called_once()


class TestCentralDuplicateLink:
    @pytest.mark.asyncio
    async def test_a_second_address_of_one_peer_is_closed_unannounced(self, manager, protocol):
        first, second = _client(PEER_READS), _client(PEER_READS)
        await _connect(manager, first, "AA:01")
        await _connect(manager, second, "AA:02")

        second.disconnect.assert_awaited_once()
        protocol.ble_peer_discovered.assert_called_once_with(peer_id=PHONE, rssi=-70)
        assert manager._device_id_to_addr[PHONE] == "AA:01"
        assert manager.holds_peer(PHONE)

    @pytest.mark.asyncio
    async def test_a_refused_address_is_not_redialed_while_the_kept_link_lives(
        self, manager, protocol
    ):
        """Without this the second address of a rotating peripheral was
        redialed, verified and refused again every cooldown."""
        first, second = _client(PEER_READS), _client(PEER_READS)
        await _connect(manager, first, "AA:01")
        await _connect(manager, second, "AA:02")
        # Past the per-address cooldown, inside the refusal: only the
        # refusal can say no here.
        inside = (
            ble_manager_module.time.monotonic()
            + ble_manager_module.ADAPTIVE_COOLDOWN_PER_PERIPHERAL
            + 1
        )
        assert inside < manager._duplicates["AA:02"][1]
        with manager._lock:
            manager._global_attempts.clear()
            assert not manager._should_connect_locked("AA:02", inside)
            # Lapses once the kept link is gone, inside the window.
            manager._clients.pop("AA:01")
            assert manager._should_connect_locked("AA:02", inside)
            assert "AA:02" not in manager._duplicates

    @pytest.mark.asyncio
    async def test_a_refusal_lapses_after_its_window(self, manager, protocol):
        first, second = _client(PEER_READS), _client(PEER_READS)
        await _connect(manager, first, "AA:01")
        await _connect(manager, second, "AA:02")
        later = (
            ble_manager_module.time.monotonic()
            + ble_manager_module.DUPLICATE_LINK_SUPPRESS
            + ble_manager_module.ADAPTIVE_COOLDOWN_PER_PERIPHERAL
            + 1
        )
        with manager._lock:
            manager._global_attempts.clear()
            assert manager._should_connect_locked("AA:02", later)

    @pytest.mark.asyncio
    async def test_a_peer_whose_kept_link_died_is_linked_again(self, manager, protocol):
        first = _client(PEER_READS, connected=False)
        await _connect(manager, first, "AA:01")
        second = _client(PEER_READS)
        await _connect(manager, second, "AA:02")
        second.disconnect.assert_not_awaited()
        assert manager._device_id_to_addr[PHONE] == "AA:02"


class TestCentralLiveness:
    @pytest.mark.asyncio
    async def test_a_link_whose_peer_is_gone_is_dropped_and_reported(self, manager, protocol):
        client = _client(PEER_READS)
        await _connect(manager, client, "AA:01")
        client.read_gatt_char = AsyncMock(side_effect=RuntimeError("attribute not found"))

        await manager._check_liveness()

        client.disconnect.assert_awaited()
        protocol.ble_peer_lost.assert_called_once_with(peer_id=PHONE)
        assert not manager.holds_peer(PHONE)

    @pytest.mark.asyncio
    async def test_a_link_that_now_names_another_peer_is_dropped(self, manager, protocol):
        client = _client(PEER_READS)
        await _connect(manager, client, "AA:01")
        client.read_gatt_char = AsyncMock(return_value=bytearray(OTHER.encode()))
        await manager._check_liveness()
        protocol.ble_peer_lost.assert_called_once_with(peer_id=PHONE)

    @pytest.mark.asyncio
    async def test_a_live_link_is_kept(self, manager, protocol):
        client = _client(PEER_READS)
        await _connect(manager, client, "AA:01")
        await manager._check_liveness()
        client.disconnect.assert_not_awaited()
        protocol.ble_peer_lost.assert_not_called()

    @pytest.mark.asyncio
    async def test_a_read_that_hangs_counts_as_dead(self, manager, protocol, monkeypatch):
        monkeypatch.setattr(ble_manager_module, "LIVENESS_READ_TIMEOUT", 0.01)
        client = _client(PEER_READS)
        await _connect(manager, client, "AA:01")

        async def hang(uuid):
            await asyncio.sleep(10)

        client.read_gatt_char = AsyncMock(side_effect=hang)
        await manager._check_liveness()
        protocol.ble_peer_lost.assert_called_once_with(peer_id=PHONE)
