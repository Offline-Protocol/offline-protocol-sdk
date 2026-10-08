"""The envelope two HTTP fronts exchange, and its limits.

``docs/spec/http-front.md`` is the contract. Every limit, the carried-header
list and the error tokens are literals here and pinned as literals in
``tests/http_front/test_envelope.py`` (rule C5): a test that read them from
this module would agree with any edit.

The envelope is the content of an ordinary direct message, which the engine
seals with MLS. It is never sent on the service request path, whose bodies
are signed plaintext (threat model R9).
"""

from __future__ import annotations

import base64
import binascii
import json
import re
from dataclasses import dataclass, field
from typing import Any, Mapping
from urllib.parse import unquote

#: The application id every front declares and stamps on what it sends.
FRONT_APP_ID = "offline-protocol-http-front"
ENVELOPE_VERSION = 1

#: Raw body bytes, either direction. Base64 and the envelope keep a body this
#: size well under the engine's 256 KiB content limit.
MAX_BODY_BYTES = 128 * 1024
#: Carried header names plus values.
MAX_HEADER_BYTES = 8 * 1024
#: The path and query.
MAX_PATH_BYTES = 2048
DEFAULT_DEADLINE_SECONDS = 30
MAX_DEADLINE_SECONDS = 300
#: How far past its deadline a provider still serves a request: the two
#: hosts' clocks can disagree, notably after an offline power cycle.
CLOCK_SLACK_SECONDS = 60
#: The least time a provider gives a callback, whatever the deadline says.
MIN_CALLBACK_SECONDS = 5
#: Callbacks a provider runs at once. Any device with a session can send
#: requests, and each holds a connection to the callback for up to the
#: maximum deadline; past this a request is refused at once, not queued.
MAX_CONCURRENT_CALLBACKS = 32

#: Headers that cross, in either direction, besides ``x-*`` (below).
CARRIED_HEADERS = frozenset(
    {
        "accept",
        "accept-language",
        "authorization",
        "cache-control",
        "content-language",
        "content-type",
        "etag",
        "if-match",
        "if-none-match",
        "last-modified",
        "location",
    }
)
#: The front's own headers; never carried from one host to the other.
OWN_HEADER_PREFIX = "x-offline-protocol-"
#: ``x-*`` names a reverse proxy in front of a callback reads as the client's
#: address or the original target. The front calls the callback from
#: loopback, which such proxies trust most, so a requester that could set
#: these would choose the source a callback sees, or a path outside the
#: registered one.
FORWARDING_HEADER_PREFIX = "x-forwarded-"
FORWARDING_HEADERS = frozenset({"x-real-ip", "x-original-url", "x-rewrite-url"})

#: Error token -> HTTP status.
ERRORS: dict[str, int] = {
    "bad_host": 400,
    "bad_request": 400,
    "unknown_device": 404,
    "body_too_large": 413,
    "deadline_exceeded": 504,
    "recipient_unreachable": 502,
    "send_failed": 502,
    "not_connected": 503,
    "unauthorized": 401,
    "unknown_service": 404,
    "callback_failed": 502,
    "response_too_large": 502,
    "unsupported_version": 400,
}


class EnvelopeError(ValueError):
    """Content that is not an envelope this front can process."""


def carried(headers: Mapping[str, str] | Any) -> dict[str, str]:
    """The headers that cross, names in lower case.

    ``headers`` may hold a name more than once (an aiohttp ``CIMultiDict``);
    repeated values are joined with ``, `` as HTTP allows for list-valued
    headers.
    """
    out: dict[str, str] = {}
    items = headers.items() if hasattr(headers, "items") else headers
    for name, value in items:
        lower = name.lower()
        if lower.startswith((OWN_HEADER_PREFIX, FORWARDING_HEADER_PREFIX)) or lower in FORWARDING_HEADERS:
            continue
        if lower in CARRIED_HEADERS or lower.startswith("x-"):
            out[lower] = f"{out[lower]}, {value}" if lower in out else str(value)
    return out


def header_bytes(headers: Mapping[str, str]) -> int:
    return sum(len(k.encode()) + len(v.encode()) for k, v in headers.items())


#: An HTTP token (RFC 9110 section 5.6.2): a method or a header name.
TOKEN = re.compile(r"^[!#$%&'*+.^_`|~0-9A-Za-z-]+$")
#: A control character other than horizontal tab: CR and LF split a header,
#: and NUL ends one in some parsers.
_CONTROL = re.compile(r"[\x00-\x08\x0a-\x1f\x7f]")


def has_dot_segment(path: str) -> bool:
    """Whether the path part holds a ``.`` or ``..`` segment, raw or
    percent-encoded. The HTTP client normalizes both away before it writes
    the request, so ``/api/../admin`` and ``/api/%2e%2e/admin`` both reach
    ``/admin``: joined to a callback URL, such a path escapes the path the
    provider registered."""
    return any(unquote(segment) in (".", "..") for segment in path.split("?", 1)[0].split("/"))


