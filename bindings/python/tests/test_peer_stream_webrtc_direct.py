"""Streams a page dialled to the Python host's WebRTC Direct listener.

The listener itself (admission, DTLS, the data channel, the preamble) is the
FFI's and is pinned in Rust. What this host owes is the bridge in front of
it (``docs/spec/stream-framing.md``, "What a receiver owes" and "Dialling a
listener from a browser"; ADR 0039): dialled streams join the TCP streams'
one table, so the core sees one announcement and one loss per address, the
newer of two dialled streams wins, a TCP stream keeps its address against a
dial and the other way round, a dialled stream counts as inbound from its
page's address, a stream proving this host's own address is refused, and the
code is never logged.

No page can dial from Python, so the bridge is driven by a fake listener
that yields scripted events and records every call made into it; the UDP
socket under it is real. One test opens the real library's listener on a
private address of this machine.
"""

from __future__ import annotations

import asyncio
import ipaddress
import logging
import os
import re
import socket
from unittest.mock import MagicMock

import pytest

from offline_protocol_sdk.offline_protocol import (
    OfflineProtocol,
    OverflowPolicy,
    ProtocolConfig,
    ProtocolError,
    WebrtcDirectCode,
    WebrtcDirectDatagram,
    WebrtcDirectEvent,
    WebrtcDirectEventKind,
)
from offline_protocol_sdk.peer_stream_manager import PeerStreamManager, frame
from offline_protocol_sdk.transport_manager import TransportError

from conftest import InMemoryStorage
from test_peer_stream_manager import (  # noqa: F401 (fake_verifier is a fixture)
    OTHER_ADDRESS,
    OUR_ADDRESS,
    PEER_ADDRESS,
    PEER_ASSERTION,
    body_for,
    closed_by_peer,
    fake_verifier,
    handshake,
    make_protocol,
    until,
)

THIRD_ADDRESS = "off1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgp"
FOURTH_ADDRESS = "off1qgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqsz"
# Distinctive values, so a leak into the log is found by search.
PASSWORD = "SecretPasswordNeverLoggedAnywhere"
FRAGMENT = (
    "op-direct=1&h=192.168.1.10&p=40000&c=" + "A" * 43
    + f"&u=UfragUfragUfrag12&w={PASSWORD}&a={OUR_ADDRESS}"
)
PAGE_IP = "192.168.1.20"


