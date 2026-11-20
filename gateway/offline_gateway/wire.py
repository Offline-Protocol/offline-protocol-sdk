"""The device-facing wire: newline-delimited JSON, as the gateway contract
specifies it (``docs/spec/gateway-contract.md``, "Gateway-daemon contract v1").

Everything here is a function of its arguments, so the frame shapes can be
tested without a socket. The daemon (``gateway.py``) owns connections and
state.

The bounds below mirror the reference clients' own
(``bindings/python/offline_protocol_sdk/gateway_attach_policy.py``), because a
gateway that emits what a client refuses is not speaking the contract. They
are not hand-mirrored constants of the SDK's (C5): a daemon may choose
tighter ones, and the contract states each as a rule rather than a number.
"""

from __future__ import annotations

import base64
import binascii
import json
import re
from typing import Any

#: The contract version this daemon speaks, sent on ``Challenge``.
PROTOCOL_VERSION = 1

#: Bytes of challenge minted per connection. The reference clients sign
#: nothing else.
CHALLENGE_LENGTH = 32

#: Longest line this daemon reads or writes. The contract says a gateway
#: MUST NOT emit a longer line than the clients accept, and the reference
#: clients abandon a connection past 1 MiB. Reading is held to the same bound:
#: a line past it cannot be resynchronised, so the connection goes.
MAX_LINE_BYTES = 1 << 20

#: Peers answered per ``CheckPresence``; the rest of a longer list is ignored.
MAX_PRESENCE_PEERS = 64

#: Longest address this daemon accepts anywhere. A real one is 44
#: characters; the bound keeps a hostile string out of logs and maps.
MAX_ADDRESS_BYTES = 128

#: What a client-chosen message id must look like to be echoed verbatim.
#: The shape both reference implementations use; a UUID satisfies it.
MESSAGE_ID_PATTERN = re.compile(r"\A[A-Za-z0-9._-]{1,64}\Z")

#: The verdict prefix the SDK classifier matches. It parks the message,
#: offers it to the mesh and starts the escalating probe. Text after it is
#: for human logs only.
RECIPIENT_UNREACHABLE = "recipient_unreachable"

# Plain failures: the SDK retries on each and reads nothing else in the text.
ATTACH_REQUIRED = "attach_required"
BUDGET_EXCEEDED = "budget_exceeded"
FRAME_TOO_LARGE = "frame_too_large"
MALFORMED_REQUEST = "malformed_request"
BACKBONE_TIMEOUT = "backbone_timeout"


def encode(frame: dict[str, Any]) -> bytes:
    """One frame as one line. Compact, ASCII-only JSON, so a line's length in
    bytes is its length in characters and no client meets a surrogate."""
    return json.dumps(frame, separators=(",", ":"), ensure_ascii=True).encode("ascii") + b"\n"


def decode(line: bytes) -> dict[str, Any] | None:
    """A received line as a frame, or ``None`` for anything that is not a JSON
    object carrying a string ``type``.

    ``None`` is ignored by the caller, never answered with a close: the
    contract's unknown-type tolerance is what lets either side add messages
    without a flag day, and a line that does not parse is the same case from
    a newer peer's point of view.
    """
    try:
        frame = json.loads(line)
    except (UnicodeDecodeError, ValueError):
        return None
    if not isinstance(frame, dict) or not isinstance(frame.get("type"), str):
        return None
    return frame


def b64decode(value: Any) -> bytes | None:
    """Strict standard base64, or ``None``."""
    if not isinstance(value, str):
        return None
    try:
        return base64.b64decode(value, validate=True)
    except (binascii.Error, ValueError):
        return None


def b64encode(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii")


def address_field(value: Any) -> str | None:
    """An address-shaped string, or ``None``.

    Shape only: what makes an address mean something is the attach proof, and
    a recipient is routed by exact string match against addresses that were
    proved. This only keeps arbitrary strings out of the routing tables.
    """
    if not isinstance(value, str) or not value.startswith("off1"):
        return None
    if len(value.encode("utf-8", "surrogatepass")) > MAX_ADDRESS_BYTES:
        return None
    if not value.isascii() or not value.isalnum():
        return None
    return value


def message_id_field(value: Any) -> str | None:
    """A client-chosen id this daemon will echo verbatim, or ``None`` when it
    must mint one instead."""
    if isinstance(value, str) and MESSAGE_ID_PATTERN.match(value):
        return value
    return None


def content_field(frame: dict[str, Any]) -> bytes | None:
    """The frame's payload bytes. ``encoding`` absent means UTF-8 text; the
    reference clients always send ``base64``."""
    content = frame.get("content")
    encoding = frame.get("encoding")
    if encoding is None:
        return content.encode("utf-8") if isinstance(content, str) else None
    if encoding == "base64":
        return b64decode(content)
    return None


# -- Frames this daemon writes ------------------------------------------------


def challenge(nonce: bytes) -> dict[str, Any]:
    return {"type": "Challenge", "challenge": b64encode(nonce), "protocol_version": PROTOCOL_VERSION}


def address_declared(address: str) -> dict[str, Any]:
    return {"type": "AddressDeclared", "address": address}


def address_error(reason: str) -> dict[str, Any]:
    return {"type": "AddressError", "reason": reason}


def capabilities(tokens: list[str]) -> dict[str, Any]:
    return {"type": "Capabilities", "tokens": list(tokens)}


def status_update(status: str) -> dict[str, Any]:
    return {"type": "StatusUpdate", "status": status}


def message_sent(message_id: str, recipient: str | None) -> dict[str, Any]:
    # ``pushed`` and ``stored`` default to false when absent, and this daemon
    # implements neither, so it omits both rather than asserting false.
    frame: dict[str, Any] = {"type": "MessageSent", "message_id": message_id}
    if recipient is not None:
        frame["recipient"] = recipient
    return frame


def delivery_error(message_id: str, recipient: str | None, reason: str) -> dict[str, Any]:
    frame: dict[str, Any] = {"type": "DeliveryError", "message_id": message_id, "reason": reason}
    if recipient is not None:
        frame["recipient"] = recipient
    return frame


def message_received(
    sender: str, content: bytes, message_id: str | None, reply_to_msg: str | None
) -> dict[str, Any]:
    frame: dict[str, Any] = {
        "type": "MessageReceived",
        "sender": sender,
        "content": b64encode(content),
        "encoding": "base64",
    }
    if message_id is not None:
        frame["message_id"] = message_id
    if reply_to_msg is not None:
        frame["reply_to_msg"] = reply_to_msg
    return frame


def presence_status(peer: str, online: bool, last_seen_ms: int | None) -> dict[str, Any]:
    frame: dict[str, Any] = {"type": "PresenceStatus", "peer": peer, "online": online}
    if last_seen_ms is not None:
        frame["last_seen_ms"] = last_seen_ms
    return frame
