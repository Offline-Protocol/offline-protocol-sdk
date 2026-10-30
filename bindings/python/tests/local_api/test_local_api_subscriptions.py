"""Service subscriptions on the local API: who may make each call, where a
subscription's events go, and the subscription types on the wire.

The dispatcher runs against a stand-in ``MeshServices`` here, because the
rules under test are the server's and a real subscription needs a second
device with a confirmed session. The engine's own refusals are covered in
the Rust suite.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any

import pytest

from offline_protocol_sdk import offline_protocol as generated
from offline_protocol_sdk.local_api import EventRouter, Policy, ServiceOwnership, Session
from offline_protocol_sdk.local_api.codec import RpcError
from offline_protocol_sdk.local_api.dispatch import Dispatcher
from offline_protocol_sdk.local_api.mux import ISSUED_CAPACITY


def drain(session: Session) -> list[dict]:
    out = []
    while not session._queue.empty():
        out.append(json.loads(session._queue.get_nowait())["params"])
    return out


def attached(router: EventRouter, app_id: str) -> Session:
    session = Session("unix")
    session.app_id = app_id
    router.attach(session)
    return session


def subscriber(sub_id: str, service_id: str) -> generated.ServiceSubscriberInfo:
    return generated.ServiceSubscriberInfo(
        sub_id=sub_id,
        subscriber="off1peer",
        service_id=service_id,
        topic="t",
        mode=generated.SubscriptionMode.LATEST,
        state=generated.SubscriptionState.PENDING,
        params="",
        keys=[],
    )


def subscription(sub_id: str) -> generated.ServiceSubscriptionInfo:
    return generated.ServiceSubscriptionInfo(
        sub_id=sub_id,
        provider="off1peer",
        service_id="svc",
        topic="t",
        mode=generated.SubscriptionMode.LOG,
        state=generated.SubscriptionState.ACTIVE,
        keys=[],
        topics=["t"],
    )


class FakeServices:
    """Records each call and answers like the engine would."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, dict[str, Any]]] = []
        self.subscribers: list[generated.ServiceSubscriberInfo] = []
        self.subscriptions: list[generated.ServiceSubscriptionInfo] = []
        self.next_sub_id = 0

    def subscribe_service(self, **kwargs: Any) -> str:
        self.calls.append(("subscribe_service", kwargs))
        self.next_sub_id += 1
        return f"sub{self.next_sub_id}"

    def service_subscribers(self, service_id: str | None) -> list:
        return [s for s in self.subscribers if service_id in (None, s.service_id)]

    def service_subscriptions(self) -> list:
        return list(self.subscriptions)

    def publish_service_update(self, **kwargs: Any) -> generated.ServicePublishReport:
        self.calls.append(("publish_service_update", kwargs))
        return generated.ServicePublishReport(
            subscribers=3, sent_live=1, queued=1, coalesced=0, failed=1
        )

    def __getattr__(self, name: str) -> Any:
        def call(**kwargs: Any) -> Any:
            self.calls.append((name, kwargs))
            return True if name == "unsubscribe_service" else None

        return call


@pytest.fixture
def world():
    ownership = ServiceOwnership()
    router = EventRouter(Policy(), ownership)
    services = FakeServices()
    dispatcher = Dispatcher(
        None, services, None, router, Policy(), ownership, asyncio.Lock()
    )
    return dispatcher, router, ownership, services


def variant(err: pytest.ExceptionInfo[RpcError]) -> str:
    return (err.value.data or {}).get("variant", "")


# -- routing -----------------------------------------------------------------


async def test_a_subscription_outlives_any_number_of_later_identifiers(world):
    dispatcher, router, _, _ = world
    owner = attached(router, "notes")
    other = attached(router, "other")
    sub_id = await dispatcher.call(
        owner,
        "services.subscribe_service",
        {
            "provider": "off1peer",
            "service_id": "svc",
            "topic": "t",
            "mode": "Latest",
            "params": "",
            "keys": [],
        },
    )
    # A busy server: enough later sends to evict anything in the issued table.
    router.note_ids("other", [f"m{i}" for i in range(ISSUED_CAPACITY + 10)])
    router.route({"type": "service_update_received", "sub_id": sub_id, "service_id": "svc", "body": "secret"})
    assert [e["body"] for e in drain(owner)] == ["secret"]
    assert drain(other) == []


async def test_an_ended_subscription_leaves_the_table_and_its_stragglers_still_route(router_only):
    router = router_only
    owner = attached(router, "notes")
    other = attached(router, "other")
    router.note_subscription("notes", "s1")
    ended = {
        "type": "service_subscription_changed",
        "sub_id": "s1",
        "role": "consumer",
        "service_id": "svc",
        "state": "ended",
    }
    router.route(ended)
    assert router.subscription_count() == 0
    router.route({"type": "service_update_received", "sub_id": "s1", "service_id": "svc"})
    assert len(drain(owner)) == 2
    assert drain(other) == []


async def test_a_provider_side_event_is_routed_by_its_service_not_a_colliding_id(router_only):
    router = router_only
    router._ownership.claim("printer", "provider-app")
    consumer = attached(router, "consumer-app")
    provider = attached(router, "provider-app")
    # A peer picks a subscription id equal to one a local application holds.
    router.note_subscription("consumer-app", "s1")
    router.route(
        {"type": "service_subscribe_received", "sub_id": "s1", "service_id": "printer", "sender": "off1x"}
    )
    router.route(
        {
            "type": "service_subscription_changed",
            "sub_id": "s1",
            "role": "provider",
            "service_id": "printer",
            "state": "ended",
        }
    )
    assert [e["type"] for e in drain(provider)] == [
        "service_subscribe_received",
        "service_subscription_changed",
    ]
    assert drain(consumer) == []
    # And a provider-side end never settles the consumer's subscription.
    assert router.subscription_owner("s1") == "consumer-app"