class FakeListener:
    """The FFI's ``webrtc_direct_*`` as a script. Events are queued by the
    test; calls into it are recorded. Like the real listener, a proved
    stream that is closed (by the host, by ``close``) yields one ``Closed``,
    and nothing else does."""

    def __init__(self) -> None:
        self.events: list[WebrtcDirectEvent] = []
        self.datagrams: list[WebrtcDirectDatagram] = []
        self.received: list[tuple[str, int, bytes]] = []
        self.sent: list[tuple[int, bytes]] = []
        self.closed_streams: list[int] = []
        self.proved: set[int] = set()
        self.ticks: list[int | None] = []
        self.tick_calls = 0
        self.opened: list[tuple[str, int]] = []
        self.close_calls = 0
        self.open_error: Exception | None = None
        self.send_error: Exception | None = None

    def install(self, protocol: MagicMock) -> None:
        protocol.webrtc_direct_open = MagicMock(side_effect=self.open)
        protocol.webrtc_direct_close = MagicMock(side_effect=self.close)
        protocol.webrtc_direct_datagram_received = MagicMock(side_effect=self.datagram_received)
        protocol.webrtc_direct_next_datagram = MagicMock(
            side_effect=lambda: self.datagrams.pop(0) if self.datagrams else None
        )
        protocol.webrtc_direct_next_event = MagicMock(
            side_effect=lambda: self.events.pop(0) if self.events else None
        )
        protocol.webrtc_direct_send = MagicMock(side_effect=self.send)
        protocol.webrtc_direct_close_stream = MagicMock(side_effect=self.close_stream)
        protocol.webrtc_direct_tick = MagicMock(side_effect=self.tick)

    # -- the FFI ---------------------------------------------------------------

    def open(self, local_ip: str, local_port: int) -> WebrtcDirectCode:
        if self.open_error is not None:
            raise self.open_error
        self.opened.append((local_ip, local_port))
        return WebrtcDirectCode(
            fragment=FRAGMENT, host=local_ip, port=local_port, address=OUR_ADDRESS
        )

    def close(self) -> None:
        self.close_calls += 1
        for stream in sorted(self.proved):
            self.events.append(_event(WebrtcDirectEventKind.CLOSED, stream))
        self.proved.clear()

    def datagram_received(self, source_ip: str, source_port: int, data: list[int]) -> None:
        self.received.append((source_ip, source_port, bytes(data)))

    def send(self, stream: int, body: list[int]) -> None:
        if self.send_error is not None:
            raise self.send_error
        if stream not in self.proved:
            raise ProtocolError.InvalidArgument("no such stream")
        self.sent.append((stream, bytes(body)))

    def close_stream(self, stream: int) -> None:
        self.closed_streams.append(stream)
        if stream in self.proved:
            self.proved.discard(stream)
            self.events.append(_event(WebrtcDirectEventKind.CLOSED, stream))

    def tick(self) -> int | None:
        self.tick_calls += 1
        return self.ticks.pop(0) if self.ticks else None

    # -- the script ------------------------------------------------------------

    def prove(self, stream: int, address: str, remote_ip: str = PAGE_IP, port: int = 50000) -> None:
        self.proved.add(stream)
        self.events.append(
            WebrtcDirectEvent(
                kind=WebrtcDirectEventKind.PROVED,
                stream=stream,
                address=address,
                remote_ip=remote_ip,
                remote_port=port,
                data=None,
            )
        )

    def body(self, stream: int, data: bytes) -> None:
        self.events.append(_event(WebrtcDirectEventKind.BODY, stream, data))

    def end(self, stream: int) -> None:
        """The page went: the listener reports the stream closed."""
        self.proved.discard(stream)
        self.events.append(_event(WebrtcDirectEventKind.CLOSED, stream))


def _event(kind: WebrtcDirectEventKind, stream: int, data: bytes | None = None) -> WebrtcDirectEvent:
    return WebrtcDirectEvent(
        kind=kind,
        stream=stream,
        address=None,
        remote_ip=None,
        remote_port=None,
        data=list(data) if data is not None else None,
    )


@pytest.fixture
def fake_listener() -> FakeListener:
    return FakeListener()


@pytest.fixture
def protocol(fake_listener):
    p = make_protocol()
    queue: list = []
    p.wifi_direct_get_next_message = MagicMock(side_effect=lambda: queue.pop(0) if queue else None)
    p.outbound = queue
    fake_listener.install(p)
    return p


async def started(protocol, **kwargs) -> PeerStreamManager:
    manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0, **kwargs)
    await manager.start()
    # The fake refuses nothing, so the socket can be on loopback here.
    await manager.open_webrtc_direct("127.0.0.1")
    return manager


@pytest.fixture
async def host(protocol, fake_verifier):
    manager = await started(protocol)
    try:
        yield manager
    finally:
        await manager.stop()


def deliver(manager: PeerStreamManager) -> None:
    """Hand the fake's queued events to the manager, as a datagram or a
    timer would."""
    assert manager._webrtc is not None
    manager._webrtc.pump()


async def recv_datagram(sock: socket.socket, timeout: float) -> tuple[bytes, object]:
    """One datagram on a non-blocking socket, or ``asyncio.TimeoutError``.
    (``loop.sock_recvfrom`` is 3.11 and later; the package supports 3.10.)"""
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    while True:
        try:
            return sock.recvfrom(2048)
        except BlockingIOError:
            pass
        if loop.time() > deadline:
            raise asyncio.TimeoutError
        await asyncio.sleep(0.01)


async def settle() -> None:
    """Let work the manager scheduled (a superseded stream's close) run."""
    for _ in range(3):
        await asyncio.sleep(0)


# ---------------------------------------------------------------------------
# One table, one announcement, one loss
# ---------------------------------------------------------------------------


