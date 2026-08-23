"""Whether this connection proves its ``off1…`` address to the relay, and the
exact bytes it signs to do it.

Mirrors ``bindings/react-native/ios/AddressDeclarationPolicy.swift`` and
``android/src/main/java/com/offlineprotocol/AddressDeclarationPolicy.kt``.
Keep the three in sync.

Why the declaration exists
--------------------------

The relay authenticates a JWT and knows the connection by its *account name*,
but since the addressing cutover the core stamps ``Message.sender`` with
``local_address()``. A relay that attributes an inbound frame by account name
therefore hands the receiver a ``transport_peer_id`` that cannot match the
sender it is strict-matched against in ``validate_transport_sender``, and every
security-gated control frame (``__MLS_KEY_PKG__``, ``__MLS_WELCOME__``) is
rejected, so no MLS session can be established over the relay at all. Declaring
closes that, and is also what makes an ``off1…`` recipient resolvable in the
relay's registry.

Why the account name is inside the signed bytes
-----------------------------------------------

The signing key here is the identity key that also signs mesh control frames
under ``offline-ctrl-v1``. If the proof were a signature over a bare
relay-chosen challenge, a hostile relay could hand out a challenge shaped like a
control-frame payload and harvest a signature that replays as a control frame
from this peer. Binding the domain *and* the account makes the signature
meaningful only as "this account, on this relay, holds this key": it cannot be
replayed under another account, nor onto another connection, since the challenge
is minted per connection.

The layout is fixed by the relay's ``address_binding::address_proof_payload``
and pinned there by a hex vector, which
``test_address_declaration.py::test_proof_payload_matches_the_pinned_relay_vector``
mirrors byte for byte. Neither side may change it alone.

Why every failure is a skip rather than an error
------------------------------------------------

A connection that does not declare keeps working exactly as it did before
addresses existed: the relay attributes it by account name, which is the legacy
path and still the only path an older relay has. So the absence of a capability,
of a challenge, or of a local identity is a reason to stay quiet, not to fail a
connection that is otherwise fine.
"""

from __future__ import annotations

import base64
import json
from dataclasses import dataclass
from typing import Any, Union

# The relay capability token gating the whole exchange. A relay that does not
# advertise it also omits ``address_challenge``, and would parse a
# ``DeclareAddress`` into nothing and answer nothing, so the token is the tell,
# not a timeout.
CAPABILITY = "address_routing_v1"

# Domain separator prefixing the signed payload. Must not prefix, nor be
# prefixed by, **either** of the core's control-frame domains,
# ``offline-ctrl-v1`` and ``offline-ctrl-v2``; the relay pins that relation in
# ``the_proof_domain_cannot_collide_with_control_message_signing``.
PROOF_DOMAIN = b"offline-relay-addr-v1"

# The relay mints exactly this many challenge bytes. A frame carrying any other
# length is malformed, and signing it would produce a proof that cannot verify,
# so it is better to skip and say so.
CHALLENGE_LENGTH = 32


class Reason:
    """Stable diagnostic reasons, shared with the Swift and Kotlin mirrors so a
    field report reproduces under the same string on all three platforms.

    The vocabulary diverges at two points, both deliberate. Swift and Kotlin
    also define ``frame_unserializable``, which Python cannot reach:
    ``json.dumps`` over three ``str`` values is total, where
    ``JSONSerialization`` and ``JSONObject`` are fallible, so the constant
    would be a token nothing can ever emit. Going the other way,
    ``connection_replaced`` is Python-only: the other two bridges make the same
    stale-socket check but return from it silently, so there is no shared
    string to match.
    """

    #: The relay does not advertise ``address_routing_v1``, an older
    #: deployment. Expected, and the reason this is not an error.
    CAPABILITY_ABSENT = "capability_absent"
    #: Capability advertised but no ``address_challenge`` came with it.
    CHALLENGE_ABSENT = "challenge_absent"
    #: The challenge was not standard base64, or did not decode to exactly
    #: :data:`CHALLENGE_LENGTH` bytes.
    CHALLENGE_MALFORMED = "challenge_malformed"
    #: The ``Authenticated`` frame carried no account name to bind the proof
    #: to. Never sign a locally-chosen substitute here: the relay verifies
    #: against the name *it* resolved, so a guess produces a signature that
    #: cannot verify and is indistinguishable, in the relay's logs, from an
    #: attack.
    ACCOUNT_ABSENT = "account_absent"
    #: ``local_address()`` was None, so MLS is not initialized and there is no
    #: identity to prove. An app running with encryption disabled stays in
    #: account-name space by construction.
    ADDRESS_UNAVAILABLE = "address_unavailable"
    #: The identity key or the signature could not be produced.
    SIGNING_FAILED = "signing_failed"
    #: The socket that was handed the challenge is no longer the live one.
    #: Its successor carries its own ``Authenticated`` frame, and its own
    #: challenge, so this connection has nothing to prove.
    CONNECTION_REPLACED = "connection_replaced"


