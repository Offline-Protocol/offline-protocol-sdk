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

    presence_epoch = "test-epoch"

    def local_snapshot(self):
        return 1, {address(1): 1}

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

    backbone._apply(peer, {"e": "x", "snap": True, "q": 1, "on": [[address(5), 1], ["garbage", 1]], "off": []})
    assert backbone.claim(address(5)).online
    assert backbone.claim("garbage") is None
    assert hooks.changed == [address(5)]

    backbone._apply(peer, {"e": "x", "snap": False, "on": [], "off": [[address(5), 1234, 2]]})
    claim = backbone.claim(address(5))
    assert not claim.online and claim.last_seen_ms == 1234

    backbone._apply(
        peer,
        {"e": "x", "snap": False, "on": [[address(6), 3]], "off": [[address(7), True, 4], "bad", [address(8), 1]]},
    )
    assert backbone.claim(address(6)).online
    assert backbone.claim(address(7)).last_seen_ms is None  # a bool is not a time
    assert backbone.claim(address(8)) is None  # an entry with no number is dropped

    # A snapshot with no number of its own cannot be ordered, so it is refused.
    backbone._apply(peer, {"e": "x", "snap": True, "on": [], "off": []})
    assert backbone.claim(address(6)).online

    # Losing the peer's link withdraws everything it claimed.
    hooks.changed.clear()
    backbone._forget(peer)
    assert backbone.claim(address(6)) is None
    assert set(hooks.changed) == {address(5), address(6), address(7)}


async def test_the_snapshot_this_gateway_serves_carries_its_numbering():
    identity, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    assert backbone._snapshot_message() == {
        "e": "test-epoch",
        "snap": True,
        "q": 1,
        "on": [[address(1), 1]],
        "off": [],
    }


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
        backbone.local_presence_changed(address(1), True, None, 1)
        backbone.local_presence_changed(address(2), True, None, 2)
        backbone.local_presence_changed(address(1), False, 99, 3)
        await asyncio.sleep(0.2)
    finally:
        module.PRESENCE_COALESCE = original
    assert sent == [{"e": "test-epoch", "snap": False, "on": [[address(2), 2]], "off": [[address(1), 99, 3]]}]


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


async def test_a_link_the_peer_cannot_attribute_is_torn_down_for_a_redial():
    """A lost identify packet leaves a link every request on which is
    refused. A forward refused for that reason drops the link rather than
    failing every frame on it from then on."""
    from offline_gateway.backbone import BackboneFrame
    from offline_gateway.reticulum import NOT_A_PEER

    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    backbone.table.update(peer_hex, address(2), True, None, 1)

    class Link:
        torn = False

        def teardown(self):
            self.torn = True

    link = Link()
    backbone._active_out = lambda peer: link  # type: ignore[method-assign]

    async def refused(link, path, data, timeout, admit=None):
        return {"o": "refused", "w": NOT_A_PEER}

    backbone._request = refused  # type: ignore[method-assign]
    result, _ = await backbone.forward(BackboneFrame(address(2), address(1), b"x", "m", None), 5)
    assert result is ForwardResult.TIMEOUT
    assert link.torn


class _Resource:
    def __init__(self, status):
        self.status = status
        self.cancelled = False

    def cancel(self):
        self.cancelled = True


class _Receipt:
    def __init__(self, resource):
        self.resource = resource


async def test_a_request_that_times_out_cancels_its_transfer(monkeypatch):
    import offline_gateway.reticulum as module

    monkeypatch.setattr(module, "MIN_TRANSFER_WINDOW", 0.0)
    """A frame the device was told failed must stop spending airtime, or its
    retry runs a second transfer beside the first."""
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    resource = _Resource(RNS.Resource.TRANSFERRING)

    class SilentLink:
        def request(self, path, data, response_callback, failed_callback, timeout):
            return _Receipt(resource)

    with pytest.raises(asyncio.TimeoutError):
        await backbone._request(SilentLink(), "/forward", {}, 0.05)
    assert resource.cancelled


async def test_reticulums_own_timeout_also_cancels_the_transfer(monkeypatch):
    import offline_gateway.reticulum as module

    monkeypatch.setattr(module, "MIN_TRANSFER_WINDOW", 0.0)
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    resource = _Resource(RNS.Resource.TRANSFERRING)

    class FailingLink:
        def request(self, path, data, response_callback, failed_callback, timeout):
            receipt = _Receipt(resource)
            failed_callback(receipt)
            return receipt

    with pytest.raises(asyncio.TimeoutError):
        await backbone._request(FailingLink(), "/forward", {}, 5)
    assert resource.cancelled


async def test_a_finished_transfer_is_left_alone():
    from offline_gateway.reticulum import _cancel_transfer

    done = _Resource(RNS.Resource.COMPLETE)
    _cancel_transfer(_Receipt(done))
    _cancel_transfer(_Receipt(None))
    assert not done.cancelled