class TestDialledStreams:
    async def test_a_proved_dial_is_announced_once_and_carries_bodies_both_ways(
        self, host, protocol, fake_listener
    ):
        fake_listener.prove(1, PEER_ADDRESS)
        deliver(host)
        protocol.wifi_direct_peer_connected.assert_called_once_with(peer_id=PEER_ADDRESS)
        assert host.connected_peers() == [PEER_ADDRESS]
        assert host.get_metrics()["dialled_streams"] == 1

        fake_listener.body(1, b"from the page")
        deliver(host)
        protocol.wifi_direct_message_received.assert_called_once_with(
            sender_id=PEER_ADDRESS, data=list(b"from the page")
        )

        protocol.outbound.append(body_for(PEER_ADDRESS, b"to the page"))
        host._drain()
        assert fake_listener.sent == [(1, b"to the page")]

        fake_listener.end(1)
        deliver(host)
        protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)
        assert host.connected_peers() == []
        protocol.wifi_direct_peer_connected.assert_called_once()

    async def test_a_newer_dial_supersedes_the_older_silently(self, host, protocol, fake_listener):
        # The page reloaded: its old stream has not timed out yet. The newer
        # takes the address over, the older is closed with no loss, and the
        # core is not told a second time.
        fake_listener.prove(1, PEER_ADDRESS)
        deliver(host)
        fake_listener.prove(2, PEER_ADDRESS)
        # A body from the older stream after the newer proved belongs to no
        # announced peer.
        fake_listener.body(1, b"stale")
        deliver(host)
        await settle()

        assert fake_listener.closed_streams == [1], "the older dialled stream is the one closed"
        protocol.wifi_direct_peer_connected.assert_called_once_with(peer_id=PEER_ADDRESS)
        protocol.wifi_direct_peer_disconnected.assert_not_called()
        protocol.wifi_direct_message_received.assert_not_called()
        assert host.connected_peers() == [PEER_ADDRESS]
        assert host.get_metrics()["streams_superseded"] == 1

        fake_listener.body(2, b"after the reload")
        deliver(host)
        protocol.wifi_direct_message_received.assert_called_once_with(
            sender_id=PEER_ADDRESS, data=list(b"after the reload")
        )
        protocol.outbound.append(body_for(PEER_ADDRESS, b"reply"))
        host._drain()
        assert fake_listener.sent == [(2, b"reply")]

        fake_listener.end(2)
        deliver(host)
        protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)

    async def test_a_tcp_stream_keeps_its_address_against_a_dial(self, host, protocol, fake_listener):
        reader, writer = await handshake(host)
        await until(lambda: protocol.wifi_direct_peer_connected.called)

        fake_listener.prove(1, PEER_ADDRESS)
        deliver(host)
        assert fake_listener.closed_streams == [1], "the dial is refused"
        protocol.wifi_direct_peer_connected.assert_called_once()
        protocol.wifi_direct_peer_disconnected.assert_not_called()
        assert host.get_metrics()["dialled_streams"] == 0

        # The TCP stream still holds the address.
        writer.write(frame(b"over tcp"))
        await writer.drain()
        await until(lambda: protocol.wifi_direct_message_received.called)
        protocol.wifi_direct_message_received.assert_called_once_with(
            sender_id=PEER_ADDRESS, data=list(b"over tcp")
        )
        writer.close()
        await until(lambda: protocol.wifi_direct_peer_disconnected.called)
        protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)

    async def test_a_dial_keeps_its_address_against_a_tcp_stream(self, host, protocol, fake_listener):
        fake_listener.prove(1, PEER_ADDRESS)
        deliver(host)

        # PEER_ADDRESS is the lower address, so this inbound TCP stream is
        # the kind that wins against TCP; against a dial it is refused.
        reader, writer = await handshake(host, PEER_ASSERTION)
        assert await closed_by_peer(reader), "the TCP stream is the one closed"
        assert fake_listener.closed_streams == []
        protocol.wifi_direct_peer_connected.assert_called_once_with(peer_id=PEER_ADDRESS)
        protocol.wifi_direct_peer_disconnected.assert_not_called()
        assert host.connected_peers() == [PEER_ADDRESS]
        writer.close()

    async def test_a_dial_proving_our_own_address_is_refused(self, host, protocol, fake_listener):
        fake_listener.prove(1, OUR_ADDRESS)
        deliver(host)
        assert fake_listener.closed_streams == [1]
        protocol.wifi_direct_peer_connected.assert_not_called()
        assert host.connected_peers() == []
        fake_listener.body(1, b"loop")
        deliver(host)
        protocol.wifi_direct_message_received.assert_not_called()
        protocol.wifi_direct_peer_disconnected.assert_not_called()


