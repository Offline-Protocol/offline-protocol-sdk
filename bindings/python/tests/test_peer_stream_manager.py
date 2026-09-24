"""Tests for PeerStreamManager, the TCP peer stream behind the ``wifi_direct`` slot.

The protocol is mocked; the sockets are real loopback sockets, because the
framing and the preamble position are what this manager owes and a fake
socket would test the fake. Every test that opens a stream plays the far
end by hand: reads the manager's preamble frame, writes its own, and then
either speaks or misbehaves.
"""

from __future__ import annotations

import asyncio
import json
import os
import socket
import time
from unittest.mock import AsyncMock, MagicMock

import pytest

from offline_protocol_sdk import peer_stream_manager as psm
from offline_protocol_sdk.peer_stream_manager import (
    LENGTH_PREFIX_LEN,
    MAX_FRAME_BYTES,
    PREAMBLE_FLOOR,
    SERVICE_TYPE,
    TXT_VERSION,
    PeerEntry,
    PeerStreamManager,
    frame,
    peers_from_record,
    service_instance_name,
    txt_record,
)
from offline_protocol_sdk.transport_manager import TransportError, TransportState

# Addresses and the (opaque to this layer) bytes that prove them. The fake
# verifier below maps one to the other; the real one is used once, on the
# chapter's vectors.
OUR_ADDRESS = "off1qyulwy7s5ezz20cy222zrw04rwds39uapqv9j8r0"
OUR_ASSERTION = bytes([0x11]) * 96
PEER_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
PEER_ASSERTION = bytes(range(96))
OTHER_ADDRESS = "off1qzk5zj9m3f5c8k2v8x6q4n7r0t2w5y8b3d6g9h1j4"
OTHER_ASSERTION = bytes([0x22]) * 96

VECTORS = os.path.join(
    os.path.dirname(__file__),
    "..",
    "..",
    "..",
    "crates",
    "offline-protocol-transport",
    "tests",
    "data",
    "stream-framing-v1.vectors.json",
)


# ---------------------------------------------------------------------------
# Fixtures and helpers
# ---------------------------------------------------------------------------


@pytest.fixture
def fake_verifier(monkeypatch):
    """The verifier as a lookup: known bytes derive to their address, anything
    else is refused the way the real one refuses."""
    table = {
        OUR_ASSERTION: OUR_ADDRESS,
        PEER_ASSERTION: PEER_ADDRESS,
        OTHER_ASSERTION: OTHER_ADDRESS,
    }

    def verify(assertion):
        try:
            return table[bytes(assertion)]
        except KeyError:
            raise RuntimeError("identity assertion does not verify") from None

    monkeypatch.setattr(psm, "verify_identity_assertion", verify)
    return table


@pytest.fixture
def fast_reconnect(monkeypatch):
    monkeypatch.setattr(psm, "RECONNECT_INITIAL_DELAY", 0.02)
    monkeypatch.setattr(psm, "RECONNECT_MAX_DELAY", 0.1)


def make_protocol(assertion: bytes = OUR_ASSERTION, address: str = OUR_ADDRESS) -> MagicMock:
    p = MagicMock()
    p.identity_assertion = MagicMock(return_value=list(assertion))
    p.local_address = MagicMock(return_value=address)
    p.wifi_direct_status_changed = MagicMock()
    p.wifi_direct_peer_connected = MagicMock()
    p.wifi_direct_peer_disconnected = MagicMock()
    p.wifi_direct_message_received = MagicMock()
    p.wifi_direct_get_next_message = MagicMock(return_value=None)
    return p


@pytest.fixture
def mock_protocol():
    return make_protocol()


@pytest.fixture
async def running(mock_protocol, fake_verifier):
    """A started manager on an ephemeral loopback port, stopped afterwards."""
    manager = PeerStreamManager(mock_protocol, listen_host="127.0.0.1", listen_port=0)
    await manager.start()
    try:
        yield manager
    finally:
        await manager.stop()


async def connect(manager: PeerStreamManager):
    return await asyncio.open_connection("127.0.0.1", manager.listen_port)


async def read_frame(reader: asyncio.StreamReader) -> bytes:
    prefix = await asyncio.wait_for(reader.readexactly(LENGTH_PREFIX_LEN), 2.0)
    return await asyncio.wait_for(reader.readexactly(int.from_bytes(prefix, "big")), 2.0)


async def closed_by_peer(reader: asyncio.StreamReader) -> bool:
    """True when the far end closed the stream (EOF) within the deadline."""
    try:
        return await asyncio.wait_for(reader.read(), 2.0) == b""
    except (asyncio.IncompleteReadError, ConnectionError):
        return True
    except asyncio.TimeoutError:
        return False


async def until(predicate, timeout: float = 2.0) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() > deadline:
            raise AssertionError("condition not met in time")
        await asyncio.sleep(0.01)


async def handshake(manager: PeerStreamManager, assertion: bytes = PEER_ASSERTION):
    """Open a stream as the peer and complete the preamble exchange."""
    reader, writer = await connect(manager)
    assert await read_frame(reader) == OUR_ASSERTION, "ours goes first, without waiting"
    writer.write(frame(assertion))
    await writer.drain()
    return reader, writer


# ---------------------------------------------------------------------------
# The literals the chapter fixes
# ---------------------------------------------------------------------------


class TestChapterLiterals:
    def test_framing_bounds_are_the_chapter_literals(self):
        # As literals, not read from the core: a test that read them would
        # agree with any edit.
        assert LENGTH_PREFIX_LEN == 4
        assert MAX_FRAME_BYTES == 1_048_576
        assert PREAMBLE_FLOOR == 96

    def test_service_type_and_txt_record(self):
        assert SERVICE_TYPE == "_offlineprotocol._tcp.local."
        assert len("_offlineprotocol") == 16  # underscore plus fifteen characters
        record = txt_record(PEER_ADDRESS)
        assert list(record.items())[0] == ("txtvers", TXT_VERSION)
        assert record["addr"] == PEER_ADDRESS
        name = service_instance_name(PEER_ADDRESS)
        assert name.endswith("." + SERVICE_TYPE)
        assert PEER_ADDRESS not in name, "the address lives in the TXT entry, once"

    def test_frame_prefixes_big_endian_and_refuses_the_receivers_refusals(self):
        assert frame(b"abc") == b"\x00\x00\x00\x03abc"
        assert frame(bytes(MAX_FRAME_BYTES))[:4] == b"\x00\x10\x00\x00"
        with pytest.raises(ValueError):
            frame(b"")
        with pytest.raises(ValueError):
            frame(bytes(MAX_FRAME_BYTES + 1))


