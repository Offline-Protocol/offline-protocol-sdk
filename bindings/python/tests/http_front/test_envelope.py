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
        ]
    )
    assert headers == {"content-type": "text/plain", "x-trace": "1, 2", "authorization": "Bearer x"}


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
        {"op": "resp", "v": 1, "id": "x", "s": True},
        {"op": "resp", "v": 2, "id": "x", "s": 200},
        {"op": "resp", "v": 1, "id": "x", "s": 200, "h": {"x-big": "v" * 8200}},
    ],
)
def test_what_is_not_an_envelope_is_refused(doc):
    with pytest.raises(EnvelopeError):
        decode(doc if isinstance(doc, str) else json.dumps(doc))
