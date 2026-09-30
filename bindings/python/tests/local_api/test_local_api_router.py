"""The routing rules and the hold, against the router alone."""

from __future__ import annotations

import json

import pytest

from offline_protocol_sdk.local_api import (
    HELD_TAGS,
    HOLD_CAPACITY,
    EventRouter,
    Policy,
    ServiceOwnership,
    Session,
)


def drain(session: Session) -> list[dict]:
    """Everything queued on a session, as the event objects."""
    out = []
    while not session._queue.empty():
        item = session._queue.get_nowait()
        out.append(json.loads(item)["params"])
    return out


def attached(router: EventRouter, app_id: str) -> Session:
    session = Session("unix")
    session.app_id = app_id
    router.attach(session)
    return session


@pytest.fixture
def router():
    return EventRouter(Policy(), ServiceOwnership())


async def test_a_stamped_event_reaches_only_its_application(router):
    notes = attached(router, "notes")
    other = attached(router, "other")
    router.route({"type": "message_received", "message_id": "m1", "app_id": "notes"})
    assert [e["message_id"] for e in drain(notes)] == ["m1"]
    assert drain(other) == []


async def test_a_stamped_event_with_no_client_is_held_and_replayed_in_order(router):
    other = attached(router, "other")
    for i in range(3):
        router.route({"type": "message_received", "message_id": f"m{i}", "app_id": "notes"})
    router.route({"type": "file_received", "file_id": "f1", "app_id": "notes"})
    assert drain(other) == []
    assert router.held_count("notes") == 4
    late = Session("unix")
    late.app_id = "notes"
    held = router.attach(late)
    assert [e.get("message_id") or e.get("file_id") for e in held] == ["m0", "m1", "m2", "f1"]
    assert router.held_count("notes") == 0
    # A second client of the same application gets nothing from the hold.
    second = attached(router, "notes")
    assert router.attach(second) == []


async def test_the_hold_drops_the_oldest_past_its_capacity(router):
    dropped = []
    router = EventRouter(Policy(), ServiceOwnership(), on_drop=lambda app, e: dropped.append(e["message_id"]))
    for i in range(HOLD_CAPACITY + 2):
        router.route({"type": "message_received", "message_id": f"m{i}", "app_id": "notes"})
    assert router.held_count("notes") == HOLD_CAPACITY
    assert dropped == ["m0", "m1"]
    assert router.dropped == 2
    late = Session("unix")
    late.app_id = "notes"
    held = router.attach(late)
    assert held[0]["message_id"] == "m2"
    assert held[-1]["message_id"] == f"m{HOLD_CAPACITY + 1}"


async def test_only_the_stamped_inbound_tags_are_held(router):
    assert HELD_TAGS == {"message_received", "file_received", "media_resend_required"}
    # No client at all: a broadcast event is dropped, not held.
    router.route({"type": "neighbor_discovered", "peer_id": "p", "transport": "BLE"})
    router.route({"type": "message_decryption_failed", "message_id": "x", "sender": "s"})
    late = attached(router, "notes")
    assert router.attach(late) == []


async def test_a_stamped_tag_with_no_app_id_is_broadcast_not_held(router, caplog):
    notes = attached(router, "notes")
    other = attached(router, "other")
    with caplog.at_level("INFO", logger="offline_protocol_sdk.local_api.mux"):
        router.route({"type": "file_received", "file_id": "f", "app_id": None})
    assert [e["file_id"] for e in drain(notes)] == ["f"]
    assert [e["file_id"] for e in drain(other)] == ["f"]
    assert router.held_count("notes") == 0
    assert any("carries no app_id" in r.message for r in caplog.records)


async def test_an_event_emitted_inside_a_call_belongs_to_the_caller(router):
    notes = attached(router, "notes")
    other = attached(router, "other")
    router.current_caller = notes
    router.route({"type": "message_sent", "message_id": "m9", "sender": "a", "recipient": "b"}, in_call=True)
    router.current_caller = None
    assert [e["type"] for e in drain(notes)] == ["message_sent"]
    assert drain(other) == []
    # The id it named is now the caller's: a later event correlates.
    router.route({"type": "message_delivered", "message_id": "m9"})
    assert [e["type"] for e in drain(notes)] == ["message_delivered"]
    assert drain(other) == []


