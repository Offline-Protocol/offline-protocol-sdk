"""The verifier's commands. Each one prints JSON lines on stdout, a short
summary on stderr, and returns an exit status: 0 when what it waited for
happened, 2 when its time ran out first, 1 on a terminal failure (the engine
gave the message up, refused the call, or the service went away).

Events are read as the engine serialises them (``docs/spec/local-api.md``,
the event table): ``type`` is the tag, and a carrier is named by its
lowercase label (``wifiDirect``, ``ble``, ``internet``, ``reticulum``,
``nostr``). ``transport_switched`` is never read: the FFI layer emits it
with other names and for no Bluetooth LE edge, so it cannot say which
carrier a message took. ``message_delivered.transport`` can.
"""

from __future__ import annotations

import asyncio
import json
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import IO, Any, Callable

from websockets.exceptions import WebSocketException

from .client import DEFAULT_APP_ID, RpcFailure, VerifyClient

PASS = 0
FAILED = 1
TIMED_OUT = 2

#: Events about a message the sender is still waiting on: printed as status
#: and never the outcome. ``message_undeliverable`` is among them: it is the
#: relay's or gateway's verdict that the recipient is away right now, it
#: repeats while the message is parked, and the message settles only by
#: ``message_delivered`` or ``message_failed``.
STATUS_TAGS = frozenset({"message_sent", "message_deferred", "message_retrying", "message_undeliverable"})

#: Appended to the application id by the commands that only read state
#: (``state``, ``pair``); see :meth:`Target.open`.
OBSERVER_SUFFIX = ".observer"

#: How often ``pair`` asks for the session state.
PAIR_POLL_INTERVAL = 0.25

#: Receipts ``ping`` keeps for ids it has not been handed yet.
EARLY_CAPACITY = 256

#: How long ``watch`` waits before reconnecting to a service that went away.
WATCH_RECONNECT_DELAY = 0.5


@dataclass
class Target:
    """Where the service listens, and the application id to declare."""

    socket_path: str | None = None
    tcp_port: int | None = None
    token_file: str | None = None
    app_id: str = DEFAULT_APP_ID

    async def open(self, *, observer: bool = False) -> VerifyClient:
        """Connects as the application, or with ``observer`` under an id of
        its own. The service hands every message it held for an application
        to the first connection of that application, so a command that only
        reads state must not declare it: a ``state`` run on the recipient
        before ``await --until received`` would take the message and drop it."""
        return await VerifyClient.open(
            socket_path=self.socket_path,
            tcp_port=self.tcp_port,
            token_file=self.token_file,
            app_id=f"{self.app_id}{OBSERVER_SUFFIX}" if observer else self.app_id,
        )


@dataclass
class Output:
    """Where lines go; tests swap these for buffers."""

    out: IO[str] = field(default_factory=lambda: sys.stdout)
    err: IO[str] = field(default_factory=lambda: sys.stderr)

    def line(self, payload: dict[str, Any]) -> None:
        self.out.write(json.dumps(payload, separators=(",", ":")) + "\n")
        self.out.flush()

    def say(self, text: str) -> None:
        self.err.write(text + "\n")
        self.err.flush()


def _names(event: dict[str, Any], message_id: str) -> bool:
    return event.get("message_id") == message_id


async def state(target: Target, peers: list[str], output: Output) -> int:
    """What this device is and holds right now: its address, the carriers
    that are up, the queues, the relay counters, and
    the session state toward each of ``peers``."""
    client = await target.open(observer=True)
    try:
        report: dict[str, Any] = {
            "local_address": client.local_address,
            "active_transports": await client.call("get_active_transports"),
            "pending_ack_count": await client.call("get_pending_ack_count"),
            "retry_queue_size": await client.call("get_retry_queue_size"),
            "mesh_relay_stats": await client.call("get_mesh_relay_stats"),
            "sessions": {
                peer: await client.call("get_establishment_state", {"peer_id": peer}) for peer in peers
            },
        }
    finally:
        await client.close()
    output.line({"state": report})
    output.say(f"{report['local_address']}: transports {', '.join(report['active_transports']) or 'none'}")
    return PASS


async def send(
    target: Target,
    recipient: str,
    content: str,
    output: Output,
    *,
    wait: bool = False,
    timeout: float = 120.0,
) -> int:
    """Sends one message and prints its id. The id is what ``await`` takes.

    With ``wait`` it then waits for the receipt as ``await --until
    delivered`` does, on the same connection. That is the only race-free
    way to see the receipt of a message to a recipient that is reachable
    now: the connection is subscribed before ``send_message`` is called,
    whereas a separate ``await`` connects only after this one closed, and
    a receipt that fires in between reaches no client and is not held."""
    client = await target.open()
    try:
        message_id = await client.call(
            "send_message", {"recipient": recipient, "content": content, "priority": "Medium"}
        )
        output.line({"sent": {"message_id": message_id, "recipient": recipient}})
        output.say(f"sent {message_id} to {recipient}")
        if wait:
            return await _await_on(client, message_id, "delivered", timeout, output)
    finally:
        await client.close()
    return PASS


