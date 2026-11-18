"""The WebRTC Direct listener behind ``webrtc_direct_*`` (ADR 0039).

The Python package ships the library with the ``webrtc-direct`` feature
(decision 10), and ``scripts/build-desktop.sh`` builds the one these tests
load the same way. A library built without it still has every method, so
the bindings load and every other test passes, and ``webrtc_direct_open``
refuses on every host. This is the one test that notices.

The listener's behaviour (admission, dials, streams) is pinned in Rust, in
``offline-protocol-webrtc-direct`` and in the FFI crate's own tests; here
only the boundary's shape is.
"""

from __future__ import annotations

import pytest

from offline_protocol_sdk.offline_protocol import (
    OfflineProtocol,
    OverflowPolicy,
    ProtocolConfig,
    ProtocolError,
    WebrtcDirectCode,
)

from conftest import InMemoryStorage


def _config(profile: str) -> ProtocolConfig:
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
        auto_key_exchange=True,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )


def test_the_shipped_library_opens_a_listener_with_this_devices_code() -> None:
    protocol = OfflineProtocol(_config("webrtc-direct-device"))
    protocol.initialize_mls(InMemoryStorage(), InMemoryStorage())
    address = protocol.local_address()
    assert address is not None

    code = protocol.webrtc_direct_open("192.168.1.10", 40000)
    assert isinstance(code, WebrtcDirectCode)
    assert code.fragment.startswith("op-direct=1&")
    assert f"&a={address}" in code.fragment
    assert (code.host, code.port, code.address) == ("192.168.1.10", 40000, address)

    # Nothing is dialling, so there is nothing to send, report or time.
    protocol.webrtc_direct_datagram_received("192.168.1.20", 50000, [0] * 20)
    assert protocol.webrtc_direct_next_datagram() is None
    assert protocol.webrtc_direct_next_event() is None
    assert protocol.webrtc_direct_tick() is None
    with pytest.raises(ProtocolError):
        protocol.webrtc_direct_send(0, list(b"body"))

    protocol.webrtc_direct_close()
    with pytest.raises(ProtocolError):
        protocol.webrtc_direct_datagram_received("192.168.1.20", 50000, [0] * 20)


def test_a_code_never_names_an_address_a_page_cannot_dial() -> None:
    protocol = OfflineProtocol(_config("webrtc-direct-refusals"))
    protocol.initialize_mls(InMemoryStorage(), InMemoryStorage())
    for ip in ("127.0.0.1", "8.8.8.8", "0.0.0.0"):
        with pytest.raises(ProtocolError.InvalidArgument):
            protocol.webrtc_direct_open(ip, 40000)
