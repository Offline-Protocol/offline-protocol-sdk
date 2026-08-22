"""Cross-repo contract for the relay address declaration.

The signed byte layout is fixed by the relay's
``address_binding::address_proof_payload`` and pinned there by a hex vector.
This suite mirrors that vector byte for byte, exactly as the Swift
``AddressDeclarationPolicyTests.proofPayloadMatchesThePinnedRelayVector`` and
the Kotlin ``AddressDeclarationPolicyTest.proofPayloadMatchesThePinnedRelayVector``
do. Neither side may change the layout alone.

A hand-rolled payload does not fail loudly. It produces a signature the relay
refuses, so the connection silently stays in account-name space and every
security-gated control frame it carries is dropped at the receiver. That is the
failure this vector exists to catch.

If the format ever legitimately changes, re-derive the literal from the relay's
structure. Never edit it to match new output.
"""

from __future__ import annotations

import base64
import json

from offline_protocol_sdk import address_declaration as policy

# The relay's own vector: account "alice", challenge = bytes 0x00..0x1f.
VECTOR_CHALLENGE = bytes(range(32))
VECTOR_HEX = (
    "6f66666c696e652d72656c61792d616464722d7631"
    "00000005"
    "616c696365"
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
)


class TestProofPayload:
    def test_proof_payload_matches_the_pinned_relay_vector(self) -> None:
        assert policy.proof_payload("alice", VECTOR_CHALLENGE).hex() == VECTOR_HEX

    def test_account_length_prefix_is_big_endian(self) -> None:
        """A little-endian binding writes ``05000000`` here and produces a
        signature the relay refuses. Isolated so a failure names the cause."""
        payload = policy.proof_payload("alice", VECTOR_CHALLENGE)
        domain_len = len(policy.PROOF_DOMAIN)
        assert payload[domain_len : domain_len + 4].hex() == "00000005"

    def test_account_length_counts_utf8_bytes_not_characters(self) -> None:
        """The relay reads the same bytes back out, so a character count
        silently mis-frames every account name outside ASCII."""
        account = "zoë"  # 3 characters, 4 UTF-8 bytes
        payload = policy.proof_payload(account, VECTOR_CHALLENGE)
        domain_len = len(policy.PROOF_DOMAIN)
        assert payload[domain_len : domain_len + 4].hex() == "00000004"
        assert len(payload) == domain_len + 4 + 4 + 32

    def test_the_account_length_prefix_makes_the_payload_unambiguous(self) -> None:
        """Without the prefix, ("ab", "cX") and ("abc", "X") concatenate to the
        same bytes and one signature covers both, so an account able to pick a
        name that re-splits into another's would inherit its proofs."""
        assert policy.proof_payload("ab", b"cX") != policy.proof_payload("abc", b"X")

    def test_proof_domain_cannot_collide_with_control_frame_signing(self) -> None:
        """If either domain prefixed the other, a relay-chosen challenge could
        steer this signature into the control-message domain and replay it as a
        frame from this peer. The relay pins the same relation from its side."""
        control_domain = b"offline-ctrl-v1"
        assert not policy.PROOF_DOMAIN.startswith(control_domain)
        assert not control_domain.startswith(policy.PROOF_DOMAIN)

    def test_payload_is_never_the_bare_challenge(self) -> None:
        """The naive implementation signs the challenge alone. The relay
        refuses that shape explicitly."""
        payload = policy.proof_payload("alice", VECTOR_CHALLENGE)
        assert payload != VECTOR_CHALLENGE
        assert payload.startswith(policy.PROOF_DOMAIN)


class TestDecide:
    @staticmethod
    def _challenge_b64() -> str:
        return base64.b64encode(VECTOR_CHALLENGE).decode("ascii")

    def test_declares_when_capability_challenge_and_account_are_present(self) -> None:
        outcome = policy.decide(
            ["group_delivery_v3", policy.CAPABILITY], self._challenge_b64(), "alice"
        )
        assert outcome == policy.Declare(account="alice", challenge=VECTOR_CHALLENGE)

    def test_skips_when_the_relay_does_not_advertise_the_capability(self) -> None:
        outcome = policy.decide(["group_delivery_v3"], self._challenge_b64(), "alice")
        assert outcome == policy.Skip(policy.Reason.CAPABILITY_ABSENT)

    def test_a_scalar_capabilities_field_is_not_a_substring_match(self) -> None:
        """`in` over a str is a substring test, so a relay sending a bare
        string could satisfy a naive membership check with a token that merely
        contains the capability name."""
        outcome = policy.decide(
            "not_" + policy.CAPABILITY, self._challenge_b64(), "alice"
        )
        assert outcome == policy.Skip(policy.Reason.CAPABILITY_ABSENT)

    def test_skips_when_the_challenge_is_absent(self) -> None:
        assert policy.decide([policy.CAPABILITY], None, "alice") == policy.Skip(
            policy.Reason.CHALLENGE_ABSENT
        )
        assert policy.decide([policy.CAPABILITY], "", "alice") == policy.Skip(
            policy.Reason.CHALLENGE_ABSENT
        )

    def test_skips_when_the_challenge_is_the_wrong_length(self) -> None:
        short = base64.b64encode(b"\x00" * 31).decode("ascii")
        assert policy.decide([policy.CAPABILITY], short, "alice") == policy.Skip(
            policy.Reason.CHALLENGE_MALFORMED
        )

    def test_skips_when_the_challenge_is_not_standard_base64(self) -> None:
        """The relay decodes with a strict engine: base64url and unpadded input
        are refused there, so signing them here would waste a proof."""
        url_alphabet = (
            base64.urlsafe_b64encode(bytes([251] * 32)).decode("ascii").rstrip("=")
        )
        assert policy.decide(
            [policy.CAPABILITY], url_alphabet, "alice"
        ) == policy.Skip(policy.Reason.CHALLENGE_MALFORMED)

    def test_skips_when_the_relay_supplied_no_account_name(self) -> None:
        """The relay verifies the proof against the name *it* resolved. A
        locally-chosen substitute cannot verify and reads as an attack in the
        relay's logs, so the absence of a name is a skip and never a guess."""
        assert policy.decide(
            [policy.CAPABILITY], self._challenge_b64(), None
        ) == policy.Skip(policy.Reason.ACCOUNT_ABSENT)


class TestDeclarationJson:
    def test_frame_uses_standard_padded_base64(self) -> None:
        frame = json.loads(
            policy.declaration_json("off1abc", bytes(range(32)), bytes(range(64)))
        )
        assert frame["type"] == "DeclareAddress"
        assert frame["address"] == "off1abc"
        # A 32-byte key encodes to 44 characters, a 64-byte signature to 88.
        assert len(frame["public_key"]) == 44
        assert len(frame["signature"]) == 88
        assert base64.b64decode(frame["public_key"], validate=True) == bytes(range(32))
        assert base64.b64decode(frame["signature"], validate=True) == bytes(range(64))