async def test_an_inbound_link_that_never_identifies_is_closed():
    identity, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()

    class Link:
        torn = False

        def teardown(self):
            self.torn = True

    anonymous, known = Link(), Link()
    backbone._in[bytes.fromhex(peer_hex)] = known
    backbone._drop_if_anonymous(anonymous)
    backbone._drop_if_anonymous(known)
    assert anonymous.torn and not known.torn


async def test_requests_on_a_link_go_one_at_a_time_and_a_late_one_never_reaches_reticulum(monkeypatch):
    """Reticulum parks a second resource on a link and cannot withdraw it.
    The daemon holds it instead: a frame whose time runs out before its turn
    is refused without ever being handed over."""
    import offline_gateway.reticulum as module
    from offline_gateway.reticulum import QueuedTooLong

    monkeypatch.setattr(module, "MIN_TRANSFER_WINDOW", 0.0)
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    handed_over: list[str] = []
    answers: list = []

    class OneAtATimeLink:
        def request(self, path, data, response_callback, failed_callback, timeout):
            handed_over.append(data)
            receipt = _Receipt(_Resource(RNS.Resource.TRANSFERRING))
            receipt.response = "ok"
            answers.append(lambda: response_callback(receipt))
            return receipt

    link = OneAtATimeLink()
    first = asyncio.ensure_future(backbone._request(link, "/forward", "first", 5))
    await asyncio.sleep(0.05)
    with pytest.raises(QueuedTooLong):
        await backbone._request(link, "/forward", "late", 0.1)
    assert handed_over == ["first"]

    # Once the first is answered the lane frees, and the next one goes.
    answers[0]()
    assert await first == "ok"
    second = asyncio.ensure_future(backbone._request(link, "/forward", "second", 5))
    await asyncio.sleep(0.05)
    assert handed_over == ["first", "second"]
    answers[1]()
    assert await second == "ok"


async def test_a_frame_queued_too_long_is_a_backbone_timeout():
    from offline_gateway.backbone import BackboneFrame
    from offline_gateway.reticulum import QueuedTooLong

    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    backbone.table.update(peer_hex, address(2), True, None, 1)
    backbone._active_out = lambda peer: object()  # type: ignore[method-assign]

    async def queued(link, path, data, timeout, admit=None):
        raise QueuedTooLong()

    backbone._request = queued  # type: ignore[method-assign]
    result, reason = await backbone.forward(BackboneFrame(address(2), address(1), b"x", "m", None), 5)
    assert result is ForwardResult.TIMEOUT and "queued" in reason


async def test_a_lost_presence_update_is_resent_while_the_link_lasts():
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone.config.reconnect_interval = 0.01
    backbone._loop = asyncio.get_running_loop()
    link = object()
    active = {"link": link}
    backbone._active_out = lambda peer: active["link"]  # type: ignore[method-assign]
    attempts: list = []

    async def lossy(l, path, data, timeout):
        attempts.append(data)
        if len(attempts) < 3:
            raise asyncio.TimeoutError()
        return True

    backbone._request = lossy  # type: ignore[method-assign]
    await backbone._send_presence(link, bytes.fromhex(peer_hex), {"on": []})
    assert len(attempts) == 3

    # A link that has gone is not retried on; the next one re-snapshots.
    attempts.clear()

    async def silent(l, path, data, timeout):
        attempts.append(data)
        active["link"] = None
        raise asyncio.TimeoutError()

    backbone._request = silent  # type: ignore[method-assign]
    await backbone._send_presence(link, bytes.fromhex(peer_hex), {"on": []})
    assert len(attempts) == 1


async def test_a_peer_that_never_answers_is_redialled_less_and_less_often(monkeypatch):
    """One-sided provisioning: the peer takes the link and refuses everything.
    Without backoff that is a redial, and half a dozen packets, every time."""
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone.config.reconnect_interval = 0.01
    backbone._identity = RNS.Identity()
    backbone._loop = asyncio.get_running_loop()
    backbone._running = True
    dials: list[float] = []

    class FakeLink:
        def __init__(self, destination, established_callback, closed_callback):
            dials.append(asyncio.get_running_loop().time())
            self.status = RNS.Link.ACTIVE
            established_callback(self)

        def identify(self, identity):
            pass

        def teardown(self):
            self.status = RNS.Link.CLOSED

    monkeypatch.setattr(RNS.Transport, "has_path", staticmethod(lambda h: True))
    monkeypatch.setattr(RNS.Identity, "recall", staticmethod(lambda h: object()))
    monkeypatch.setattr(RNS, "Destination", type("D", (), {"OUT": 0, "SINGLE": 0, "__init__": lambda self, *a: None}))
    monkeypatch.setattr(RNS, "Link", type("L", (FakeLink,), {"ACTIVE": RNS.Link.ACTIVE, "CLOSED": RNS.Link.CLOSED}))

    async def refused(link, peer):
        link.teardown()
        return False

    backbone._exchange_snapshots = refused  # type: ignore[method-assign]
    task = asyncio.ensure_future(backbone._dial_loop(bytes.fromhex(peer_hex)))
    await asyncio.sleep(0.4)
    backbone._running = False
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    # 0.01, 0.02, 0.04, 0.08, 0.16 s apart: a handful, not hundreds.
    assert 3 <= len(dials) <= 7
    gaps = [b - a for a, b in zip(dials, dials[1:])]
    assert gaps == sorted(gaps)