async def await_message(
    target: Target,
    message_id: str,
    until: str,
    timeout: float,
    output: Output,
) -> int:
    """Waits for one message's outcome.

    ``until="delivered"`` runs on the sender: passes on ``message_delivered``
    naming the id, fails on ``message_failed``, and prints every status event
    in between. ``until="received"`` runs on the recipient: passes on
    ``message_received`` naming the id, which a service holds for the
    application while no client of it is connected.

    Events are matched by the id they carry, never by the server's
    correlation, which knows only the ids its own clients were handed in
    this process. A sender's receipt that fires while no client is connected
    is not held: start this before the recipient can answer.
    """
    client = await target.open()
    try:
        return await _await_on(client, message_id, until, timeout, output)
    finally:
        await client.close()


async def _await_on(client: VerifyClient, message_id: str, until: str, timeout: float, output: Output) -> int:
    started = time.monotonic()
    if until == "received":
        settles = {"message_received"}
    else:
        settles = {"message_delivered", "message_failed"}

    def accept(event: dict[str, Any]) -> bool:
        return event.get("type") in settles and _names(event, message_id)

    def status(event: dict[str, Any]) -> None:
        if event.get("type") in STATUS_TAGS and _names(event, message_id):
            output.line({"status": event})

    try:
        event = await client.wait_for(accept, timeout, on_other=status)
    except TimeoutError:
        output.line({"result": "timeout", "message_id": message_id, "until": until})
        output.say(f"{message_id}: nothing after {timeout:g} s")
        return TIMED_OUT
    except ConnectionError as exc:
        output.line({"result": "failed", "message_id": message_id, "reason": str(exc)})
        output.say(f"{message_id}: {exc}")
        return FAILED
    elapsed = round(time.monotonic() - started, 3)
    if event["type"] == "message_failed":
        output.line({"result": "failed", "event": event, "elapsed_s": elapsed})
        output.say(f"{message_id}: failed, {event.get('reason')}")
        return FAILED
    output.line({"result": "pass", "event": event, "elapsed_s": elapsed})
    output.say(
        f"{message_id}: {event['type'].removeprefix('message_')} over {event.get('transport')}, "
        f"{event.get('hop_count')} hop(s), after {elapsed:g} s"
    )
    return PASS


async def pair(target: Target, peer: str, timeout: float, output: Output) -> int:
    """Waits until the session toward ``peer`` is confirmed.

    The engine forms it by itself once the two devices hear each other
    directly (``docs/state-machines/session-lifecycle.md``); this only waits.
    The automatic key exchange never crosses a hop, so two devices that will
    later reach each other only through a third must have met once."""
    client = await target.open(observer=True)
    loop = asyncio.get_running_loop()
    deadline = loop.time() + timeout
    last = None
    try:
        while True:
            last = await client.call("get_establishment_state", {"peer_id": peer})
            if last == "SessionConfirmed":
                output.line({"result": "pass", "peer": peer, "state": last})
                output.say(f"session with {peer} confirmed")
                return PASS
            if loop.time() >= deadline:
                output.line({"result": "timeout", "peer": peer, "state": last})
                output.say(f"no session with {peer} after {timeout:g} s (state {last})")
                return TIMED_OUT
            await asyncio.sleep(PAIR_POLL_INTERVAL)
    finally:
        await client.close()


