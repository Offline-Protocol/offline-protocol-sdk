"""The encoding table, both directions."""

from __future__ import annotations

import base64

import pytest

from offline_protocol_sdk import offline_protocol as generated
from offline_protocol_sdk.local_api import codec
from offline_protocol_sdk.local_api.codec import RpcError


def _refused(type_name, value):
    with pytest.raises(RpcError) as err:
        codec.decode(type_name, value, "p")
    assert err.value.code == codec.INVALID_PARAMS
    return err.value.message


def test_enums_use_the_definition_spelling_both_ways():
    assert codec.decode("MessagePriority", "Medium", "p") is generated.MessagePriority.MEDIUM
    assert codec.decode("TransportType", "WiFiDirect", "p") is generated.TransportType.WI_FI_DIRECT
    assert codec.encode("TransportType", generated.TransportType.WI_FI_DIRECT) == "WiFiDirect"
    assert codec.encode("ProtocolState", generated.ProtocolState.RUNNING) == "Running"
    assert "Low, Medium, High, Critical" in _refused("MessagePriority", "medium")
    assert codec.decode("MessagePriority?", None, "p") is None


def test_bytes_are_base64_and_sequences_of_u8_become_lists():
    raw = bytes(range(8))
    text = base64.b64encode(raw).decode()
    assert codec.decode("bytes", text, "p") == raw
    assert codec.decode("sequence<u8>", text, "p") == list(raw)
    assert codec.encode("sequence<u8>", list(raw)) == text
    assert codec.encode("bytes", raw) == text
    _refused("sequence<u8>", "not base64!")
    _refused("sequence<u8>", [1, 2, 3])


def test_integers_are_checked_for_range_and_kind():
    assert codec.decode("u8", 255, "p") == 255
    _refused("u8", 256)
    _refused("u32", -1)
    _refused("u64", True)
    _refused("i16", 40000)
    _refused("string", 5)
    _refused("boolean", 1)
    assert codec.decode("double", 3, "p") == 3.0


def test_records_take_the_definition_defaults_for_omitted_fields():
    options = codec.decode("SendMessageOptions", {"priority": "High"}, "p")
    assert isinstance(options, generated.SendMessageOptions)
    assert options.priority is generated.MessagePriority.HIGH
    assert options.app_id is None
    assert options.reply_to_msg is None
    # An optional field with no default is passed as null.
    metadata = codec.decode(
        "MediaMetadata",
        {"mime_type": "image/png", "file_name": "a.png", "file_size": 3},
        "p",
    )
    assert metadata.duration_ms is None and metadata.width is None
    assert "requires file_size" in _refused(
        "MediaMetadata", {"mime_type": "image/png", "file_name": "a.png"}
    )
    assert "no field" in _refused("SendMessageOptions", {"priorty": "High"})
    _refused("SendMessageOptions", "not an object")


def test_records_encode_field_by_field_with_nested_types():
    info = generated.ForwardInfo(
        original_sender="a", original_message_id="m", original_timestamp=5, forward_count=1
    )
    assert codec.encode("ForwardInfo?", info) == {
        "original_sender": "a",
        "original_message_id": "m",
        "original_timestamp": 5,
        "forward_count": 1,
    }
    assert codec.encode("ForwardInfo?", None) is None
    assert codec.encode("void", None) is None
    assert codec.encode("sequence<string>", ["x"]) == ["x"]
    assert codec.encode("record<DOMString,string>", {"k": "v"}) == {"k": "v"}


def test_records_and_maps_decode_recursively():
    assert codec.decode("record<DOMString,string>", {"k": "v"}, "p") == {"k": "v"}
    _refused("record<DOMString,string>", {"k": 1})
    assert codec.decode("sequence<string>", ["a"], "p") == ["a"]
    _refused("sequence<string>", "a")


def test_values_without_a_json_form_are_refused_not_crashed():
    assert "no JSON form" in _refused("EventCallback", {})
    with pytest.raises(RpcError) as err:
        codec.encode("OfflineProtocol", object())
    assert err.value.code == codec.INTERNAL_ERROR