class TestCaps:
    async def test_dialled_streams_count_against_the_caps(self, protocol, fake_listener, fake_verifier):
        manager = await started(protocol, max_streams=3, max_streams_per_host=2)
        try:
            a, b, c = "192.168.1.21", "192.168.1.22", "192.168.1.23"
            fake_listener.prove(1, PEER_ADDRESS, a)
            fake_listener.prove(2, OTHER_ADDRESS, a)
            fake_listener.prove(3, THIRD_ADDRESS, a)  # a third from one page's address
            fake_listener.prove(4, THIRD_ADDRESS, b)
            fake_listener.prove(5, FOURTH_ADDRESS, c)  # a fourth stream
            deliver(manager)
            assert fake_listener.closed_streams == [3, 5]
            assert sorted(manager.connected_peers()) == sorted(
                [PEER_ADDRESS, OTHER_ADDRESS, THIRD_ADDRESS]
            )
            assert manager.get_metrics()["streams_refused"] == 2

            # At both caps, a page reloading is still let in: the stream it
            # supersedes is not counted against it.
            fake_listener.prove(6, PEER_ADDRESS, a)
            deliver(manager)
            await settle()
            assert fake_listener.closed_streams == [3, 5, 1]
            assert protocol.wifi_direct_peer_connected.call_count == 3
            protocol.wifi_direct_peer_disconnected.assert_not_called()
        finally:
            await manager.stop()

    async def test_a_dialled_stream_counts_as_inbound_from_its_page(
        self, protocol, fake_listener, fake_verifier
    ):
        # One inbound stream per remote host: a page on 127.0.0.1 holds the
        # host's one, so a TCP connection from the same address is refused.
        manager = await started(protocol, max_streams_per_host=1)
        try:
            fake_listener.prove(1, PEER_ADDRESS, "127.0.0.1")
            deliver(manager)
            assert manager.connected_peers() == [PEER_ADDRESS]
            reader, writer = await asyncio.open_connection("127.0.0.1", manager.listen_port)
            assert await closed_by_peer(reader), "refused, and no preamble sent"
            assert manager.get_metrics()["streams_refused"] == 1
            writer.close()
        finally:
            await manager.stop()