class TestPeerEntry:
    def test_parse_host_port_and_claim(self):
        assert PeerEntry.parse("10.0.0.7:7878") == PeerEntry("10.0.0.7", 7878)
        assert PeerEntry.parse(f"{PEER_ADDRESS}@mesh.local:7878") == PeerEntry(
            "mesh.local", 7878, PEER_ADDRESS
        )
        assert PeerEntry.parse("[::1]:7878") == PeerEntry("::1", 7878)
        assert PeerEntry.parse(f" {PEER_ADDRESS}@[fe80::1]:1 ") == PeerEntry(
            "fe80::1", 1, PEER_ADDRESS
        )

    @pytest.mark.parametrize(
        "bad", ["", "host", ":7878", "host:", "host:abc", "host:0", "host:65536", "@host:1", "[::1]7878"]
    )
    def test_parse_refuses(self, bad):
        with pytest.raises(ValueError):
            PeerEntry.parse(bad)


class TestDiscoveryRecords:
    def _info(self, **kw):
        info = MagicMock()
        info.properties = kw.pop("properties", {b"txtvers": b"1", b"addr": PEER_ADDRESS.encode()})
        info.port = kw.pop("port", 7878)
        info.parsed_addresses = MagicMock(return_value=kw.pop("addresses", ["192.168.1.9"]))
        info.server = kw.pop("server", "host.local.")
        return info

    def test_record_yields_a_claimed_entry_per_address(self):
        entries = peers_from_record(self._info(addresses=["192.168.1.9", "fe80::1"]))
        assert entries == [
            PeerEntry("192.168.1.9", 7878, PEER_ADDRESS),
            PeerEntry("fe80::1", 7878, PEER_ADDRESS),
        ]

    def test_record_without_addr_yields_nothing(self):
        assert peers_from_record(self._info(properties={b"txtvers": b"1"})) == []
        assert peers_from_record(self._info(properties={b"addr": b"not-an-address"})) == []
        assert peers_from_record(self._info(port=0)) == []

    def test_record_falls_back_to_the_server_name(self):
        assert peers_from_record(self._info(addresses=[])) == [
            PeerEntry("host.local", 7878, PEER_ADDRESS)
        ]

    def test_our_own_record_is_ignored(self, mock_protocol, fake_verifier):
        manager = PeerStreamManager(mock_protocol)
        manager._ensure_connector = MagicMock()
        manager._on_record_resolved("ours", [PeerEntry("127.0.0.1", 1, OUR_ADDRESS)])
        manager._ensure_connector.assert_not_called()
        manager._on_record_resolved("theirs", [PeerEntry("127.0.0.1", 1, PEER_ADDRESS)])
        manager._ensure_connector.assert_called_once()


# ---------------------------------------------------------------------------
# Lifecycle
# ---------------------------------------------------------------------------


class TestLifecycle:
    async def test_start_binds_a_port_and_reports_the_layer_up(self, mock_protocol, fake_verifier):
        manager = PeerStreamManager(mock_protocol, listen_host="127.0.0.1", listen_port=0)
        assert manager.state == TransportState.STOPPED
        await manager.start()
        try:
            assert manager.state == TransportState.RUNNING
            assert manager.listen_port and manager.listen_port > 0
            mock_protocol.wifi_direct_status_changed.assert_called_once_with(is_connected=True)
            assert manager.get_metrics()["listen_port"] == manager.listen_port
            with pytest.raises(TransportError):
                await manager.start()
        finally:
            await manager.stop()
        assert manager.state == TransportState.STOPPED
        assert manager.listen_port is None
        mock_protocol.wifi_direct_status_changed.assert_called_with(is_connected=False)

    async def test_start_refuses_without_an_identity_to_prove(self, mock_protocol, fake_verifier):
        mock_protocol.identity_assertion.side_effect = RuntimeError("MLS not initialized")
        manager = PeerStreamManager(mock_protocol, listen_host="127.0.0.1", listen_port=0)
        with pytest.raises(TransportError):
            await manager.start()
        assert manager.state == TransportState.STOPPED
        mock_protocol.wifi_direct_status_changed.assert_not_called()

    async def test_configure_is_refused_while_running(self, running):
        with pytest.raises(TransportError):
            running.configure(peers=["127.0.0.1:1"])

    async def test_stop_reports_announced_peers_lost_and_unannounced_ones_not(
        self, mock_protocol, fake_verifier
    ):
        manager = PeerStreamManager(mock_protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        announced_reader, announced_writer = await handshake(manager)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)
        silent_reader, silent_writer = await connect(manager)
        await read_frame(silent_reader)  # never answers
        await until(lambda: len(manager._streams) == 2)

        await manager.stop()

        mock_protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)
        assert await closed_by_peer(announced_reader)
        assert await closed_by_peer(silent_reader)
        for w in (announced_writer, silent_writer):
            w.close()


# ---------------------------------------------------------------------------
# The preamble
# ---------------------------------------------------------------------------