async def test_results_the_server_handed_out_correlate_later_events(router):
    notes = attached(router, "notes")
    other = attached(router, "other")
    router.note_ids("notes", ["m1"])
    router.route({"type": "message_failed", "message_id": "m1", "reason": "x", "retry_count": 1})
    router.route({"type": "file_progress", "file_id": "unknown", "chunks_sent": 1})
    assert [e["type"] for e in drain(notes)] == ["message_failed", "file_progress"]
    assert [e["type"] for e in drain(other)] == ["file_progress"]


async def test_a_service_request_reaches_the_owning_application(router):
    ownership = ServiceOwnership()
    router = EventRouter(Policy(), ownership)
    notes = attached(router, "notes")
    other = attached(router, "other")
    ownership.claim("svc", "notes")
    router.route({"type": "service_request_received", "request_id": "r", "service_id": "svc"})
    router.route({"type": "service_request_received", "request_id": "r2", "service_id": "nobodys"})
    assert [e["request_id"] for e in drain(notes)] == ["r", "r2"]
    assert [e["request_id"] for e in drain(other)] == ["r2"]


async def test_the_subscription_filter_narrows_delivery_without_changing_routing(router):
    notes = attached(router, "notes")
    notes.subscribe(["message_delivered"])
    router.route({"type": "message_received", "message_id": "m1", "app_id": "notes"})
    router.route({"type": "neighbor_lost", "peer_id": "p"})
    assert drain(notes) == []
    notes.subscribe("all")
    router.route({"type": "neighbor_lost", "peer_id": "p"})
    assert [e["type"] for e in drain(notes)] == ["neighbor_lost"]
    notes.unsubscribe("all")
    router.route({"type": "neighbor_lost", "peer_id": "p"})
    assert drain(notes) == []


async def test_document_events_are_filtered_by_the_space_allow_list():
    policy = Policy(spaces={"notes": ["notes-*"]})
    router = EventRouter(policy, ServiceOwnership())
    notes = attached(router, "notes")
    other = attached(router, "other")
    router.route({"type": "data_changed", "space_id": "notes-1", "doc_id": "d", "delta_bytes": 1})
    router.route({"type": "data_changed", "space_id": "mail-1", "doc_id": "d", "delta_bytes": 1})
    assert [e["space_id"] for e in drain(notes)] == ["notes-1"]
    assert [e["space_id"] for e in drain(other)] == ["notes-1", "mail-1"]


async def test_detach_stops_delivery(router):
    notes = attached(router, "notes")
    router.detach(notes)
    router.route({"type": "message_received", "message_id": "m1", "app_id": "notes"})
    assert drain(notes) == []
    assert router.held_count("notes") == 1


async def test_an_event_from_the_run_loop_during_a_call_is_not_the_callers(router):
    notes = attached(router, "notes")
    other = attached(router, "other")
    router.current_caller = notes
    # Emitted on the loop thread by `process()` while notes' call is in
    # flight on the executor: never the caller's. An event naming no
    # identifier broadcasts at once; one naming an identifier nobody owns
    # is parked until the call's result is recorded, then broadcast, and
    # the id it names stays unknown.
    router.route({"type": "message_delivered", "message_id": "not-ours"})
    router.route({"type": "neighbor_lost", "peer_id": "p"})
    assert [e["type"] for e in drain(notes)] == ["neighbor_lost"]
    assert [e["type"] for e in drain(other)] == ["neighbor_lost"]
    assert router.parked_count() == 1
    router.current_caller = None
    router.flush_parked()
    assert [e["type"] for e in drain(notes)] == ["message_delivered"]
    assert [e["type"] for e in drain(other)] == ["message_delivered"]
    assert not router.knows("not-ours") and router.parked_count() == 0