class TestListenerLifecycle:
    async def test_closing_the_listener_reports_each_held_address_lost_once(
        self, host, protocol, fake_listener
    ):
        fake_listener.prove(1, PEER_ADDRESS)
        fake_listener.prove(2, OTHER_ADDRESS)
        fake_listener.prove(3, PEER_ADDRESS)  # supersedes 1
        deliver(host)
        await settle()

        host.close_webrtc_direct()
        assert fake_listener.close_calls == 1
        lost = sorted(c.kwargs["peer_id"] for c in protocol.wifi_direct_peer_disconnected.call_args_list)
        assert lost == sorted([PEER_ADDRESS, OTHER_ADDRESS])
        assert host.connected_peers() == []
        assert host.webrtc_direct_code is None
        assert host.get_metrics()["webrtc_direct_port"] is None
        host.close_webrtc_direct()  # nothing more to close
        assert fake_listener.close_calls == 1

    async def test_stopping_the_transport_closes_the_listener(self, protocol, fake_listener, fake_verifier):
        manager = await started(protocol)
        fake_listener.prove(1, PEER_ADDRESS)
        deliver(manager)
        await manager.stop()
        assert fake_listener.close_calls == 1
        protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)
        assert manager.webrtc_direct_code is None

    async def test_opening_again_replaces_the_listener_and_its_streams(self, host, protocol, fake_listener):
        fake_listener.prove(1, PEER_ADDRESS)
        deliver(host)
        old_port = host.get_metrics()["webrtc_direct_port"]
        await host.open_webrtc_direct("127.0.0.1")
        assert fake_listener.close_calls == 1
        protocol.wifi_direct_peer_disconnected.assert_called_once_with(peer_id=PEER_ADDRESS)
        assert len(fake_listener.opened) == 2
        # A fresh socket, so a fresh port.
        assert fake_listener.opened[1][1] != old_port

    async def test_the_code_is_returned_and_never_logged(self, protocol, fake_listener, fake_verifier, caplog):
        caplog.set_level(logging.DEBUG)
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        diagnostics: list = []
        manager.set_delegate(on_diagnostic=lambda *d: diagnostics.append(d))
        await manager.start()
        try:
            fragment = await manager.open_webrtc_direct("127.0.0.1")
            assert fragment == FRAGMENT
            assert manager.webrtc_direct_code.fragment == FRAGMENT
            fake_listener.prove(1, PEER_ADDRESS)
            fake_listener.body(1, b"x")
            deliver(manager)
            manager.close_webrtc_direct()
        finally:
            await manager.stop()
        assert PASSWORD not in caplog.text
        assert PASSWORD not in repr(diagnostics)
        assert any(d[1] == "WebRTC Direct listener open" for d in diagnostics)

    async def test_open_needs_a_running_transport(self, protocol, fake_listener):
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        with pytest.raises(TransportError):
            await manager.open_webrtc_direct("127.0.0.1")
        assert fake_listener.opened == []

    async def test_a_refused_open_leaves_nothing_open(self, protocol, fake_listener, fake_verifier):
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        try:
            fake_listener.open_error = ProtocolError.InvalidArgument("not a private address")
            with pytest.raises(TransportError):
                await manager.open_webrtc_direct("127.0.0.1")
            assert manager.webrtc_direct_code is None
            with pytest.raises(TransportError):
                await manager.open_webrtc_direct("not-an-address")
        finally:
            await manager.stop()
        assert fake_listener.close_calls == 0

    async def test_a_close_while_the_open_binds_wins(self, protocol, fake_listener, fake_verifier):
        # The application closes while the socket is being bound, when no
        # listener is installed yet: the open must not install one after,
        # or a code the application believes dead admits dials (R35).
        manager = PeerStreamManager(protocol, listen_host="127.0.0.1", listen_port=0)
        await manager.start()
        loop = asyncio.get_running_loop()
        binding = asyncio.Event()
        release = asyncio.Event()
        transports: list[asyncio.DatagramTransport] = []
        bind = loop.create_datagram_endpoint

        async def gated_bind(*args, **kwargs):
            binding.set()
            await release.wait()
            transport, protocol_ = await bind(*args, **kwargs)
            transports.append(transport)
            return transport, protocol_

        loop.create_datagram_endpoint = gated_bind  # type: ignore[method-assign]
        try:
            opening = asyncio.create_task(manager.open_webrtc_direct("127.0.0.1"))
            await binding.wait()
            manager.close_webrtc_direct()
            release.set()
            with pytest.raises(TransportError, match="closed while it opened"):
                await opening
            assert manager.webrtc_direct_code is None
            assert manager.get_metrics()["webrtc_direct_port"] is None
            # The core's listener was opened and closed again, and the socket
            # with it.
            assert len(fake_listener.opened) == 1
            assert fake_listener.close_calls == 1
            assert len(transports) == 1 and transports[0].is_closing()
            # A later open is the application's own and goes ahead.
            del loop.create_datagram_endpoint
            assert await manager.open_webrtc_direct("127.0.0.1") == FRAGMENT
        finally:
            if "create_datagram_endpoint" in vars(loop):
                del loop.create_datagram_endpoint
            await manager.stop()

    async def test_a_body_the_listener_refuses_is_dropped(self, host, protocol, fake_listener):
        fake_listener.prove(1, PEER_ADDRESS)
        deliver(host)
        fake_listener.send_error = ProtocolError.InvalidArgument("too much queued")
        protocol.outbound.append(body_for(PEER_ADDRESS, b"refused"))
        host._drain()
        assert host.get_metrics()["frames_dropped"] == 1
        assert host.connected_peers() == [PEER_ADDRESS]


