"""The envelope's limits, header list and tokens, pinned as literals (C5).

These literals restate ``docs/spec/http-front.md``. They are written out
here rather than read from the module: a test that read them would agree
with any edit.
"""

from __future__ import annotations

import base64
import json

import pytest

from offline_protocol_sdk.http_front import envelope
from offline_protocol_sdk.http_front.envelope import EnvelopeError, Request, Response, carried, decode


def test_the_limits_are_the_chapters():
    assert envelope.FRONT_APP_ID == "offline-protocol-http-front"
    assert envelope.ENVELOPE_VERSION == 1
    assert envelope.MAX_BODY_BYTES == 131072
    assert envelope.MAX_HEADER_BYTES == 8192
    assert envelope.MAX_PATH_BYTES == 2048
    assert envelope.DEFAULT_DEADLINE_SECONDS == 30
    assert envelope.MAX_DEADLINE_SECONDS == 300
    assert envelope.CLOCK_SLACK_SECONDS == 60
    assert envelope.MIN_CALLBACK_SECONDS == 5
    assert envelope.MAX_CONCURRENT_CALLBACKS == 32


def test_the_carried_headers_are_the_chapters():
    assert envelope.CARRIED_HEADERS == {
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
    assert envelope.OWN_HEADER_PREFIX == "x-offline-protocol-"
    assert envelope.FORWARDING_HEADER_PREFIX == "x-forwarded-"
    assert envelope.FORWARDING_HEADERS == {"x-real-ip", "x-original-url", "x-rewrite-url"}


def test_the_error_tokens_are_the_chapters():
    assert envelope.ERRORS == {
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


def test_a_body_at_the_limit_fits_the_engines_content_limit():
    """128 KiB raw, as base64 inside a full envelope, stays under 256 KiB."""
    request = Request(
        "a" * 32,
        "s" * 63,
        "POST",
        "/" + "p" * 2047,
        {"x-h": "v" * 8000},
        b"\xff" * envelope.MAX_BODY_BYTES,
        2_000_000_000,
    )
    assert len(request.encode().encode()) < 256 * 1024


def test_only_listed_and_x_headers_cross():
    headers = carried(
        [
            ("Content-Type", "text/plain"),
            ("Cookie", "secret=1"),
            ("Connection", "keep-alive"),
            ("Host", "a.b.c"),
            ("Content-Length", "3"),
            ("X-Trace", "1"),
            ("X-Trace", "2"),
            ("X-Offline-Protocol-Token", "t"),
            ("Authorization", "Bearer x"),
            ("X-Forwarded-For", "10.0.0.1"),
            ("X-Forwarded-Host", "admin.internal"),
            ("X-Real-IP", "10.0.0.1"),
            ("X-Original-URL", "/admin"),
            ("X-Rewrite-URL", "/admin"),
        ]
    )
    assert headers == {"content-type": "text/plain", "x-trace": "1, 2", "authorization": "Bearer x"}


@pytest.mark.parametrize(
    "path, dotted",
    [
        ("/api/../admin", True),
        ("/api/%2e%2e/admin", True),
        ("/api/%2E%2e/admin", True),
        ("/./x", True),
        ("/api/%2e/x", True),
        ("/api/..", True),
        ("/a..b/..c/c..", False),
        ("/x?next=/../admin", False),
        ("/", False),
    ],
)
def test_a_dot_segment_is_found_raw_or_encoded(path, dotted):
    assert envelope.has_dot_segment(path) is dotted


def test_a_tab_in_a_header_value_is_allowed():
    doc = {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/", "dl": 1, "h": {"x-a": "a\tb"}}
    assert decode(json.dumps(doc)).headers == {"x-a": "a\tb"}


def test_a_request_and_a_response_round_trip():
    request = Request("id1", "timeofday", "post", "/now?tz=utc", {"accept": "*/*"}, b"\x00\x01", 1760000000)
    decoded = decode(request.encode())
    assert decoded == Request("id1", "timeofday", "POST", "/now?tz=utc", {"accept": "*/*"}, b"\x00\x01", 1760000000)
    response = Response("id1", 201, {"etag": "x"}, b"done")
    assert decode(response.encode()) == response
    failure = Response.failure("id1", "unknown_service")
    assert decode(failure.encode()) == Response("id1", 404, {}, b"", "unknown_service")


def test_a_request_with_another_version_comes_back_for_refusal():
    decoded = decode(json.dumps({"op": "req", "v": 2, "id": "x"}))
    assert isinstance(decoded, Request) and decoded.version == 2


@pytest.mark.parametrize(
    "doc",
    [
        "not json",
        "[]",
        {"op": "req", "v": 1},
        {"op": "nope", "v": 1, "id": "x"},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "no-slash", "dl": 1},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/", "dl": "soon"},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/", "dl": 1, "b": "!!"},
        {
            "op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/", "dl": 1,
            "b": base64.b64encode(b"x" * 131073).decode(),
        },
        {"op": "resp", "v": 1, "id": "x", "s": 99},
        {"op": "resp", "v": 1, "id": "x", "s": 101},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/api/../admin", "dl": 1},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/api/%2e%2e/admin", "dl": 1},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/a\r\nb", "dl": 1},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET /x HTTP/1.1\r\nX: y", "p": "/", "dl": 1},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "", "p": "/", "dl": 1},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/", "dl": 1, "h": {"x-a": "v\r\nInjected: 1"}},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/", "dl": 1, "h": {"x-a": "v\u0000"}},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/", "dl": 1, "h": {"x-a b": "v"}},
        {"op": "resp", "v": 1, "id": "x", "s": 200, "h": {"x-a": "v\nInjected: 1"}},
        {"op": "resp", "v": 1, "id": "x", "s": True},
        {"op": "resp", "v": 2, "id": "x", "s": 200},
        {"op": "resp", "v": 1, "id": "x", "s": 200, "h": {"x-big": "v" * 8200}},
        # A lone surrogate has no UTF-8 form: refused as an envelope, not
        # left to raise a UnicodeEncodeError nothing catches.
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/\udcff", "dl": 1},
        {"op": "req", "v": 1, "id": "x", "svc": "s", "m": "GET", "p": "/", "dl": 1, "h": {"x-a": "\udcff"}},
        {"op": "resp", "v": 1, "id": "x", "s": 200, "h": {"x-a": "\udcff"}},
    ],
)
def test_what_is_not_an_envelope_is_refused(doc):
    """Including what the HTTP library would refuse with a ValueError later,
    and a path that would escape the callback's."""
    with pytest.raises(EnvelopeError):
        decode(doc if isinstance(doc, str) else json.dumps(doc))