@pytest.fixture
def router_only():
    return EventRouter(Policy(), ServiceOwnership())


# -- who may make each call ----------------------------------------------------


@pytest.mark.parametrize(
    "method, params",
    [
        ("services.accept_service_subscription", {"sub_id": "p1"}),
        ("services.reject_service_subscription", {"sub_id": "p1", "reason": "no"}),
        ("services.end_service_subscription", {"sub_id": "p1", "reason": "bye"}),
    ],
)
async def test_only_the_service_owner_decides_its_subscribers(world, method, params):
    dispatcher, router, ownership, services = world
    ownership.claim("printer", "app-a")
    services.subscribers = [subscriber("p1", "printer")]
    a = attached(router, "app-a")
    b = attached(router, "app-b")
    with pytest.raises(RpcError) as err:
        await dispatcher.call(b, method, params)
    assert variant(err) == "PermissionDenied"
    assert services.calls == []
    await dispatcher.call(a, method, params)
    assert services.calls == [(method.split(".", 1)[1], params)]


async def test_a_decision_on_an_unknown_subscription_is_left_to_the_engine(world):
    dispatcher, router, ownership, services = world
    ownership.claim("printer", "app-a")
    b = attached(router, "app-b")
    await dispatcher.call(b, "services.accept_service_subscription", {"sub_id": "nobody"})
    assert services.calls == [("accept_service_subscription", {"sub_id": "nobody"})]


@pytest.mark.parametrize(
    "method", ["services.unsubscribe_service", "services.resync_service_subscription"]
)
async def test_only_the_holder_acts_on_a_subscription(world, method):
    dispatcher, router, _, services = world
    a = attached(router, "app-a")
    b = attached(router, "app-b")
    router.note_subscription("app-a", "s1")
    with pytest.raises(RpcError) as err:
        await dispatcher.call(b, method, {"sub_id": "s1"})
    assert variant(err) == "PermissionDenied"
    assert services.calls == []
    await dispatcher.call(a, method, {"sub_id": "s1"})
    # An id this server never issued is the engine's to refuse.
    await dispatcher.call(b, method, {"sub_id": "elsewhere"})
    assert [c[1]["sub_id"] for c in services.calls] == ["s1", "elsewhere"]


@pytest.mark.parametrize(
    "method, params",
    [
        ("services.set_service_subscription_policy", {"service_id": "printer", "policy": {"kind": "AcceptAll"}}),
        ("services.publish_service_update", {"service_id": "printer", "topic": "t", "body": "{}"}),
        ("services.set_service_topic_checkpoint", {"service_id": "printer", "topic": "t", "body": "{}"}),
        ("services.clear_service_topic", {"service_id": "printer", "topic": "t"}),
    ],
)
async def test_only_the_service_owner_acts_on_its_topics(world, method, params):
    dispatcher, router, ownership, services = world
    ownership.claim("printer", "app-a")
    b = attached(router, "app-b")
    with pytest.raises(RpcError) as err:
        await dispatcher.call(b, method, params)
    assert variant(err) == "PermissionDenied"
    assert services.calls == []


async def test_each_application_lists_only_its_own(world):
    dispatcher, router, ownership, services = world
    ownership.claim("printer", "app-a")
    ownership.claim("scanner", "app-b")
    services.subscribers = [subscriber("p1", "printer"), subscriber("p2", "scanner"), subscriber("p3", "unowned")]
    services.subscriptions = [subscription("s1"), subscription("s2"), subscription("s3")]
    router.note_subscription("app-a", "s1")
    router.note_subscription("app-b", "s2")
    a = attached(router, "app-a")
    listed = await dispatcher.call(a, "services.service_subscribers", {})
    assert [s["sub_id"] for s in listed] == ["p1", "p3"]
    held = await dispatcher.call(a, "services.service_subscriptions", {})
    assert [s["sub_id"] for s in held] == ["s1"]
    assert held[0]["state"] == "Active" and held[0]["mode"] == "Log"


# -- the types on the wire -------------------------------------------------------


async def test_a_policy_names_its_kind_and_nothing_else(world):
    dispatcher, router, _, services = world
    a = attached(router, "app-a")
    await dispatcher.call(
        a, "services.set_service_subscription_policy", {"service_id": "svc", "policy": {"kind": "AcceptAll"}}
    )
    (_, args), = services.calls
    policy = args["policy"]
    assert policy.kind == generated.SubscriptionPolicyKind.ACCEPT_ALL
    assert policy.allow_peers == [] and policy.allow_groups == []
    assert policy.rules_fallback == generated.SubscriptionRulesFallback.ASK


async def test_a_publish_report_crosses_as_an_object(world):
    dispatcher, router, _, services = world
    a = attached(router, "app-a")
    report = await dispatcher.call(
        a, "services.publish_service_update", {"service_id": "svc", "topic": "t", "key": "k", "body": "1"}
    )
    assert report == {"subscribers": 3, "sent_live": 1, "queued": 1, "coalesced": 0, "failed": 1}
    assert services.calls[0][1]["key"] == "k"