@dataclass(frozen=True)
class Declare:
    """Sign :func:`proof_payload` for these values and send the declaration."""

    account: str
    challenge: bytes


@dataclass(frozen=True)
class Skip:
    """Send nothing. ``reason`` is one of :class:`Reason`."""

    reason: str


Outcome = Union[Declare, Skip]


def decide(
    capabilities: Any,
    address_challenge: str | None,
    username: str | None,
) -> Outcome:
    """Decide whether this connection can declare, from the ``Authenticated``
    frame alone.

    Deliberately does **not** take the local address: this runs before any FFI
    call so that the common skip (an older relay) costs no acquisition of the
    protocol mutex. The caller fetches the address only after a :class:`Declare`
    and reports :attr:`Reason.ADDRESS_UNAVAILABLE` if it is None.

    :param capabilities: the ``capabilities`` field, verbatim and untrusted.
    :param address_challenge: the ``address_challenge`` field, base64 of the raw
        challenge bytes, absent on relays without the capability.
    :param username: the ``username`` field **as the relay sent it**. Callers
        must not substitute a local fallback (see :attr:`Reason.ACCOUNT_ABSENT`).
    """
    # A relay is free to send anything. `in` over a str is a substring test, so
    # a scalar "not_address_routing_v1" would satisfy a bare membership check;
    # require the JSON array the schema promises.
    if not isinstance(capabilities, list) or CAPABILITY not in capabilities:
        return Skip(Reason.CAPABILITY_ABSENT)
    if not address_challenge:
        return Skip(Reason.CHALLENGE_ABSENT)
    try:
        # Standard alphabet, padding required: the relay decodes with the same,
        # and rejects base64url or unpadded input. `validate=True` is what makes
        # this a rejection rather than a silent discard of stray characters.
        challenge = base64.b64decode(address_challenge, validate=True)
    except Exception:
        return Skip(Reason.CHALLENGE_MALFORMED)
    if len(challenge) != CHALLENGE_LENGTH:
        return Skip(Reason.CHALLENGE_MALFORMED)
    if not username:
        return Skip(Reason.ACCOUNT_ABSENT)
    return Declare(account=username, challenge=challenge)


def proof_payload(account: str, challenge: bytes) -> bytes:
    """The exact bytes the relay verifies the signature over.

    ``"offline-relay-addr-v1" || u32be(len(account.utf8)) || account.utf8 || challenge``

    No separators, no terminators. The length prefix is what makes the
    concatenation unambiguous: without it, an account name ending in bytes that
    look like the start of a challenge could be re-split, so two different
    (account, challenge) pairs would share one payload.
    """
    account_bytes = account.encode("utf-8")
    # Big-endian, and of the UTF-8 *byte* count, not the character count, which
    # differs for any non-ASCII account name.
    return (
        PROOF_DOMAIN
        + len(account_bytes).to_bytes(4, "big")
        + account_bytes
        + challenge
    )


def declaration_json(address: str, public_key: bytes, signature: bytes) -> str:
    """The ``DeclareAddress`` frame, serialized.

    Owns the base64 encoding so the wire contract lives in one place per
    platform: standard alphabet, padded, no line breaks. A 32-byte key encodes
    to 44 characters and a 64-byte signature to 88. The relay decodes with a
    strict engine and refuses anything else.
    """
    return json.dumps(
        {
            "type": "DeclareAddress",
            "address": address,
            "public_key": base64.b64encode(public_key).decode("ascii"),
            "signature": base64.b64encode(signature).decode("ascii"),
        }
    )