async def test_an_unexpected_error_does_not_end_the_dial_loop(monkeypatch):
    import offline_gateway.reticulum as module

    monkeypatch.setattr(module, "MAX_DIAL_BACKOFF", 0.01)
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._running = True
    calls: list[int] = []

    async def flaky(peer):
        calls.append(1)
        if len(calls) == 1:
            raise RuntimeError("reticulum had a bad day")
        backbone._running = False

    backbone._dial = flaky  # type: ignore[method-assign]
    await asyncio.wait_for(backbone._dial_loop(bytes.fromhex(peer_hex)), 2)
    assert len(calls) == 2


def test_one_host_may_not_fill_the_unbound_pool():
    from offline_gateway import ConfigError, GatewayConfig

    with pytest.raises(ConfigError):
        GatewayConfig(max_unbound_sessions=8, max_sessions_per_host=8).validate()
    GatewayConfig(max_unbound_sessions=8, max_sessions_per_host=7).validate()


async def test_a_one_packet_request_does_not_wait_behind_a_transfer():
    """A presence update is one packet, which Reticulum sends at once. It
    must not queue behind a frame on the lane, or a lost one would hold the
    lane for its whole timeout and time out every frame behind it."""
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    handed_over: list = []

    class Link:
        mdu = 400

        def request(self, path, data, response_callback, failed_callback, timeout):
            handed_over.append(path)
            receipt = _Receipt(None)
            receipt.response = True
            if path == "/presence":
                response_callback(receipt)
            return receipt

    link = Link()
    big = asyncio.ensure_future(backbone._request(link, "/forward", {"c": b"x" * 2000}, 30))
    await asyncio.sleep(0.05)
    answer = await asyncio.wait_for(backbone._request(link, "/presence", {"on": [[address(1), 1]]}, 5), 1)
    assert answer is True
    assert handed_over == ["/forward", "/presence"]
    big.cancel()
    await asyncio.gather(big, return_exceptions=True)


def test_the_one_packet_test_matches_reticulums_own():
    from offline_gateway.reticulum import _fits_one_packet

    class Link:
        mdu = 400

    assert _fits_one_packet(Link(), "/presence", {"on": [[address(1), 1]]})
    assert not _fits_one_packet(Link(), "/forward", {"c": b"x" * 500})
    assert not _fits_one_packet(object(), "/presence", None)  # no mdu: assume not


async def test_a_frame_that_never_reaches_the_radio_is_never_charged(monkeypatch):
    """Charging at submission spent the budgets on frames that then waited out
    their time in the queue, and charged their retries again."""
    import offline_gateway.reticulum as module
    from offline_gateway.backbone import BackboneFrame

    monkeypatch.setattr(module, "MIN_TRANSFER_WINDOW", 0.0)
    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    backbone.table.update(peer_hex, address(2), True, None, 1)
    charged: list[str] = []

    class Link:
        status = RNS.Link.ACTIVE
        mdu = 400

        def request(self, path, data, response_callback, failed_callback, timeout):
            return _Receipt(_Resource(RNS.Resource.TRANSFERRING))  # never answers

    link = Link()
    backbone._active_out = lambda peer: link  # type: ignore[method-assign]

    def admit(peer):
        charged.append(peer)
        return True

    frame = BackboneFrame(address(2), address(1), b"x" * 1000, "m", None)
    first = asyncio.ensure_future(backbone.forward(frame, 5, admit))
    await asyncio.sleep(0.05)
    result, reason = await backbone.forward(frame, 0.1, admit)
    assert result is ForwardResult.TIMEOUT and "queued" in reason
    assert charged == [peer_hex]  # only the frame that went to the radio
    first.cancel()
    await asyncio.gather(first, return_exceptions=True)


async def test_a_frame_the_budgets_refuse_at_its_turn_is_not_sent():
    from offline_gateway.backbone import BackboneFrame

    _, peer_hex = _peer()
    backbone, _ = _backbone([peer_hex])
    backbone._loop = asyncio.get_running_loop()
    backbone.table.update(peer_hex, address(2), True, None, 1)
    sent: list = []

    class Link:
        status = RNS.Link.ACTIVE
        mdu = 400

        def request(self, *args, **kwargs):
            sent.append(args)
            return False

    backbone._active_out = lambda peer: Link()  # type: ignore[method-assign]
    frame = BackboneFrame(address(2), address(1), b"x" * 1000, "m", None)
    result, reason = await backbone.forward(frame, 30, lambda peer: False)
    assert (result, reason) == (ForwardResult.REFUSED, "budget_exceeded")
    assert not sent
