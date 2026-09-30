"""The one mapping between the interface definition's types and JSON.

The chapter's encoding table, applied in both directions from the generated
table rather than from a hand-written list per method: a parameter is decoded
by its declared type, a result is encoded by its declared type, and a
dictionary's fields are decoded by the dictionary's declaration. Nothing here
knows a method by name.

Errors are the engine's own taxonomy. A ``ProtocolError`` becomes a JSON-RPC
error whose ``code`` is ``-32000 - position`` in the definition's error enum
and whose ``data.variant`` is the variant name; the session refusals the
chapter defines reuse the same variants, so no error exists only on this
wire.
"""

from __future__ import annotations

import base64
import binascii
import enum
from typing import Any

from .. import offline_protocol as generated
from .table import TABLE

PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603

#: The base of the taxonomy's range: variant ``n`` is ``TAXONOMY_BASE - n``.
TAXONOMY_BASE = -32000

_INT_RANGES = {
    "u8": (0, 2**8 - 1),
    "u16": (0, 2**16 - 1),
    "u32": (0, 2**32 - 1),
    "u64": (0, 2**64 - 1),
    "i16": (-(2**15), 2**15 - 1),
    "i32": (-(2**31), 2**31 - 1),
    "i64": (-(2**63), 2**63 - 1),
}
_FLOATS = ("f32", "double")

ERROR_VARIANTS: tuple[str, ...] = TABLE["errors"]["ProtocolError"]


class RpcError(Exception):
    """A JSON-RPC error object, raised to end a call."""

    def __init__(self, code: int, message: str, data: dict[str, Any] | None = None) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data

    def to_json(self) -> dict[str, Any]:
        error: dict[str, Any] = {"code": self.code, "message": self.message}
        if self.data is not None:
            error["data"] = self.data
        return error


def taxonomy_error(variant: str, message: str) -> RpcError:
    """A refusal spelled with one of the engine's variants."""
    position = ERROR_VARIANTS.index(variant)
    return RpcError(TAXONOMY_BASE - position, message, {"variant": variant})


def error_from_protocol(exc: BaseException) -> RpcError:
    """The engine's exception as the wire's error, variant and all."""
    variant = type(exc).__name__
    if variant not in ERROR_VARIANTS:
        # A generated error class the table does not know: the table is
        # stale against the library, which a test refuses before this can
        # happen in a checked-in tree.
        return RpcError(INTERNAL_ERROR, f"unmapped engine error {variant}: {exc}")
    return taxonomy_error(variant, str(exc))


def invalid_params(message: str) -> RpcError:
    return RpcError(INVALID_PARAMS, message)


# -- decoding (JSON -> the generated binding's values) ------------------------


def _b64decode(value: Any, where: str) -> bytes:
    if not isinstance(value, str):
        raise invalid_params(f"{where}: bytes are a base64 string")
    try:
        return base64.b64decode(value, validate=True)
    except (binascii.Error, ValueError) as exc:
        raise invalid_params(f"{where}: not valid base64 ({exc})") from None


def decode(type_name: str, value: Any, where: str) -> Any:
    """``value`` (parsed JSON) as the Python value the binding takes for
    ``type_name`` (a type as the definition spells it, spaces removed)."""
    if type_name.endswith("?"):
        if value is None:
            return None
        return decode(type_name[:-1], value, where)
    if type_name == "string":
        if not isinstance(value, str):
            raise invalid_params(f"{where}: expected a string")
        return value
    if type_name == "boolean":
        if not isinstance(value, bool):
            raise invalid_params(f"{where}: expected true or false")
        return value
    if type_name in _INT_RANGES:
        if isinstance(value, bool) or not isinstance(value, int):
            raise invalid_params(f"{where}: expected an integer")
        low, high = _INT_RANGES[type_name]
        if not low <= value <= high:
            raise invalid_params(f"{where}: {value} is outside {type_name}")
        return value
    if type_name in _FLOATS:
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise invalid_params(f"{where}: expected a number")
        try:
            return float(value)
        except OverflowError:
            # An integer JSON can spell but a double cannot hold: a refusal,
            # not an exception that closes the connection.
            raise invalid_params(f"{where}: {value!r} is outside {type_name}") from None
    if type_name == "bytes":
        return _b64decode(value, where)
    if type_name == "sequence<u8>":
        return list(_b64decode(value, where))
    if type_name.startswith("sequence<"):
        inner = type_name[len("sequence<") : -1]
        if not isinstance(value, list):
            raise invalid_params(f"{where}: expected an array")
        return [decode(inner, item, f"{where}[{i}]") for i, item in enumerate(value)]
    if type_name.startswith("record<DOMString,"):
        inner = type_name[len("record<DOMString,") : -1]
        if not isinstance(value, dict):
            raise invalid_params(f"{where}: expected an object")
        return {key: decode(inner, item, f"{where}.{key}") for key, item in value.items()}
    if type_name in TABLE["enums"]:
        variants = TABLE["enums"][type_name]
        if not isinstance(value, str) or value not in variants:
            raise invalid_params(
                f"{where}: expected one of {', '.join(variants)} for {type_name}"
            )
        return getattr(generated, type_name)(variants.index(value))
    if type_name in TABLE["records"]:
        return _decode_record(type_name, value, where)
    raise invalid_params(f"{where}: a value of type {type_name} has no JSON form")


def _decode_record(type_name: str, value: Any, where: str) -> Any:
    if not isinstance(value, dict):
        raise invalid_params(f"{where}: expected an object shaped as {type_name}")
    fields = TABLE["records"][type_name]
    known = {name for name, _, _ in fields}
    unknown = sorted(set(value) - known)
    if unknown:
        raise invalid_params(f"{where}: {type_name} has no field {unknown[0]!r}")
    kwargs: dict[str, Any] = {}
    for name, field_type, has_default in fields:
        if name in value:
            kwargs[name] = decode(field_type, value[name], f"{where}.{name}")
        elif has_default:
            # Left to the definition's own default (C6): the server never
            # substitutes a literal for a field the client did not send.
            continue
        elif field_type.endswith("?"):
            kwargs[name] = None
        else:
            raise invalid_params(f"{where}: {type_name} requires {name}")
    return getattr(generated, type_name)(**kwargs)


# -- encoding (the binding's values -> JSON) ----------------------------------


def encode(type_name: str, value: Any) -> Any:
    """``value`` as returned by the binding for ``type_name``, as JSON."""
    if value is None:
        return None
    if type_name.endswith("?"):
        return encode(type_name[:-1], value)
    if type_name == "void":
        return None
    if type_name in ("string", "boolean") or type_name in _INT_RANGES or type_name in _FLOATS:
        return value
    if type_name == "bytes":
        return base64.b64encode(bytes(value)).decode("ascii")
    if type_name == "sequence<u8>":
        return base64.b64encode(bytes(value)).decode("ascii")
    if type_name.startswith("sequence<"):
        inner = type_name[len("sequence<") : -1]
        return [encode(inner, item) for item in value]
    if type_name.startswith("record<DOMString,"):
        inner = type_name[len("record<DOMString,") : -1]
        return {key: encode(inner, item) for key, item in value.items()}
    if type_name in TABLE["enums"]:
        if isinstance(value, enum.Enum):
            return TABLE["enums"][type_name][value.value]
        return TABLE["enums"][type_name][int(value)]
    if type_name in TABLE["records"]:
        return {
            name: encode(field_type, getattr(value, name))
            for name, field_type, _ in TABLE["records"][type_name]
        }
    raise RpcError(INTERNAL_ERROR, f"a result of type {type_name} has no JSON form")
