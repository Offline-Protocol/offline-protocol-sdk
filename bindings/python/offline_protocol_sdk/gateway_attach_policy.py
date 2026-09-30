"""The gateway daemon contract's decisions and frame shapes, with no socket
and no FFI, so they can be unit-tested.

A port of ``GatewayAttachPolicy.swift`` and ``GatewayAttachPolicy.kt``. The
manager (``gateway_manager.py``) owns the socket, the tasks and the
lifecycle; everything here is a function of its arguments. See
``docs/spec/gateway-contract.md``.

**What is deliberately not here: the signed proof.** The bytes a device
signs to attach are built and signed in the core, behind
``gateway_address_declaration(challenge)``. The relay's equivalent had to
live on the binding side because it commits the relay's account name, which
only a bridge knows. This one commits our own address, so it exists once,
where the conformance vectors can pin it, and a Rust guard refuses the
signing domain's name in this file. What is left here is the framing around
it.

The numbers below are hand-mirrored in three languages with no compiler
between them (C5). ``test_gateway_attach_policy.py`` pins each as a literal,
and the Rust guard ``gateway_manager_constants_match_across_both_bridges``
reads this file beside the Swift and Kotlin policies and holds the verdict
timeout under the core's own expiry.
"""

from __future__ import annotations

import base64
import binascii
import json
import re
from dataclasses import dataclass
from enum import Enum
from typing import Any

# -- Wire constants ----------------------------------------------------------

#: The contract version this client speaks, sent on ``Identify``.
PROTOCOL_VERSION = 1

#: Bytes of challenge the gateway mints per connection. A declaration is not
#: attempted for anything else: the core refuses to sign it, and finding that
#: out from a raised error is worse than not asking.
CHALLENGE_LENGTH = 32

#: How long the whole handshake may take, in seconds, from ``Identify`` to
#: ``StatusUpdate(connected)``.
#:
#: Far shorter than the 60 s connection timeout, and deliberately so: TCP to
#: the daemon is a LAN hop, and a gateway that has accepted the socket but
#: not finished the handshake is not slow, it is broken or wedged. Waiting a
#: minute to find that out means a minute of a carrier the selector has been
#: told nothing about.
ATTACH_TIMEOUT = 10.0

#: How long, in seconds, a submitted frame may go without a verdict before
#: this client treats the gateway's silence as a failure.
#:
#: **Must stay below the core's 120 s pending-confirmation timeout**, which
#: is the other clock on the same frame. If this were the longer of the two,
#: the core would expire the frame first and count it a failure, and the
#: verdict arriving afterwards would confirm or fail a message id the core
#: had already settled. The Rust guard pins both this number and that
#: relationship.
VERDICT_TIMEOUT = 60.0

#: Frames submitted but not yet answered. The gateway answers every
#: submission, so this bounds nothing but memory and the size of the loss
#: when a connection dies; it is also roughly what the core's own session
#: bootstrap bursts, so a smaller number would throttle the one case that
#: matters most.
MAX_IN_FLIGHT = 8

#: Longest line this client will assemble before abandoning the stream.
#:
#: A frame at the gateway's cap arrives base64-encoded, so 4/3 of its size
#: plus the JSON around it, and the buffer has to hold the largest one the
#: gateway can legitimately send. Past this the stream is not
#: resynchronisable: the rest of the over-long line would be read as a fresh
#: one, so the connection goes rather than the line.
MAX_LINE_BYTES = 1 << 20

#: Capability bounds, matching the relay's, which is what the contract points
#: at rather than inventing a second pair.
MAX_CAPABILITY_TOKENS = 64
MAX_CAPABILITY_TOKEN_BYTES = 128

#: Peers a gateway answers per ``CheckPresence``. Asking about more only
#: guarantees silence for the ones past the cap.
MAX_PRESENCE_PEERS = 64

#: Longest ``AddressDeclared`` echo a manager hands to the core. An address
#: is 44 characters; the bound is what keeps a hostile echo, which may be as
#: long as a line, out of the core's log and security event.
MAX_ADDRESS_BYTES = 128

#: Longest remote-chosen reason text a diagnostic carries. The core bounds
#: what it logs the same way; a diagnostic is a second log.
MAX_DIAGNOSTIC_REASON_CHARS = 256

# -- The core's failure-reason vocabulary ------------------------------------
#
# Exact literals the core classifies on (``SEND_FAIL_REASON_*`` in the engine).
# ``recipient_unreachable`` is matched by prefix and the gateway's own text
# may follow it; the other three are matched exactly and carry nothing.

#: The verdict prefix that parks a message and offers it to the mesh.
RECIPIENT_UNREACHABLE = "recipient_unreachable"
#: ``MessageSent { pushed: true }``: accepted, handed to a device push, not
#: delivered to a session. Parks a plain DM; never a failure.
RELAY_PUSHED = "relay_pushed"
#: ``DeliveryError { stored: true }``: the socket write missed but the
#: gateway's mailbox holds the frame and re-sends it on the recipient's next
#: attach. The unreachable verdict, parked without a reachability probe.
RELAY_STORED = "relay_stored"
#: ``MessageSent { pushed: true, stored: true }``: both of the above.
RELAY_PUSHED_STORED = "relay_pushed_stored"
#: What a frame the gateway never answered is failed with. The core
#: classifies it to its generic transport failure and retries.
GATEWAY_SILENT = f"gateway_silent: no verdict within {int(VERDICT_TIMEOUT)}s"