class TestPreamble:
    async def test_a_verified_preamble_announces_the_derived_address(self, running, mock_protocol):
        reader, writer = await handshake(running)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)
        mock_protocol.wifi_direct_peer_connected.assert_called_once_with(peer_id=PEER_ADDRESS)
        assert running.connected_peers() == [PEER_ADDRESS]
        writer.close()
        await until(lambda: mock_protocol.wifi_direct_peer_disconnected.called)
        mock_protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)
        assert running.connected_peers() == []

    async def test_a_preamble_that_does_not_verify_closes_the_stream_unannounced(
        self, running, mock_protocol
    ):
        reader, writer = await handshake(running, assertion=bytes([0xEE]) * 100)
        assert await closed_by_peer(reader)
        mock_protocol.wifi_direct_peer_connected.assert_not_called()
        mock_protocol.wifi_direct_peer_disconnected.assert_not_called()
        assert not running._refusals, (
            "an inbound endpoint is a fresh ephemeral port each time; remembering it only grows the map"
        )
        writer.close()

    async def test_a_preamble_under_the_floor_is_refused_on_the_prefix(self, running, mock_protocol):
        reader, writer = await connect(running)
        await read_frame(reader)
        writer.write((PREAMBLE_FLOOR - 1).to_bytes(4, "big"))  # and no body, ever
        await writer.drain()
        assert await closed_by_peer(reader)
        mock_protocol.wifi_direct_peer_connected.assert_not_called()
        writer.close()

    async def test_a_message_before_the_preamble_closes_the_stream(self, running, mock_protocol):
        reader, writer = await connect(running)
        await read_frame(reader)
        writer.write(frame(b'{"id":"looks like a message"}' + bytes(80)))
        await writer.drain()
        assert await closed_by_peer(reader)
        mock_protocol.wifi_direct_peer_connected.assert_not_called()
        mock_protocol.wifi_direct_message_received.assert_not_called()
        writer.close()

    async def test_a_silent_stream_is_closed_at_the_deadline(self, mock_protocol, fake_verifier):
        manager = PeerStreamManager(
            mock_protocol, listen_host="127.0.0.1", listen_port=0, preamble_timeout=0.1
        )
        await manager.start()
        try:
            reader, writer = await connect(manager)
            await read_frame(reader)
            assert await closed_by_peer(reader)
            mock_protocol.wifi_direct_peer_connected.assert_not_called()
            writer.close()
        finally:
            await manager.stop()

    async def test_a_stream_proving_our_own_address_is_refused(self, running, mock_protocol):
        reader, writer = await handshake(running, assertion=OUR_ASSERTION)
        assert await closed_by_peer(reader)
        mock_protocol.wifi_direct_peer_connected.assert_not_called()
        writer.close()


# ---------------------------------------------------------------------------
# Message frames
# ---------------------------------------------------------------------------


class TestMessageFrames:
    async def test_a_body_is_handed_to_the_core_attributed_to_the_proved_address(
        self, running, mock_protocol
    ):
        reader, writer = await handshake(running)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)
        body = b"\xf5" + bytes(range(148))
        writer.write(frame(body) + frame(b"x"))
        await writer.drain()
        await until(lambda: mock_protocol.wifi_direct_message_received.call_count == 2)
        first, second = mock_protocol.wifi_direct_message_received.call_args_list
        assert first.kwargs == {"sender_id": PEER_ADDRESS, "data": list(body)}
        assert second.kwargs == {"sender_id": PEER_ADDRESS, "data": [ord("x")]}, (
            "the floor applies to the preamble only"
        )
        assert running.get_metrics()["frames_received"] == 2
        writer.close()

    async def test_a_body_of_exactly_the_ceiling_is_delivered(self, running, mock_protocol):
        reader, writer = await handshake(running)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)
        body = bytes([0xA5]) * MAX_FRAME_BYTES
        writer.write(frame(body))
        await writer.drain()
        await until(lambda: mock_protocol.wifi_direct_message_received.called, timeout=5.0)
        delivered = mock_protocol.wifi_direct_message_received.call_args.kwargs["data"]
        assert len(delivered) == MAX_FRAME_BYTES
        writer.close()

    @pytest.mark.parametrize(
        "prefix", [b"\x00\x00\x00\x00", (MAX_FRAME_BYTES + 1).to_bytes(4, "big")], ids=["zero", "over"]
    )
    async def test_a_zero_or_oversize_prefix_closes_before_the_body(self, running, mock_protocol, prefix):
        reader, writer = await handshake(running)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)
        writer.write(prefix)
        await writer.drain()
        assert await closed_by_peer(reader)
        mock_protocol.wifi_direct_message_received.assert_not_called()
        # Announced, so its close is reported, once.
        await until(lambda: mock_protocol.wifi_direct_peer_disconnected.called)
        mock_protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)
        writer.close()


# ---------------------------------------------------------------------------
# Outbound
# ---------------------------------------------------------------------------


class TestOutbound:
    async def test_queued_bodies_are_framed_onto_the_stream_that_proved_the_recipient(
        self, running, mock_protocol
    ):
        reader, writer = await handshake(running)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)

        first = MagicMock(recipient_id=PEER_ADDRESS, data=list(b"hello"))
        second = MagicMock(recipient_id=PEER_ADDRESS, data=list(b"world"))
        mock_protocol.wifi_direct_get_next_message.side_effect = [first, second, None]
        # From a foreign thread, as the Rust callback arrives.
        await asyncio.to_thread(running.on_messages_available)

        assert await read_frame(reader) == b"hello"
        assert await read_frame(reader) == b"world"
        assert running.get_metrics()["frames_sent"] == 2
        writer.close()

    async def test_a_body_for_an_unproved_recipient_is_not_written_anywhere(self, running, mock_protocol):
        reader, writer = await handshake(running)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)
        stray = MagicMock(recipient_id=OTHER_ADDRESS, data=list(b"lost"))
        mock_protocol.wifi_direct_get_next_message.side_effect = [stray, None]
        diagnostics = []
        running.set_delegate(on_diagnostic=lambda level, msg, ctx: diagnostics.append((msg, ctx)))
        running.on_messages_available()
        await until(lambda: any("No announced stream" in m for m, _ in diagnostics))
        with pytest.raises(asyncio.TimeoutError):
            await asyncio.wait_for(reader.readexactly(1), 0.2)
        writer.close()

    async def test_an_announcement_drains_what_was_already_queued(self, running, mock_protocol):
        queued = MagicMock(recipient_id=PEER_ADDRESS, data=list(b"waiting"))
        mock_protocol.wifi_direct_get_next_message.side_effect = [queued, None]
        reader, writer = await handshake(running)
        assert await read_frame(reader) == b"waiting"
        writer.close()


# ---------------------------------------------------------------------------
# One announced stream per address
# ---------------------------------------------------------------------------