async def test_issued_identifiers_are_bounded_oldest_first(router):
    from offline_protocol_sdk.local_api.mux import ISSUED_CAPACITY

    notes = attached(router, "notes")
    other = attached(router, "other")
    router.note_ids("notes", [f"id{i}" for i in range(ISSUED_CAPACITY + 5)])
    assert router.issued_count() == ISSUED_CAPACITY
    assert not router.knows("id0") and not router.knows("id4")
    assert router.knows("id5") and router.knows(f"id{ISSUED_CAPACITY + 4}")
    # An evicted id is unknown, so its event is broadcast, as the chapter says.
    router.route({"type": "message_failed", "message_id": "id0", "reason": "x", "retry_count": 1})
    router.route({"type": "message_failed", "message_id": "id5", "reason": "x", "retry_count": 1})
    assert [e["message_id"] for e in drain(notes)] == ["id0", "id5"]
    assert [e["message_id"] for e in drain(other)] == ["id0"]


async def test_a_terminal_event_forgets_its_identifier_after_routing_it(router):
    notes = attached(router, "notes")
    other = attached(router, "other")
    router.note_ids("notes", ["m1", "f1"])
    router.route({"type": "file_progress", "file_id": "f1", "chunks_sent": 1})
    assert router.knows("f1")
    router.route({"type": "message_delivered", "message_id": "m1"})
    router.route({"type": "media_sent", "file_id": "f1", "recipient": "r"})
    assert [e["type"] for e in drain(notes)] == ["file_progress", "message_delivered", "media_sent"]
    assert drain(other) == []
    assert not router.knows("m1") and not router.knows("f1")
    # Anything later naming the id is broadcast.
    router.route({"type": "message_failed", "message_id": "m1", "reason": "x", "retry_count": 1})
    assert [e["type"] for e in drain(other)] == ["message_failed"]


async def test_a_loop_event_naming_the_calls_own_id_waits_for_the_id_and_reaches_only_the_caller(router):
    notes = attached(router, "notes")
    other = attached(router, "other")
    router.current_caller = notes  # the call is in flight on the executor
    # The transport takes the frame on a `process()` tick that lands between
    # the executor's completion and the wakeup that records the result: the
    # id is not yet known, and the event carries the content.
    router.route({"type": "message_sent", "message_id": "m1", "sender": "a", "recipient": "b", "content": "private"})
    assert drain(notes) == [] and drain(other) == []
    assert router.parked_count() == 1
    # The call completes: caller cleared, result recorded, parked flushed.
    router.current_caller = None
    router.note_ids("notes", ["m1"])
    router.flush_parked()
    assert [e["message_id"] for e in drain(notes)] == ["m1"]
    assert drain(other) == []
    assert router.parked_count() == 0
    # A later event for the id correlates as usual.
    router.route({"type": "message_delivered", "message_id": "m1"})
    assert [e["type"] for e in drain(notes)] == ["message_delivered"]
    assert drain(other) == []


async def test_parking_needs_a_call_in_flight_and_an_unknown_identifier(router):
    notes = attached(router, "notes")
    other = attached(router, "other")
    # No call in flight: an unknown identifier broadcasts at once.
    router.route({"type": "message_delivered", "message_id": "peer-1"})
    assert [e["type"] for e in drain(notes)] == ["message_delivered"]
    assert [e["type"] for e in drain(other)] == ["message_delivered"]
    # A call in flight, but the identifier is known: delivered at once.
    router.note_ids("other", ["o1"])
    router.current_caller = notes
    router.route({"type": "message_delivered", "message_id": "o1"})
    assert drain(other) == [{"type": "message_delivered", "message_id": "o1"}]
    assert drain(notes) == [] and router.parked_count() == 0
    # A call in flight and a stamped event: routed by its id, never parked.
    router.route({"type": "message_received", "message_id": "x", "app_id": "other"})
    assert [e["message_id"] for e in drain(other)] == ["x"]
    assert router.parked_count() == 0
    router.current_caller = None


async def test_a_failed_call_flushes_what_it_parked_by_the_ordinary_rules(router):
    notes = attached(router, "notes")
    other = attached(router, "other")
    router.current_caller = notes
    router.route({"type": "message_delivered", "message_id": "peer-2"})
    assert router.parked_count() == 1
    # The call failed: nothing issued, so the event broadcasts on the flush.
    router.current_caller = None
    router.flush_parked()
    assert [e["message_id"] for e in drain(notes)] == ["peer-2"]
    assert [e["message_id"] for e in drain(other)] == ["peer-2"]