# -- Attach ------------------------------------------------------------------


class SkipReason:
    """Why a declaration was not attempted. Reported as a diagnostic; the
    carrier stays unavailable either way."""

    ADDRESS_UNAVAILABLE = "address_unavailable"
    CHALLENGE_ABSENT = "challenge_absent"
    CHALLENGE_MALFORMED = "challenge_malformed"
    CHALLENGE_WRONG_SIZE = "challenge_wrong_size"
    SIGNING_FAILED = "signing_failed"
    FRAME_UNSERIALIZABLE = "frame_unserializable"


@dataclass(frozen=True)
class Declare:
    """A ``Challenge`` frame yielded a challenge worth signing."""

    challenge: bytes


@dataclass(frozen=True)
class Skip:
    """A ``Challenge`` frame that cannot be answered, and why."""

    reason: str


class BindingOutcome(Enum):
    """What a gateway's ``AddressDeclared`` echo says about this device.

    The same three answers the core reports on, decided again here because
    the two act on different things: the core emits the security warning,
    and the manager decides whether this carrier can be offered at all.
    """

    #: The gateway bound the address we declared. The session is proven.
    BOUND = "bound"
    #: It bound something else. A security event, not a retry.
    MISMATCH = "mismatch"
    #: We hold no address to compare against, so nothing here was ever
    #: declared by us.
    UNKNOWN_LOCAL = "unknown_local"


def decode_challenge(frame: dict[str, Any]) -> Declare | Skip:
    """Reads the challenge out of a ``Challenge`` frame.

    The size is checked here as well as in the core because the two refusals
    mean different things to the reader: this one says the gateway is not
    speaking the contract, and the core's says something asked it to sign a
    payload it should not.
    """
    encoded = frame.get("challenge")
    if not isinstance(encoded, str) or not encoded:
        return Skip(SkipReason.CHALLENGE_ABSENT)
    try:
        challenge = base64.b64decode(encoded, validate=True)
    except (binascii.Error, ValueError):
        return Skip(SkipReason.CHALLENGE_MALFORMED)
    if len(challenge) != CHALLENGE_LENGTH:
        return Skip(SkipReason.CHALLENGE_WRONG_SIZE)
    return Declare(challenge)


def binding_outcome(declared: str, local: str | None) -> BindingOutcome:
    """Compares the gateway's echo with this device's own address."""
    if not local:
        return BindingOutcome.UNKNOWN_LOCAL
    return BindingOutcome.BOUND if declared == local else BindingOutcome.MISMATCH


def capability_tokens(frame: dict[str, Any]) -> list[str]:
    """The capability tokens worth storing, bounded on the way in.

    Oversized tokens are dropped **before** the count is applied, so a
    gateway cannot pad its list to evict the tokens that matter.
    """
    raw = frame.get("tokens")
    if not isinstance(raw, list):
        return []
    kept = [
        token
        for token in raw
        if isinstance(token, str)
        and token
        and len(token.encode("utf-8")) <= MAX_CAPABILITY_TOKEN_BYTES
    ]
    return kept[:MAX_CAPABILITY_TOKENS]


# -- Message ids -------------------------------------------------------------

_MESSAGE_ID_RE = re.compile(r"[A-Za-z0-9._-]{1,64}")


def sanitize_message_id(candidate: str | None) -> str | None:
    """The gateway's own rule for a client-supplied id: 1 to 64 characters
    of ``[A-Za-z0-9._-]``.

    Applied before sending, not after: an id the gateway would refuse is
    replaced *there* by one it mints, and the verdict then comes back under
    a name nothing here is waiting on. Message ids are UUIDs, which pass;
    this is what keeps that from being an assumption.
    """
    if not candidate:
        return None
    return candidate if _MESSAGE_ID_RE.fullmatch(candidate) else None


# -- Frames this client sends ------------------------------------------------


def _serialize(frame: dict[str, Any]) -> str:
    return json.dumps(frame, separators=(",", ":"))


def identify_json(device_id: str) -> str:
    return _serialize(
        {
            "type": "Identify",
            "device_id": device_id,
            "protocol_version": PROTOCOL_VERSION,
        }
    )


def declaration_json(address: str, public_key: bytes, signature: bytes) -> str:
    return _serialize(
        {
            "type": "DeclareAddress",
            "address": address,
            "public_key": base64.b64encode(public_key).decode("ascii"),
            "signature": base64.b64encode(signature).decode("ascii"),
        }
    )


def send_message_json(
    message_id: str, recipient: str, content: str, reply_to_msg: str | None
) -> str:
    frame: dict[str, Any] = {
        "type": "SendMessage",
        "recipient": recipient,
        "content": content,
        "encoding": "base64",
        "message_id": message_id,
    }
    if reply_to_msg:
        frame["reply_to_msg"] = reply_to_msg
    return _serialize(frame)