async def ping(
    target: Target,
    recipient: str,
    every: float,
    count: int,
    timeout: float,
    output: Output,
) -> int:
    """Sends ``count`` messages ``every`` seconds and prints each outcome
    with the carrier it arrived over. Sends do not wait for earlier answers,
    so a carrier going away while some are in flight shows as those
    messages arriving late, over another carrier, rather than as a stall.
    Passes when every message is delivered within ``timeout`` of its send."""
    client = await target.open()
    loop = asyncio.get_running_loop()
    sent: dict[str, tuple[int, float]] = {}
    outcomes: dict[str, dict[str, Any]] = {}
    early: list[dict[str, Any]] = []

    def record(event: dict[str, Any]) -> bool:
        message_id = event.get("message_id")
        if event.get("type") not in ("message_delivered", "message_failed"):
            return False
        if message_id in sent and message_id not in outcomes:
            seq, at = sent[message_id]
            outcomes[message_id] = event
            output.line(
                {
                    "seq": seq,
                    "message_id": message_id,
                    "outcome": event["type"].removeprefix("message_"),
                    "transport": event.get("transport"),
                    "hop_count": event.get("hop_count"),
                    "latency_ms": event.get("latency_ms"),
                    "elapsed_s": round(loop.time() - at, 3),
                }
            )
            return True
        if message_id is not None and message_id not in sent:
            # The receipt beat ``send_message``'s own answer: kept until the
            # id is known. Bounded, since other clients' receipts land here too.
            early.append(event)
            del early[:-EARLY_CAPACITY]
        return False

    async def drain(until: float, settled_ends: bool) -> None:
        while not (settled_ends and len(outcomes) == len(sent)):
            remaining = until - loop.time()
            if remaining <= 0:
                return
            try:
                record(await client.next_event(remaining))
            except TimeoutError:
                return

    try:
        for seq in range(count):
            at = loop.time()
            message_id = await client.call(
                "send_message", {"recipient": recipient, "content": f"ping {seq}", "priority": "Medium"}
            )
            sent[message_id] = (seq, at)
            for event in [e for e in early if e.get("message_id") == message_id]:
                early.remove(event)
                record(event)
            if seq + 1 < count:
                await drain(at + every, settled_ends=False)
        await drain(max(at for _, at in sent.values()) + timeout, settled_ends=True)
    except ConnectionError as exc:
        output.say(f"ping: {exc}")
        return FAILED
    finally:
        await client.close()
    delivered = sum(1 for e in outcomes.values() if e["type"] == "message_delivered")
    failed = sum(1 for e in outcomes.values() if e["type"] == "message_failed")
    carriers = sorted({str(e.get("transport")) for e in outcomes.values() if e["type"] == "message_delivered"})
    status = PASS if delivered == count else FAILED if failed else TIMED_OUT
    result = {PASS: "pass", FAILED: "failed", TIMED_OUT: "timeout"}[status]
    output.line({"result": result, "sent": count, "delivered": delivered, "failed": failed, "carriers": carriers})
    output.say(f"ping: {delivered}/{count} delivered over {', '.join(carriers) or 'nothing'}")
    return status


async def watch(
    target: Target,
    log: Path | None,
    duration: float | None,
    output: Output,
    on_event: Callable[[dict[str, Any]], None] | None = None,
) -> int:
    """Prints every event, one JSON line each with the local receive time,
    and appends it to ``log`` when given. Reconnects when the service goes
    away and comes back, so one watcher spans a restart; events the engine
    emits while no client is connected are not seen, except the received
    messages the service holds for the application.

    Passes when ``duration`` runs out having been connected at some point,
    and fails when it never was: an empty watch log is otherwise
    indistinguishable from a quiet network."""
    loop = asyncio.get_running_loop()
    deadline = None if duration is None else loop.time() + duration
    handle = log.open("a", encoding="utf-8") if log is not None else None
    connected = False
    ever_connected = False
    waiting_said = False
    try:
        while deadline is None or loop.time() < deadline:
            try:
                client = await target.open()
            except (OSError, ConnectionError, RpcFailure, WebSocketException, asyncio.TimeoutError) as exc:
                # A service that is stopping or starting can refuse the
                # connection, drop it inside the handshake (a websockets
                # error, not an OSError) or close it during ``hello``; each
                # is the gap a restart leaves, so each is retried.
                if connected:
                    output.say(f"watch: service gone ({exc}); reconnecting")
                    connected = False
                elif not ever_connected and not waiting_said:
                    output.say(f"watch: waiting for the service ({exc or type(exc).__name__})")
                    waiting_said = True
                await asyncio.sleep(WATCH_RECONNECT_DELAY)
                continue
            connected = ever_connected = True
            output.say(f"watch: connected to {client.local_address}")
            try:
                while True:
                    remaining = None if deadline is None else deadline - loop.time()
                    if remaining is not None and remaining <= 0:
                        return PASS
                    try:
                        event = await client.next_event(remaining)
                    except TimeoutError:
                        return PASS
                    line = {"at_ms": int(time.time() * 1000), "event": event}
                    output.line(line)
                    if handle is not None:
                        handle.write(json.dumps(line, separators=(",", ":")) + "\n")
                        handle.flush()
                    if on_event is not None:
                        on_event(event)
            except ConnectionError:
                output.say("watch: service closed the connection; reconnecting")
                connected = False
            finally:
                await client.close()
        if not ever_connected:
            output.say("watch: never connected to the service")
            return FAILED
        return PASS
    finally:
        if handle is not None:
            handle.close()