class TestOneStreamPerAddress:
    async def test_a_newer_stream_of_the_winning_kind_supersedes_silently(
        self, running, mock_protocol
    ):
        # PEER_ADDRESS sorts below OUR_ADDRESS, so a stream the peer opened
        # (inbound here) is the kind that wins, and a second one of that
        # kind is the peer reconnecting: it takes the announcement over, the
        # older stream closes without a report, and the core sees one
        # announcement and, when the kept stream ends, one loss.
        assert PEER_ADDRESS < OUR_ADDRESS
        first_reader, first_writer = await handshake(running)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)
        second_reader, second_writer = await handshake(running)
        assert await closed_by_peer(first_reader), "the older stream is closed"
        assert running.connected_peers() == [PEER_ADDRESS]
        mock_protocol.wifi_direct_peer_connected.assert_called_once()
        mock_protocol.wifi_direct_peer_disconnected.assert_not_called()

        # The kept stream carries traffic.
        second_writer.write(frame(b"after reconnect"))
        await second_writer.drain()
        await until(lambda: mock_protocol.wifi_direct_message_received.called)

        second_writer.close()
        await until(lambda: mock_protocol.wifi_direct_peer_disconnected.called)
        mock_protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)
        first_writer.close()

    async def test_a_stream_of_the_losing_kind_is_refused_while_the_winner_lives(
        self, running, mock_protocol
    ):
        # We are the higher address, so a stream we open toward the peer is
        # the losing kind: with the peer's own stream announced, ours is
        # closed without a report and the announcement stays where it was.
        # (A connector never opens toward an announced peer; the stream is
        # opened by hand here, the way two concurrent opens would race.)
        assert OUR_ADDRESS > PEER_ADDRESS
        in_reader, in_writer = await handshake(running)
        await until(lambda: mock_protocol.wifi_direct_peer_connected.called)

        accepted: list = []

        async def serve(reader, writer):
            accepted.append(reader)
            assert await read_frame(reader) == OUR_ASSERTION
            writer.write(frame(PEER_ASSERTION))
            await writer.drain()

        server = await asyncio.start_server(serve, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        try:
            out_reader, out_writer = await asyncio.open_connection("127.0.0.1", port)
            out = psm._Stream(out_reader, out_writer, "peer", expected=PEER_ADDRESS, outbound=True)
            task = asyncio.ensure_future(running._run_stream(out))
            await until(lambda: task.done())
            assert await closed_by_peer(accepted[0]), "our outbound stream is the one closed"
            assert running._by_peer[PEER_ADDRESS].outbound is False
            assert out.learned == PEER_ADDRESS, "a refused stream still says whom it reached"
            mock_protocol.wifi_direct_peer_connected.assert_called_once()
            mock_protocol.wifi_direct_peer_disconnected.assert_not_called()
        finally:
            server.close()
            in_writer.close()

    async def test_the_stream_the_lower_address_opened_supersedes_silently(self, fake_verifier):
        # Here we are the lower address, so the winning stream is one we
        # opened. An inbound stream (opened by the peer) is announced first;
        # when our outbound stream to the same peer proves it, ours takes
        # the announcement over and the inbound one closes without a report.
        protocol = make_protocol(assertion=PEER_ASSERTION, address=PEER_ADDRESS)
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        peer_server = None
        try:
            # The peer, at OUR_ADDRESS, reaches us first.
            in_reader, in_writer = await asyncio.open_connection("127.0.0.1", manager.listen_port)
            assert await read_frame(in_reader) == PEER_ASSERTION
            in_writer.write(frame(OUR_ASSERTION))
            await in_writer.drain()
            await until(lambda: protocol.wifi_direct_peer_connected.called)

            # Now our own stream toward the peer, who answers with the same
            # identity, proves the same address.
            accepted: list = []

            async def serve(reader, writer):
                accepted.append(writer)
                assert await read_frame(reader) == PEER_ASSERTION
                writer.write(frame(OUR_ASSERTION))
                await writer.drain()

            peer_server = await asyncio.start_server(serve, "127.0.0.1", 0)
            port = peer_server.sockets[0].getsockname()[1]
            out_reader, out_writer = await asyncio.open_connection("127.0.0.1", port)
            out = psm._Stream(out_reader, out_writer, "peer", expected=OUR_ADDRESS, outbound=True)
            asyncio.ensure_future(manager._run_stream(out))

            assert await closed_by_peer(in_reader), "the superseded inbound stream is closed"
            await until(lambda: manager._by_peer.get(OUR_ADDRESS) is out)
            protocol.wifi_direct_peer_connected.assert_called_once()
            protocol.wifi_direct_peer_disconnected.assert_not_called()

            # The loss is reported once, when the kept stream ends.
            for w in accepted:
                w.close()
            await until(lambda: protocol.wifi_direct_peer_disconnected.called)
            protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=OUR_ADDRESS)
            in_writer.close()
        finally:
            await manager.stop()
            if peer_server is not None:
                peer_server.close()

    async def test_two_hosts_that_list_each_other_keep_one_stream(self, fake_verifier, fast_reconnect):
        a_protocol = make_protocol(assertion=OUR_ASSERTION, address=OUR_ADDRESS)
        b_protocol = make_protocol(assertion=PEER_ASSERTION, address=PEER_ADDRESS)
        a = PeerStreamManager(a_protocol, listen_host="127.0.0.1", listen_port=0)
        b = PeerStreamManager(b_protocol, listen_host="127.0.0.1", listen_port=0)
        await asyncio.gather(a.start(), b.start())
        try:
            to_b = PeerEntry("127.0.0.1", b.listen_port)
            to_a = PeerEntry("127.0.0.1", a.listen_port)
            a._ensure_connector(to_b.endpoint, [to_b])
            b._ensure_connector(to_a.endpoint, [to_a])
            await until(lambda: a.connected_peers() == [PEER_ADDRESS] and b.connected_peers() == [OUR_ADDRESS])
            # Let any losing stream be refused and any connector settle.
            await asyncio.sleep(0.3)
            assert a.connected_peers() == [PEER_ADDRESS]
            assert b.connected_peers() == [OUR_ADDRESS]
            a_protocol.wifi_direct_peer_connected.assert_called_once_with(peer_id=PEER_ADDRESS)
            b_protocol.wifi_direct_peer_connected.assert_called_once_with(peer_id=OUR_ADDRESS)
            a_protocol.wifi_direct_peer_disconnected.assert_not_called()
            b_protocol.wifi_direct_peer_disconnected.assert_not_called()
            # And both kept the same stream: the one the lower address opened.
            assert PEER_ADDRESS < OUR_ADDRESS
            assert b._by_peer[OUR_ADDRESS].outbound is True
            assert a._by_peer[PEER_ADDRESS].outbound is False
        finally:
            await asyncio.gather(a.stop(), b.stop())


# ---------------------------------------------------------------------------
# Static peers
# ---------------------------------------------------------------------------