@dataclass(frozen=True)
class Request:
    id: str
    service: str
    method: str
    path: str
    headers: dict[str, str] = field(default_factory=dict)
    body: bytes = b""
    deadline: int = 0
    version: int = ENVELOPE_VERSION

    def encode(self) -> str:
        doc: dict[str, Any] = {
            "op": "req",
            "v": self.version,
            "id": self.id,
            "svc": self.service,
            "m": self.method,
            "p": self.path,
            "h": self.headers,
            "dl": self.deadline,
        }
        if self.body:
            doc["b"] = base64.b64encode(self.body).decode("ascii")
        return json.dumps(doc, separators=(",", ":"))


@dataclass(frozen=True)
class Response:
    id: str
    status: int
    headers: dict[str, str] = field(default_factory=dict)
    body: bytes = b""
    error: str | None = None

    @classmethod
    def failure(cls, request_id: str, token: str) -> "Response":
        return cls(request_id, ERRORS[token], error=token)

    def encode(self) -> str:
        doc: dict[str, Any] = {"op": "resp", "v": ENVELOPE_VERSION, "id": self.id, "s": self.status}
        if self.headers:
            doc["h"] = self.headers
        if self.body:
            doc["b"] = base64.b64encode(self.body).decode("ascii")
        if self.error is not None:
            doc["e"] = self.error
        return json.dumps(doc, separators=(",", ":"))


def _str(doc: Mapping[str, Any], key: str) -> str:
    value = doc.get(key)
    if not isinstance(value, str):
        raise EnvelopeError(f"{key} must be a string")
    return value


def _headers(doc: Mapping[str, Any]) -> dict[str, str]:
    raw = doc.get("h", {})
    if not isinstance(raw, dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in raw.items()):
        raise EnvelopeError("h must be an object of strings")
    # The HTTP library refuses both at write time with a ValueError, on the
    # provider as the callback is called and on the requester as the answer
    # is written, where it drops the client's connection.
    for name, value in raw.items():
        if not TOKEN.fullmatch(name) or _CONTROL.search(value):
            raise EnvelopeError("h holds a name that is not a token or a value with a control character")
    headers = carried(raw)
    if header_bytes(headers) > MAX_HEADER_BYTES:
        raise EnvelopeError("headers over the limit")
    return headers


def _body(doc: Mapping[str, Any]) -> bytes:
    raw = doc.get("b", "")
    if not isinstance(raw, str):
        raise EnvelopeError("b must be a string")
    try:
        body = base64.b64decode(raw, validate=True)
    except (binascii.Error, ValueError):
        raise EnvelopeError("b is not base64") from None
    if len(body) > MAX_BODY_BYTES:
        raise EnvelopeError("body over the limit")
    return body


def decode(content: str) -> Request | Response:
    """Parses a message's content. Raises :class:`EnvelopeError` for
    anything that is not a front envelope; a request with another version
    comes back with that version for the caller to refuse."""
    try:
        doc = json.loads(content)
    except ValueError:
        raise EnvelopeError("not JSON") from None
    if not isinstance(doc, dict):
        raise EnvelopeError("not an object")
    op = doc.get("op")
    request_id = _str(doc, "id")
    version = doc.get("v")
    if not isinstance(version, int) or isinstance(version, bool):
        raise EnvelopeError("v must be an integer")
    if op == "req":
        if version != ENVELOPE_VERSION:
            return Request(request_id, "", "", "", version=version)
        path = _str(doc, "p")
        if not path.startswith("/") or len(path.encode()) > MAX_PATH_BYTES:
            raise EnvelopeError("p must be a path within the limit")
        if has_dot_segment(path) or _CONTROL.search(path):
            raise EnvelopeError("p must hold no dot segment and no control character")
        method = _str(doc, "m")
        if not TOKEN.fullmatch(method):
            raise EnvelopeError("m must be an HTTP token")
        deadline = doc.get("dl")
        if not isinstance(deadline, int) or isinstance(deadline, bool):
            raise EnvelopeError("dl must be an integer")
        return Request(
            request_id,
            _str(doc, "svc"),
            method.upper(),
            path,
            _headers(doc),
            _body(doc),
            deadline,
        )
    if op == "resp":
        if version != ENVELOPE_VERSION:
            raise EnvelopeError(f"unsupported version {version}")
        status = doc.get("s")
        # A final status only: an interim 1xx written as the answer would
        # leave the client waiting for the real one.
        if not isinstance(status, int) or isinstance(status, bool) or not 200 <= status <= 599:
            raise EnvelopeError("s must be a final HTTP status")
        error = doc.get("e")
        if error is not None and not isinstance(error, str):
            raise EnvelopeError("e must be a string")
        return Response(request_id, status, _headers(doc), _body(doc), error)
    raise EnvelopeError(f"unknown op {op!r}")