def check_presence_json(peers: list[str]) -> str | None:
    """One frame for the whole batch, which is the shape the contract takes
    and the opposite of the relay's one-peer-per-frame ``CheckPresence``."""
    asked = list(peers[:MAX_PRESENCE_PEERS])
    if not asked:
        return None
    return _serialize({"type": "CheckPresence", "peers": asked})


# -- Frames this client reads ------------------------------------------------


@dataclass(frozen=True)
class Verdict:
    """A verdict: the id it settles, and the reason if it is a refusal."""

    message_id: str
    #: ``None`` for ``MessageSent``, the gateway's own text for
    #: ``DeliveryError``. Passed to the core verbatim: the classifier matches
    #: the ``recipient_unreachable`` prefix and discards the rest, so nothing
    #: here needs to understand it.
    reason: str | None
    recipient: str | None
    #: ``MessageSent { pushed: true }``: accepted, but handed to a device
    #: push rather than a session. Absent reads as ``False``, never unknown.
    pushed: bool = False
    #: The gateway's mailbox holds this one frame and will re-send it. A
    #: statement about the named id alone, never about the recipient.
    stored: bool = False

    @property
    def sent(self) -> bool:
        return self.reason is None


def parse_verdict(frame: dict[str, Any], frame_type: str) -> Verdict | None:
    message_id = frame.get("message_id")
    if not isinstance(message_id, str) or not message_id:
        # Nothing to settle. The gateway mints an id for a submission that
        # carried none, but this client always sends one, so a verdict
        # without an id is not ours to act on.
        return None
    recipient = frame.get("recipient")
    if not isinstance(recipient, str) or not recipient:
        recipient = None
    # ``is True``: the contract makes both optional booleans that default to
    # false when absent, and a non-boolean is not a claim the gateway made.
    pushed = frame.get("pushed") is True
    stored = frame.get("stored") is True
    if frame_type == "MessageSent":
        return Verdict(message_id, None, recipient, pushed=pushed, stored=stored)
    reason = frame.get("reason")
    if not isinstance(reason, str) or not reason:
        reason = "DeliveryError"
    return Verdict(message_id, reason, recipient, pushed=False, stored=stored)


@dataclass(frozen=True)
class VerdictReport:
    """What a settled verdict tells the core, decided here so the mapping
    from the wire's two flags to the core's four tokens is testable without
    a socket.

    ``reason`` is ``None`` for a plain ``MessageSent``, which the core hears
    as a confirmation; every other verdict is a reported failure under
    ``reason``. ``watch_recipient`` says the recipient is worth a presence
    watch and an offline presence fact: the gateway pushes a
    ``PresenceStatus`` when a watched peer attaches, and that answer is what
    un-parks the message this verdict just parked.
    """

    reason: str | None
    watch_recipient: bool

    @property
    def confirmed(self) -> bool:
        return self.reason is None


def verdict_report(verdict: Verdict) -> VerdictReport:
    """The core's token for a verdict.

    The relay client's own mapping, so a gateway that implements a mailbox
    or a push is read the way the relay is:

    * ``MessageSent`` with neither flag confirms the frame.
    * ``MessageSent { pushed }`` is ``relay_pushed``, or
      ``relay_pushed_stored`` with ``stored`` as well: a park, never a
      failure, so a connection request is not fast-failed for a frame the
      push may have delivered.
    * ``DeliveryError { stored }`` is ``relay_stored``: the unreachable
      verdict for the one frame the gateway holds, parked without the
      reachability probe its own redelivery makes pointless.
    * Any other ``DeliveryError`` carries the gateway's reason verbatim; the
      core classifies on the ``recipient_unreachable`` prefix and drops the
      rest at that boundary.
    """
    if verdict.sent:
        if not verdict.pushed:
            return VerdictReport(None, False)
        token = RELAY_PUSHED_STORED if verdict.stored else RELAY_PUSHED
        return VerdictReport(token, True)
    if verdict.stored:
        return VerdictReport(RELAY_STORED, True)
    reason = verdict.reason or "DeliveryError"
    return VerdictReport(reason, reason.startswith(RECIPIENT_UNREACHABLE))


@dataclass(frozen=True)
class PresenceAnswer:
    peer: str
    online: bool
    last_seen_ms: int | None


def parse_presence(frame: dict[str, Any]) -> PresenceAnswer | None:
    peer = frame.get("peer")
    if not isinstance(peer, str) or not peer:
        return None
    # A missing or non-boolean ``online`` is not readable as "offline": that
    # would manufacture a claim the gateway did not make.
    online = frame.get("online")
    if not isinstance(online, bool):
        return None
    raw_seen = frame.get("last_seen_ms")
    last_seen: int | None
    if isinstance(raw_seen, bool) or not isinstance(raw_seen, (int, float)):
        last_seen = None
    else:
        last_seen = int(raw_seen)
    return PresenceAnswer(peer, online, last_seen)


def bounded_reason(reason: str) -> str:
    """Remote-chosen text, cut to what a diagnostic may carry."""
    return reason[:MAX_DIAGNOSTIC_REASON_CHARS]