class TestStaticPeers:
    async def test_a_configured_peer_is_connected_and_reconnected(
        self, mock_protocol, fake_verifier, fast_reconnect
    ):
        sessions: list = []

        async def serve(reader, writer):
            sessions.append(writer)
            assert await read_frame(reader) == OUR_ASSERTION
            writer.write(frame(PEER_ASSERTION))
            await writer.drain()

        server = await asyncio.start_server(serve, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        manager = PeerStreamManager(
            mock_protocol, listen_port=None, peers=[f"{PEER_ADDRESS}@127.0.0.1:{port}"]
        )
        await manager.start()
        try:
            assert manager.listen_port is None, "connect-only"
            await until(lambda: mock_protocol.wifi_direct_peer_connected.called)
            mock_protocol.wifi_direct_peer_connected.assert_called_once_with(peer_id=PEER_ADDRESS)

            sessions[0].close()
            await until(lambda: mock_protocol.wifi_direct_peer_disconnected.called)
            await until(lambda: mock_protocol.wifi_direct_peer_connected.call_count == 2)
            assert manager.connected_peers() == [PEER_ADDRESS]
        finally:
            await manager.stop()
            server.close()

    async def test_a_claimed_address_must_match_the_derived_one_exactly(
        self, mock_protocol, fake_verifier, fast_reconnect
    ):
        async def serve(reader, writer):
            await read_frame(reader)
            writer.write(frame(PEER_ASSERTION))
            await writer.drain()

        server = await asyncio.start_server(serve, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        manager = PeerStreamManager(
            mock_protocol, listen_port=None, peers=[f"{OTHER_ADDRESS}@127.0.0.1:{port}"]
        )
        diagnostics = []
        manager.set_delegate(on_diagnostic=lambda level, msg, ctx: diagnostics.append(msg))
        await manager.start()
        try:
            await until(lambda: "Peer preamble names another address" in diagnostics)
            mock_protocol.wifi_direct_peer_connected.assert_not_called()
            assert manager.connected_peers() == []
        finally:
            await manager.stop()
            server.close()


# ---------------------------------------------------------------------------
# The chapter's vectors, through the real verifier
# ---------------------------------------------------------------------------


class TestChapterVectors:
    @pytest.fixture
    def vectors(self):
        if not os.path.exists(VECTORS):
            pytest.skip("spec vectors are not in this tree")
        with open(VECTORS, encoding="utf-8") as f:
            return json.load(f)

    async def test_the_exchange_announces_and_delivers_as_the_vectors_say(self, vectors):
        # The real verifier, on RFC 8032's own signature: this is the one
        # test here whose preamble is a genuine assertion.
        protocol = make_protocol(address=OUR_ADDRESS)
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            reader, writer = await connect(manager)
            await read_frame(reader)
            for hex_frame in vectors["exchange"]["frames"]:
                writer.write(bytes.fromhex(hex_frame))
            await writer.drain()
            await until(lambda: protocol.wifi_direct_message_received.called)
            protocol.wifi_direct_peer_connected.assert_called_once_with(
                peer_id=vectors["exchange"]["announces"]
            )
            delivered = vectors["exchange"]["delivers"][0]
            protocol.wifi_direct_message_received.assert_called_once_with(
                sender_id=delivered["from"], data=list(bytes.fromhex(delivered["body_hex"]))
            )
            writer.close()
        finally:
            await manager.stop()

    async def test_every_refusal_vector_closes_the_stream_unannounced(self, vectors):
        protocol = make_protocol(address=OUR_ADDRESS)
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            for case in vectors["refusals"]:
                reader, writer = await connect(manager)
                await read_frame(reader)
                for hex_frame in case["frames"]:
                    writer.write(bytes.fromhex(hex_frame))
                await writer.drain()
                assert await closed_by_peer(reader), case["name"]
                writer.close()
            protocol.wifi_direct_peer_connected.assert_not_called()
            protocol.wifi_direct_peer_disconnected.assert_not_called()
            protocol.wifi_direct_message_received.assert_not_called()
        finally:
            await manager.stop()

    async def test_a_prefix_of_exactly_the_ceiling_is_accepted(self, vectors):
        accepted = vectors["accepted_prefixes"][0]
        assert accepted["length"] == MAX_FRAME_BYTES
        protocol = make_protocol(address=OUR_ADDRESS)
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            reader, writer = await connect(manager)
            await read_frame(reader)
            writer.write(bytes.fromhex(vectors["preamble"]["frame_hex"]))
            writer.write(bytes.fromhex(accepted["prefix_hex"]))
            await writer.drain()
            await until(lambda: protocol.wifi_direct_peer_connected.called)
            with pytest.raises(asyncio.TimeoutError):
                await asyncio.wait_for(reader.read(1), 0.2)  # still open, awaiting the body
            writer.write(bytes(accepted["length"]))
            await writer.drain()
            await until(lambda: protocol.wifi_direct_message_received.called, timeout=5.0)
            writer.close()
        finally:
            await manager.stop()


# ---------------------------------------------------------------------------
# Stream lifetime: deadlines, bounds, liveness, the ladder
# ---------------------------------------------------------------------------


def _fake_stream(outbound: bool = False) -> "psm._Stream":
    """A stream over an in-memory reader the test feeds, and a writer that
    only records, so the read loop can be driven byte by byte."""
    writer = MagicMock()
    writer.drain = AsyncMock()
    return psm._Stream(asyncio.StreamReader(), writer, "fake", expected=None, outbound=outbound)


class TestStreamLifetime:
    async def test_the_preamble_deadline_covers_the_body_too(self, mock_protocol, fake_verifier):
        # A preamble prefix and then silence must not hold the slot: the
        # deadline is on the whole first frame, not on its four-byte prefix.
        manager = PeerStreamManager(
            mock_protocol, listen_host="127.0.0.1", listen_port=0, preamble_timeout=0.1
        )
        await manager.start()
        try:
            reader, writer = await connect(manager)
            await read_frame(reader)
            writer.write(PREAMBLE_FLOOR.to_bytes(4, "big"))  # and no body, ever
            await writer.drain()
            assert await closed_by_peer(reader)
            await until(lambda: not manager._streams)
            mock_protocol.wifi_direct_peer_connected.assert_not_called()
            writer.close()
        finally:
            await manager.stop()

    async def test_one_host_cannot_hold_more_than_its_share_of_streams(
        self, mock_protocol, fake_verifier
    ):
        manager = PeerStreamManager(
            mock_protocol, listen_host="127.0.0.1", listen_port=0, max_streams_per_host=2
        )
        await manager.start()
        try:
            held = []
            for _ in range(2):
                reader, writer = await connect(manager)
                await read_frame(reader)  # served: our preamble arrives
                held.append(writer)
            reader, writer = await connect(manager)
            assert await closed_by_peer(reader), "the third stream from one host is refused"
            assert manager.get_metrics()["streams_refused"] == 1
            for w in held + [writer]:
                w.close()
        finally:
            await manager.stop()

    async def test_every_stream_carries_tcp_keepalive(self, mock_protocol, fake_verifier):
        sessions: list = []  # held, or the writer's collection closes the socket

        async def serve(reader, writer):
            sessions.append(writer)
            await read_frame(reader)
            writer.write(frame(PEER_ASSERTION))
            await writer.drain()

        server = await asyncio.start_server(serve, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        manager = PeerStreamManager(
            mock_protocol, listen_host="127.0.0.1", listen_port=0, peers=[f"127.0.0.1:{port}"]
        )
        await manager.start()
        try:
            await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
            in_reader, in_writer = await handshake(manager, assertion=OTHER_ASSERTION)
            await until(lambda: len(manager.connected_peers()) == 2)
            kinds = set()
            for stream in manager._streams:
                sock = stream.writer.get_extra_info("socket")
                assert sock.getsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE) != 0
                kinds.add(stream.outbound)
            assert kinds == {True, False}, "both an outbound and an inbound stream were checked"
            in_writer.close()
        finally:
            await manager.stop()
            server.close()

    async def test_a_superseded_stream_hands_nothing_more_to_the_core(
        self, mock_protocol, fake_verifier
    ):
        # A body already in the superseded stream's buffer must not reach the
        # core: its frames belong to no announced peer any more.
        manager = PeerStreamManager(mock_protocol, listen_port=None)
        await manager.start()
        try:
            older = _fake_stream()
            older_task = asyncio.ensure_future(manager._run_stream(older))
            older.reader.feed_data(frame(PEER_ASSERTION))
            await until(lambda: manager._by_peer.get(PEER_ADDRESS) is older)

            newer = _fake_stream()
            newer_task = asyncio.ensure_future(manager._run_stream(newer))
            newer.reader.feed_data(frame(PEER_ASSERTION))
            await until(lambda: manager._by_peer.get(PEER_ADDRESS) is newer)

            older.reader.feed_data(frame(b"late"))
            older.reader.feed_eof()
            await asyncio.wait_for(older_task, 2.0)
            mock_protocol.wifi_direct_message_received.assert_not_called()
            mock_protocol.wifi_direct_peer_connected.assert_called_once()
            mock_protocol.wifi_direct_peer_disconnected.assert_not_called()
            assert manager.get_metrics()["streams_superseded"] == 1
            newer.reader.feed_eof()
            await asyncio.wait_for(newer_task, 2.0)
        finally:
            await manager.stop()

    async def test_an_endpoint_that_never_proves_a_peer_climbs_the_ladder(
        self, mock_protocol, fake_verifier, fast_reconnect
    ):
        # A wrong port in a peer list: some service that accepts and closes.
        # Every attempt ends unannounced, so the delay doubles to the ceiling
        # rather than retrying at the initial pace forever.
        connects: list = []

        async def serve(reader, writer):
            connects.append(time.monotonic())
            writer.close()

        server = await asyncio.start_server(serve, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        manager = PeerStreamManager(mock_protocol, listen_port=None, peers=[f"127.0.0.1:{port}"])
        await manager.start()
        try:
            await asyncio.sleep(0.6)
            # Ladder 0.02, 0.04, 0.08, 0.1, 0.1, ...: about seven attempts in
            # 0.6 s, where a fixed 0.02 s pace would make about thirty.
            assert 2 <= len(connects) <= 10, len(connects)
        finally:
            await manager.stop()
            server.close()

    async def test_a_connector_survives_an_unexpected_connect_error(
        self, mock_protocol, fake_verifier, fast_reconnect, monkeypatch
    ):
        sessions: list = []  # held, or the writer's collection closes the socket

        async def serve(reader, writer):
            sessions.append(writer)
            await read_frame(reader)
            writer.write(frame(PEER_ASSERTION))
            await writer.drain()

        server = await asyncio.start_server(serve, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        real_open = asyncio.open_connection
        calls = {"n": 0}

        async def flaky_open(host, port_):
            calls["n"] += 1
            if calls["n"] == 1:
                raise ValueError("not an OSError")
            return await real_open(host, port_)

        monkeypatch.setattr(psm.asyncio, "open_connection", flaky_open)
        manager = PeerStreamManager(mock_protocol, listen_port=None, peers=[f"127.0.0.1:{port}"])
        await manager.start()
        try:
            await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
            assert calls["n"] >= 2, "the connector retried after the ValueError"
        finally:
            await manager.stop()
            server.close()

    async def test_only_an_outbound_endpoint_is_remembered_as_refusing(
        self, mock_protocol, fake_verifier, fast_reconnect
    ):
        async def serve(reader, writer):
            await read_frame(reader)
            writer.write(frame(bytes([0xEE]) * 100))  # does not verify
            await writer.drain()

        server = await asyncio.start_server(serve, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        manager = PeerStreamManager(mock_protocol, listen_port=None, peers=[f"127.0.0.1:{port}"])
        await manager.start()
        try:
            await until(lambda: manager._refusals.get(f"127.0.0.1:{port}", 0) >= 1)
        finally:
            await manager.stop()
            server.close()

    async def test_stop_awaits_every_connector(self, mock_protocol, fake_verifier):
        manager = PeerStreamManager(mock_protocol, listen_port=None, peers=["127.0.0.1:9"])
        await manager.start()
        tasks = list(manager._connector_tasks.values())
        assert tasks
        await manager.stop()
        assert all(t.done() for t in tasks)
        assert manager._connector_tasks == {}


# ---------------------------------------------------------------------------
# DNS-SD: record lifecycle, and a live advertise-and-browse round trip
# ---------------------------------------------------------------------------


class TestDiscoveryLifecycle:
    def test_a_record_prefers_scoped_addresses(self):
        info = MagicMock()
        info.properties = {b"txtvers": b"1", b"addr": PEER_ADDRESS.encode()}
        info.port = 7878
        info.parsed_scoped_addresses = MagicMock(return_value=["fe80::1%en0", "192.168.1.9"])
        info.parsed_addresses = MagicMock(return_value=["fe80::1", "192.168.1.9"])
        assert [e.host for e in peers_from_record(info)] == ["fe80::1%en0", "192.168.1.9"]

    def test_one_listen_address_is_advertised_as_itself(self):
        assert psm.advertised_addresses("192.168.1.9") == ["192.168.1.9"]
        assert psm.advertised_addresses("::1") == ["::1"]

    def test_a_wildcard_listener_advertises_no_loopback(self):
        pytest.importorskip("ifaddr")
        for address in psm.advertised_addresses("0.0.0.0"):
            assert not address.startswith("127.") and address != "::1"

    def test_the_record_names_a_host_of_its_own(self):
        name = psm.service_host_name(PEER_ADDRESS)
        assert name.endswith(".local.") and PEER_ADDRESS not in name
        assert not name.endswith(SERVICE_TYPE), "the SRV target is a host, not the instance"

    async def test_a_removed_record_stops_reconnecting_but_keeps_the_live_stream(
        self, mock_protocol, fake_verifier, fast_reconnect
    ):
        sessions: list = []

        async def serve(reader, writer):
            sessions.append(writer)
            await read_frame(reader)
            writer.write(frame(PEER_ASSERTION))
            await writer.drain()

        server = await asyncio.start_server(serve, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        manager = PeerStreamManager(mock_protocol, listen_port=None)
        await manager.start()
        try:
            manager._on_record_resolved("peer", [PeerEntry("127.0.0.1", port, PEER_ADDRESS)])
            await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
            manager._on_record_removed("peer")
            await asyncio.sleep(0.1)
            assert manager.connected_peers() == [PEER_ADDRESS], "a record expiring cuts nothing"

            sessions[0].close()
            await until(lambda: not manager._connector_tasks)
            await asyncio.sleep(0.2)
            assert len(sessions) == 1, "no reconnect toward a removed record"
            assert manager._connector_entries == {}
        finally:
            await manager.stop()
            server.close()

    async def test_a_host_advertised_over_dns_sd_is_found_and_proved(self, fake_verifier):
        pytest.importorskip("zeroconf")
        a_protocol = make_protocol(assertion=OUR_ASSERTION, address=OUR_ADDRESS)
        b_protocol = make_protocol(assertion=PEER_ASSERTION, address=PEER_ADDRESS)
        a = PeerStreamManager(a_protocol, listen_host="127.0.0.1", listen_port=0, advertise=True)
        b = PeerStreamManager(b_protocol, listen_port=None, discover=True)
        await a.start()
        await b.start()
        try:
            await until(
                lambda: a.connected_peers() == [PEER_ADDRESS]
                and b.connected_peers() == [OUR_ADDRESS],
                timeout=15.0,
            )
            b_protocol.wifi_direct_peer_connected.assert_called_once_with(peer_id=OUR_ADDRESS)
        finally:
            await asyncio.gather(a.stop(), b.stop())


# ---------------------------------------------------------------------------
# Outbound isolation: one peer's receive window holds only that peer
# ---------------------------------------------------------------------------


def queued_protocol() -> tuple[MagicMock, list]:
    """A protocol whose outbound queue is a list the test fills."""
    protocol = make_protocol()
    queue: list = []
    protocol.wifi_direct_get_next_message = MagicMock(
        side_effect=lambda: queue.pop(0) if queue else None
    )
    return protocol, queue


def body_for(recipient: str, data: bytes):
    return MagicMock(recipient_id=recipient, data=data)


async def stalled_handshake(manager: PeerStreamManager, assertion: bytes):
    """A peer that proves itself and then never reads: small buffers on
    both ends, so a few hundred KiB fill them."""
    reader, writer = await asyncio.open_connection("127.0.0.1", manager.listen_port)
    writer.get_extra_info("socket").setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 8192)
    assert await read_frame(reader) == OUR_ASSERTION
    writer.write(frame(assertion))
    await writer.drain()
    return reader, writer


def shrink_send_buffer(manager: PeerStreamManager, peer: str) -> None:
    stream = manager._by_peer[peer]
    stream.writer.get_extra_info("socket").setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 8192)


async def until_wedged(manager: PeerStreamManager, peer: str, timeout: float = 5.0) -> None:
    """Wait until writes toward ``peer`` have stopped moving: bytes are
    buffered and the buffer has not shrunk for a while. A fixed sleep is not
    enough, because the kernel keeps accepting for a moment after the peer
    stops reading, and a stream that is still flushing closes cleanly."""
    transport = manager._by_peer[peer].writer.transport
    deadline = time.monotonic() + timeout
    last = -1
    while time.monotonic() < deadline:
        size = transport.get_write_buffer_size()
        if size > 0 and size == last:
            return
        last = size
        await asyncio.sleep(0.25)
    raise AssertionError("the stream never wedged")


class TestOutboundIsolation:
    async def test_a_peer_that_stops_reading_does_not_hold_the_others(self, fake_verifier):
        protocol, queue = queued_protocol()
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            _, stalled_writer = await stalled_handshake(manager, PEER_ASSERTION)
            healthy_reader, healthy_writer = await handshake(manager, OTHER_ASSERTION)
            await until(lambda: len(manager.connected_peers()) == 2)
            shrink_send_buffer(manager, PEER_ADDRESS)

            for _ in range(8):
                queue.append(body_for(PEER_ADDRESS, b"x" * (256 * 1024)))
            queue.append(body_for(OTHER_ADDRESS, b"for the healthy peer"))
            manager.on_messages_available()

            assert await read_frame(healthy_reader) == b"for the healthy peer"
            stalled_writer.close()
            healthy_writer.close()
        finally:
            await manager.stop()

    async def test_a_write_past_the_deadline_aborts_the_stream_and_reports_it_lost(
        self, fake_verifier
    ):
        protocol, queue = queued_protocol()
        manager = PeerStreamManager(
            protocol, listen_host="127.0.0.1", listen_port=0, write_timeout=0.2
        )
        await manager.start()
        try:
            _, stalled_writer = await stalled_handshake(manager, PEER_ASSERTION)
            await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
            shrink_send_buffer(manager, PEER_ADDRESS)
            for _ in range(8):
                queue.append(body_for(PEER_ADDRESS, b"x" * (256 * 1024)))
            manager.on_messages_available()

            await until(lambda: protocol.wifi_direct_peer_disconnected.called, timeout=3.0)
            protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)
            # Aborted, not waiting on a flush that never comes.
            await until(lambda: not manager._streams)
            stalled_writer.close()
        finally:
            await manager.stop()

    async def test_stop_after_a_stalled_peer_is_prompt_and_a_restart_writes_again(
        self, fake_verifier
    ):
        protocol, queue = queued_protocol()
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        _, stalled_writer = await stalled_handshake(manager, PEER_ASSERTION)
        await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
        shrink_send_buffer(manager, PEER_ADDRESS)
        for _ in range(8):
            queue.append(body_for(PEER_ADDRESS, b"x" * (256 * 1024)))
        manager.on_messages_available()
        await until_wedged(manager, PEER_ADDRESS)

        began = time.monotonic()
        await manager.stop()
        assert time.monotonic() - began < 1.5, "stop() did not wait on the stalled flush"
        queue.clear()

        await manager.start()
        try:
            reader, writer = await handshake(manager, OTHER_ASSERTION)
            await until(lambda: manager.connected_peers() == [OTHER_ADDRESS])
            queue.append(body_for(OTHER_ADDRESS, b"after the restart"))
            manager.on_messages_available()
            assert await read_frame(reader) == b"after the restart"
            writer.close()
            stalled_writer.close()
        finally:
            await manager.stop()

    async def test_a_body_no_receiver_would_accept_is_dropped_and_the_stream_kept(
        self, fake_verifier
    ):
        protocol, queue = queued_protocol()
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            reader, writer = await handshake(manager)
            await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
            queue.append(body_for(PEER_ADDRESS, b"x" * (MAX_FRAME_BYTES + 1)))
            queue.append(body_for(PEER_ADDRESS, b"the next one"))
            manager.on_messages_available()
            assert await read_frame(reader) == b"the next one"
            assert manager.connected_peers() == [PEER_ADDRESS]
            assert manager.get_metrics()["frames_dropped"] == 1
            writer.close()
        finally:
            await manager.stop()

    async def test_a_stalled_peer_queue_is_bounded(self, fake_verifier):
        protocol, queue = queued_protocol()
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            _, stalled_writer = await stalled_handshake(manager, PEER_ASSERTION)
            await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
            shrink_send_buffer(manager, PEER_ADDRESS)
            # Wedged first, so the drops below are the stall's and not a
            # burst's: a burst arrives before the writer has run at all.
            queue.append(body_for(PEER_ADDRESS, b"x" * MAX_FRAME_BYTES))
            manager.on_messages_available()
            await until_wedged(manager, PEER_ADDRESS)
            stream = manager._by_peer[PEER_ADDRESS]
            assert stream.write_blocked

            for _ in range(12):
                queue.append(body_for(PEER_ADDRESS, b"x" * MAX_FRAME_BYTES))
            manager.on_messages_available()
            await until(lambda: not queue)
            assert stream.queued_bytes <= psm.MAX_QUEUED_BYTES
            accepted = stream.outbound_queue.qsize()
            assert accepted >= 1, "one frame always fits on an empty queue"
            assert manager.get_metrics()["frames_dropped"] == 12 - accepted
            stalled_writer.close()
        finally:
            await manager.stop()

    async def test_a_burst_to_a_reading_peer_is_delivered_whole(self, fake_verifier):
        protocol, queue = queued_protocol()
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            reader, writer = await handshake(manager)
            await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
            # Over the queue bound, handed over in one tick.
            count = psm.MAX_QUEUED_BYTES // MAX_FRAME_BYTES + 2
            for i in range(count):
                queue.append(body_for(PEER_ADDRESS, bytes([i]) * MAX_FRAME_BYTES))
            manager.on_messages_available()
            for i in range(count):
                prefix = await asyncio.wait_for(reader.readexactly(LENGTH_PREFIX_LEN), 5.0)
                body = await asyncio.wait_for(
                    reader.readexactly(int.from_bytes(prefix, "big")), 5.0
                )
                assert body == bytes([i]) * MAX_FRAME_BYTES
            assert manager.get_metrics()["frames_dropped"] == 0
            writer.close()
        finally:
            await manager.stop()

    async def test_a_slow_reader_that_keeps_reading_is_not_cut(self, fake_verifier):
        protocol, queue = queued_protocol()
        # One frame takes several deadlines to cross at the pace below; a
        # per-frame deadline would abort it after the first.
        manager = PeerStreamManager(
            protocol, listen_host="127.0.0.1", listen_port=0, write_timeout=0.4
        )
        await manager.start()
        try:
            reader, writer = await stalled_handshake(manager, PEER_ASSERTION)
            await until(lambda: manager.connected_peers() == [PEER_ADDRESS])
            shrink_send_buffer(manager, PEER_ADDRESS)
            queue.append(body_for(PEER_ADDRESS, b"z" * MAX_FRAME_BYTES))
            manager.on_messages_available()

            prefix = await asyncio.wait_for(reader.readexactly(LENGTH_PREFIX_LEN), 5.0)
            remaining = int.from_bytes(prefix, "big")
            began = time.monotonic()
            while remaining:
                chunk = await asyncio.wait_for(reader.read(min(16 * 1024, remaining)), 2.0)
                assert chunk, "the stream was cut while the peer was reading"
                remaining -= len(chunk)
                await asyncio.sleep(0.02)
            assert time.monotonic() - began > 2 * 0.4, "slow enough to span deadlines"
            protocol.wifi_direct_peer_disconnected.assert_not_called()
            assert manager.connected_peers() == [PEER_ADDRESS]
            writer.close()
        finally:
            await manager.stop()


class TestStopAndSelf:
    async def test_an_inbound_stream_accepted_while_stopping_is_refused(
        self, running, mock_protocol
    ):
        running._update_state(TransportState.STOPPING)
        try:
            reader, writer = await connect(running)
            assert await closed_by_peer(reader), "refused, and no preamble sent"
            assert not running._streams
            writer.close()
        finally:
            running._update_state(TransportState.RUNNING)

    async def test_a_peer_entry_that_is_ourselves_backs_off(self, mock_protocol, fake_verifier):
        manager = PeerStreamManager(mock_protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            entry = PeerEntry("127.0.0.1", manager.listen_port)
            manager._ensure_connector(entry.endpoint, [entry])
            await until(lambda: manager._refusals.get(entry.endpoint, 0) >= 1)
            mock_protocol.wifi_direct_peer_connected.assert_not_called()
        finally:
            await manager.stop()