class TestSocket:
    async def test_datagrams_move_both_ways_and_the_timer_follows_the_tick(
        self, host, protocol, fake_listener
    ):
        port = host.get_metrics()["webrtc_direct_port"]
        client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        client.bind(("127.0.0.1", 0))
        client.setblocking(False)
        try:
            # Received: handed over unfiltered, with its source.
            fake_listener.ticks = [20, 20, None]
            calls_before = fake_listener.tick_calls
            client.sendto(b"\x00\x01check", ("127.0.0.1", port))
            await until(lambda: fake_listener.received)
            assert fake_listener.received == [("127.0.0.1", client.getsockname()[1], b"\x00\x01check")]
            # The tick the datagram armed fires again without another input.
            await until(lambda: fake_listener.tick_calls >= calls_before + 3, timeout=1.0)

            # Queued: sent from the listener's socket.
            fake_listener.datagrams.append(
                WebrtcDirectDatagram(
                    destination_ip="127.0.0.1",
                    destination_port=client.getsockname()[1],
                    data=list(b"answer"),
                )
            )
            deliver(host)
            data, source = await recv_datagram(client, 2.0)
            assert (data, source) == (b"answer", ("127.0.0.1", port))
        finally:
            client.close()


# ---------------------------------------------------------------------------
# The real library
# ---------------------------------------------------------------------------


def _private_ipv4() -> str | None:
    """A private, non-loopback IPv4 address of this machine, or ``None``.
    ``DIRECT_HOST`` names one; otherwise the address the kernel would use
    toward a documentation address (no packet is sent)."""
    candidates = [os.environ.get("DIRECT_HOST")]
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        probe.connect(("192.0.2.1", 9))
        candidates.append(probe.getsockname()[0])
    except OSError:
        pass
    finally:
        probe.close()
    for text in candidates:
        if not text:
            continue
        try:
            ip = ipaddress.ip_address(text)
        except ValueError:
            continue
        # IPv4 link-local is refused by the listener (str0m will not use it).
        if ip.version == 4 and ip.is_private and not ip.is_loopback and not ip.is_link_local:
            return text
    return None


def _config() -> ProtocolConfig:
    return ProtocolConfig(
        app_id="test-app",
        profile="webrtc-direct-host",
        ble_enabled=False,
        wifi_direct_enabled=True,
        internet_enabled=False,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=False,
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


class TestRealListener:
    async def test_the_host_opens_a_listener_on_a_private_address_and_answers_no_junk(self):
        ip = _private_ipv4()
        if ip is None:
            pytest.skip(
                "this machine has no private IPv4 address: a dial code names one, and the "
                "listener refuses loopback (set DIRECT_HOST to choose one)"
            )
        protocol = OfflineProtocol(_config())
        protocol.initialize_mls(InMemoryStorage(), InMemoryStorage())
        manager = PeerStreamManager(protocol, listen_port=None)
        await manager.start()
        try:
            fragment = await manager.open_webrtc_direct(ip)
            code = manager.webrtc_direct_code
            assert fragment == code.fragment
            fields = dict(part.split("=", 1) for part in fragment.split("&"))
            assert fields["op-direct"] == "1"
            assert (fields["h"], int(fields["p"]), fields["a"]) == (code.host, code.port, code.address)
            assert (code.host, code.address) == (ip, protocol.local_address())
            assert code.port == manager.get_metrics()["webrtc_direct_port"] != 0
            assert re.fullmatch(r"[A-Za-z0-9_-]{43}", fields["c"])
            assert re.fullmatch(r"[A-Za-z0-9]{16}", fields["u"])
            assert re.fullmatch(r"[A-Za-z0-9]{32}", fields["w"])

            # Anything that is not an authenticated check gets no reply.
            client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            client.bind((ip, 0))
            client.setblocking(False)
            try:
                client.sendto(b"\x00\x01\x00\x00junk that is not a binding request", (ip, code.port))
                with pytest.raises(asyncio.TimeoutError):
                    await recv_datagram(client, 0.5)
            finally:
                client.close()
            assert protocol.webrtc_direct_tick() is None, "nothing is dialling"

            manager.close_webrtc_direct()
            assert manager.webrtc_direct_code is None
            with pytest.raises(ProtocolError):
                protocol.webrtc_direct_datagram_received(ip, 50000, [0] * 20)
            # The socket is closed with it (asyncio closes it on the next
            # loop pass): its port can be bound again.
            await settle()
            again = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                again.bind((ip, code.port))
            finally:
                again.close()
        finally:
            await manager.stop()
