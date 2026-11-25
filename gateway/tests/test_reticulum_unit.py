"""The Reticulum backbone's own rules, without a Reticulum instance: who may
call the request handlers, what a forward must carry, and how presence
requests change the table."""

from __future__ import annotations

import asyncio

import pytest

RNS = pytest.importorskip("RNS")

from conftest import address  # noqa: E402
from offline_gateway.backbone import ForwardResult  # noqa: E402
from offline_gateway.config import ReticulumConfig  # noqa: E402
from offline_gateway.reticulum import APP_NAME, ASPECTS, ReticulumBackbone, _frame_from  # noqa: E402


class Hooks:
    def __init__(self) -> None:
        self.changed: list[str] = []
        self.delivered: list = []

    async def deliver_from_backbone(self, frame, peer):
        self.delivered.append((frame, peer))
        return ForwardResult.DELIVERED, ""

    def local_addresses(self):
        return [address(1)]

    def remote_presence_changed(self, addr):
        self.changed.append(addr)


def _peer() -> tuple[object, str]:
    identity = RNS.Identity()
    return identity, RNS.Destination.hash(identity, APP_NAME, *ASPECTS).hex()


def _backbone(peers: list[str]) -> tuple[ReticulumBackbone, Hooks]:
    backbone = ReticulumBackbone(ReticulumConfig(peers=peers))
    hooks = Hooks()
    backbone.hooks = hooks
    return backbone, hooks


def test_a_forward_needs_addresses_and_bytes():
    good = {"r": address(1), "s": address(2), "c": b"\x00\x01", "m": "id-1", "p": None}
    frame = _frame_from(good)
    assert frame is not None and frame.content == b"\x00\x01" and frame.message_id == "id-1"
    assert _frame_from({**good, "c": "text, not bytes"}) is None
    assert _frame_from({**good, "r": "nobody"}) is None
    assert _frame_from({**good, "s": None}) is None
    assert _frame_from([good]) is None
    # An id the daemon would not echo is dropped, not carried.
    assert _frame_from({**good, "m": "not valid!"}).message_id is None


def test_an_unprovisioned_caller_is_refused_by_every_handler():
    _, provisioned = _peer()
    stranger, _ = _peer()
    backbone, hooks = _backbone([provisioned])
    request = {"r": address(1), "s": address(2), "c": b"x"}
    answer = backbone._handle_forward("/forward", request, b"id", b"link", stranger, 0)
    assert answer["o"] == ForwardResult.REFUSED.value
    assert backbone._handle_forward("/forward", request, b"id", b"link", None, 0)["o"] == "refused"
    assert backbone._handle_presence("/presence", {"snap": True, "on": []}, b"id", b"link", stranger, 0) is None
    assert backbone._handle_snapshot("/snapshot", None, b"id", b"link", stranger, 0) is None
    assert not hooks.delivered


async def test_presence_requests_update_the_table_and_tell_the_daemon():
    identity, peer_hex = _peer()
    backbone, hooks = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    peer = bytes.fromhex(peer_hex)

    backbone._apply(peer, {"snap": True, "on": [address(5), "garbage"], "off": []})
    assert backbone.claim(address(5)).online
    assert backbone.claim("garbage") is None
    assert hooks.changed == [address(5)]

    backbone._apply(peer, {"snap": False, "on": [], "off": [[address(5), 1234]]})
    claim = backbone.claim(address(5))
    assert not claim.online and claim.last_seen_ms == 1234

    backbone._apply(peer, {"snap": False, "on": [address(6)], "off": [[address(7), True], "bad"]})
    assert backbone.claim(address(6)).online
    assert backbone.claim(address(7)).last_seen_ms is None  # a bool is not a time

    # Losing the peer's link withdraws everything it claimed.
    hooks.changed.clear()
    backbone._forget(peer)
    assert backbone.claim(address(6)) is None
    assert set(hooks.changed) == {address(5), address(6), address(7)}


async def test_a_forward_with_no_claim_is_unreachable_without_touching_a_link():
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    from offline_gateway.backbone import BackboneFrame

    result, _ = await backbone.forward(BackboneFrame(address(9), address(1), b"x", "m", None), 5)
    assert result is ForwardResult.UNREACHABLE


async def test_presence_changes_are_coalesced_into_one_update():
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    sent: list[dict] = []
    backbone._active_out = lambda peer: object()  # type: ignore[method-assign]

    async def capture(link, peer, update):
        sent.append(update)

    backbone._send_presence = capture  # type: ignore[method-assign]
    import offline_gateway.reticulum as module

    original = module.PRESENCE_COALESCE
    module.PRESENCE_COALESCE = 0.05
    try:
        backbone.local_presence_changed(address(1), True, None)
        backbone.local_presence_changed(address(2), True, None)
        backbone.local_presence_changed(address(1), False, 99)
        await asyncio.sleep(0.2)
    finally:
        module.PRESENCE_COALESCE = original
    assert sent == [{"snap": False, "on": [address(2)], "off": [[address(1), 99]]}]


def test_peers_must_be_destination_hashes():
    from offline_gateway import ConfigError, GatewayConfig

    with pytest.raises(ConfigError):
        GatewayConfig(reticulum=ReticulumConfig(peers=["not hex"])).validate()
    with pytest.raises(ConfigError):
        GatewayConfig(reticulum=ReticulumConfig(peers=["ab" * 8])).validate()
    GatewayConfig(reticulum=ReticulumConfig(peers=["ab" * 16])).validate()


def test_the_example_config_loads():
    from pathlib import Path

    from offline_gateway.config import load

    config = load(Path(__file__).resolve().parent.parent / "examples" / "gateway.toml")
    assert config.listen_host == "0.0.0.0" and config.listen_port == 4242
    assert config.reticulum is not None
    assert config.reticulum.configdir.endswith("examples/rns")
    assert config.reticulum.identity_path.endswith("examples/gateway.identity")
