"""Tests for the gateway attach policy: the decisions and frame shapes the
gateway-daemon client is built from, with no socket and no core.

The constants are pinned as literals, not recomputed from the module: they
are hand-mirrored in Swift and Kotlin with no compiler between the three
(C5), and a test that agreed with any edit would be the failure mode.
"""

from __future__ import annotations

import base64
import json

import pytest

from offline_protocol_sdk import gateway_attach_policy as policy
from offline_protocol_sdk.gateway_attach_policy import (
    BindingOutcome,
    Declare,
    PresenceAnswer,
    Skip,
    SkipReason,
    Verdict,
    VerdictReport,
    binding_outcome,
    capability_tokens,
    check_presence_json,
    declaration_json,
    decode_challenge,
    identify_json,
    parse_presence,
    parse_verdict,
    sanitize_message_id,
    send_message_json,
    verdict_report,
)

OUR_ADDRESS = "off1qysluvwl5922yctzd0u9gpr06gn3k7ldfvgtwgvn"
OTHER_ADDRESS = "off1qyulwy7s5ezz20cy222zrw04rwds39uapqv9j8r0"


# ---------------------------------------------------------------------------
# C5: the hand-mirrored constants, as literals
# ---------------------------------------------------------------------------


class TestConstantsArePinnedAsLiterals:
    def test_protocol_version(self):
        assert policy.PROTOCOL_VERSION == 1

    def test_challenge_length(self):
        assert policy.CHALLENGE_LENGTH == 32

    def test_attach_timeout(self):
        assert policy.ATTACH_TIMEOUT == 10.0

    def test_verdict_timeout_is_sixty_seconds(self):
        # The relationship (under the core's 120 s pending-confirmation
        # expiry) is held by the Rust guard, which reads this file and the
        # transport crate's constant. What this pins is the spelling.
        assert policy.VERDICT_TIMEOUT == 60.0

    def test_in_flight_cap(self):
        assert policy.MAX_IN_FLIGHT == 8

    def test_line_cap(self):
        assert policy.MAX_LINE_BYTES == 1_048_576

    def test_capability_bounds_match_the_relay_rules(self):
        assert policy.MAX_CAPABILITY_TOKENS == 64
        assert policy.MAX_CAPABILITY_TOKEN_BYTES == 128

    def test_presence_peers_per_query(self):
        assert policy.MAX_PRESENCE_PEERS == 64

    def test_address_echo_bound(self):
        assert policy.MAX_ADDRESS_BYTES == 128

    def test_core_reason_tokens(self):
        # The exact literals the engine's classifier matches
        # (SEND_FAIL_REASON_* in crates/offline-protocol/src/protocol/types.rs).
        assert policy.RECIPIENT_UNREACHABLE == "recipient_unreachable"
        assert policy.RELAY_PUSHED == "relay_pushed"
        assert policy.RELAY_STORED == "relay_stored"
        assert policy.RELAY_PUSHED_STORED == "relay_pushed_stored"
        # Not a token the classifier knows: it degrades to the generic
        # transport failure, which is a retry. It must never start with the
        # unreachable prefix, or silence would park the frame instead.
        assert policy.GATEWAY_SILENT == "gateway_silent: no verdict within 60s"
        assert not policy.GATEWAY_SILENT.startswith(policy.RECIPIENT_UNREACHABLE)


# ---------------------------------------------------------------------------
# Attach
# ---------------------------------------------------------------------------


class TestDecodeChallenge:
    def test_a_32_byte_challenge_is_declared(self):
        challenge = bytes(range(32))
        encoded = base64.b64encode(challenge).decode()
        assert decode_challenge({"challenge": encoded}) == Declare(challenge)

    def test_absent_or_empty_challenge_is_skipped(self):
        assert decode_challenge({}) == Skip(SkipReason.CHALLENGE_ABSENT)
        assert decode_challenge({"challenge": ""}) == Skip(SkipReason.CHALLENGE_ABSENT)
        assert decode_challenge({"challenge": 42}) == Skip(SkipReason.CHALLENGE_ABSENT)

    def test_malformed_base64_is_skipped(self):
        assert decode_challenge({"challenge": "not base64!"}) == Skip(
            SkipReason.CHALLENGE_MALFORMED
        )

    @pytest.mark.parametrize("length", [1, 16, 31, 33, 64])
    def test_wrong_size_is_skipped(self, length):
        encoded = base64.b64encode(bytes(length)).decode()
        assert decode_challenge({"challenge": encoded}) == Skip(
            SkipReason.CHALLENGE_WRONG_SIZE
        )


class TestBindingOutcome:
    def test_bound_when_the_echo_is_ours(self):
        assert binding_outcome(OUR_ADDRESS, OUR_ADDRESS) is BindingOutcome.BOUND

    def test_mismatch_when_the_echo_is_someone_else(self):
        assert binding_outcome(OTHER_ADDRESS, OUR_ADDRESS) is BindingOutcome.MISMATCH

    def test_unknown_local_when_we_hold_no_address(self):
        assert binding_outcome(OUR_ADDRESS, None) is BindingOutcome.UNKNOWN_LOCAL
        assert binding_outcome(OUR_ADDRESS, "") is BindingOutcome.UNKNOWN_LOCAL


class TestCapabilityTokens:
    def test_missing_or_non_list_is_empty(self):
        assert capability_tokens({}) == []
        assert capability_tokens({"tokens": "gateway_v1"}) == []

    def test_non_strings_and_empty_strings_are_dropped(self):
        assert capability_tokens({"tokens": ["a", 1, "", None, "b"]}) == ["a", "b"]

    def test_oversized_tokens_are_dropped_before_the_count_applies(self):
        # A gateway padding its list with oversized tokens must not evict
        # the ones that matter.
        padding = ["x" * 129] * 64
        tokens = capability_tokens({"tokens": padding + ["gateway_v1"]})
        assert tokens == ["gateway_v1"]

    def test_the_count_is_capped_at_64(self):
        tokens = capability_tokens({"tokens": [f"t{i}" for i in range(100)]})
        assert len(tokens) == 64
        assert tokens[0] == "t0" and tokens[-1] == "t63"

    def test_a_128_byte_token_is_kept_and_129_is_not(self):
        assert capability_tokens({"tokens": ["y" * 128]}) == ["y" * 128]
        assert capability_tokens({"tokens": ["y" * 129]}) == []

    def test_the_byte_length_counts_not_the_character_count(self):
        # 64 two-byte characters are 128 bytes; 65 are 130.
        assert capability_tokens({"tokens": ["é" * 64]}) == ["é" * 64]
        assert capability_tokens({"tokens": ["é" * 65]}) == []


# ---------------------------------------------------------------------------
# Message ids
# ---------------------------------------------------------------------------


class TestSanitizeMessageId:
    def test_a_uuid_passes(self):
        uuid = "3f2b9c1e-8a4d-4c6b-9e2f-1a2b3c4d5e6f"
        assert sanitize_message_id(uuid) == uuid

    def test_empty_and_none_are_refused(self):
        assert sanitize_message_id(None) is None
        assert sanitize_message_id("") is None

    def test_64_characters_pass_and_65_do_not(self):
        assert sanitize_message_id("a" * 64) == "a" * 64
        assert sanitize_message_id("a" * 65) is None

    @pytest.mark.parametrize("bad", ["has space", "semi;colon", "slash/", "ünicode", "a\n"])
    def test_characters_outside_the_set_are_refused(self, bad):
        assert sanitize_message_id(bad) is None


# ---------------------------------------------------------------------------
# Frames this client sends
# ---------------------------------------------------------------------------


class TestOutboundFrames:
    def test_identify(self):
        assert json.loads(identify_json(OUR_ADDRESS)) == {
            "type": "Identify",
            "device_id": OUR_ADDRESS,
            "protocol_version": 1,
        }

    def test_declaration_base64_encodes_both_byte_fields(self):
        frame = json.loads(declaration_json(OUR_ADDRESS, b"\x01" * 32, b"\x02" * 64))
        assert frame["type"] == "DeclareAddress"
        assert frame["address"] == OUR_ADDRESS
        assert base64.b64decode(frame["public_key"]) == b"\x01" * 32
        assert base64.b64decode(frame["signature"]) == b"\x02" * 64

    def test_send_message_carries_the_id_and_base64_encoding(self):
        frame = json.loads(send_message_json("id-1", OTHER_ADDRESS, "AAEC", None))
        assert frame == {
            "type": "SendMessage",
            "recipient": OTHER_ADDRESS,
            "content": "AAEC",
            "encoding": "base64",
            "message_id": "id-1",
        }

    def test_send_message_adds_reply_to_only_when_present(self):
        with_reply = json.loads(send_message_json("id-1", OTHER_ADDRESS, "AA==", "parent"))
        assert with_reply["reply_to_msg"] == "parent"
        without = json.loads(send_message_json("id-1", OTHER_ADDRESS, "AA==", ""))
        assert "reply_to_msg" not in without

    def test_frames_are_one_line(self):
        for line in (
            identify_json(OUR_ADDRESS),
            declaration_json(OUR_ADDRESS, b"k", b"s"),
            send_message_json("id", OTHER_ADDRESS, "AA==", "r"),
            check_presence_json([OTHER_ADDRESS]),
        ):
            assert line is not None and "\n" not in line

    def test_check_presence_is_one_frame_for_the_batch(self):
        frame = json.loads(check_presence_json(["a", "b", "c"]))
        assert frame == {"type": "CheckPresence", "peers": ["a", "b", "c"]}

    def test_check_presence_with_nobody_to_ask_is_no_frame(self):
        assert check_presence_json([]) is None

    def test_check_presence_asks_about_at_most_64(self):
        frame = json.loads(check_presence_json([f"p{i}" for i in range(100)]))
        assert len(frame["peers"]) == 64
        assert frame["peers"][0] == "p0"


# ---------------------------------------------------------------------------
# Frames this client reads
# ---------------------------------------------------------------------------


class TestParseVerdict:
    def test_message_sent_is_a_confirmation(self):
        verdict = parse_verdict({"message_id": "id", "recipient": OTHER_ADDRESS}, "MessageSent")
        assert verdict == Verdict("id", None, OTHER_ADDRESS)
        assert verdict.sent

    def test_a_verdict_without_an_id_is_not_ours(self):
        assert parse_verdict({}, "MessageSent") is None
        assert parse_verdict({"message_id": ""}, "DeliveryError") is None
        assert parse_verdict({"message_id": 7}, "DeliveryError") is None

    def test_delivery_error_carries_the_reason_verbatim(self):
        verdict = parse_verdict(
            {"message_id": "id", "reason": "recipient_unreachable: no route"},
            "DeliveryError",
        )
        assert verdict.reason == "recipient_unreachable: no route"
        assert not verdict.sent

    def test_delivery_error_without_a_reason_is_still_a_failure(self):
        verdict = parse_verdict({"message_id": "id"}, "DeliveryError")
        assert verdict.reason == "DeliveryError"

    def test_flags_default_to_false_and_read_only_true(self):
        # The contract makes both optional booleans that default to false;
        # a non-boolean is not a claim the gateway made.
        absent = parse_verdict({"message_id": "id"}, "MessageSent")
        assert (absent.pushed, absent.stored) == (False, False)
        strings = parse_verdict(
            {"message_id": "id", "pushed": "true", "stored": 1}, "MessageSent"
        )
        assert (strings.pushed, strings.stored) == (False, False)
        both = parse_verdict({"message_id": "id", "pushed": True, "stored": True}, "MessageSent")
        assert (both.pushed, both.stored) == (True, True)

    def test_pushed_is_never_read_on_a_delivery_error(self):
        verdict = parse_verdict({"message_id": "id", "pushed": True}, "DeliveryError")
        assert verdict.pushed is False

    def test_an_empty_recipient_reads_as_none(self):
        assert parse_verdict({"message_id": "id", "recipient": ""}, "MessageSent").recipient is None


class TestVerdictReport:
    """The wire's two flags to the core's tokens: the relay client's own
    mapping, which is what makes a gateway's mailbox or push read the way
    the relay's does."""

    def test_plain_message_sent_confirms(self):
        report = verdict_report(Verdict("id", None, OTHER_ADDRESS))
        assert report == VerdictReport(None, False)
        assert report.confirmed

    def test_pushed_parks_under_relay_pushed(self):
        report = verdict_report(Verdict("id", None, OTHER_ADDRESS, pushed=True))
        assert report == VerdictReport("relay_pushed", True)

    def test_pushed_and_stored_parks_under_relay_pushed_stored(self):
        report = verdict_report(Verdict("id", None, OTHER_ADDRESS, pushed=True, stored=True))
        assert report == VerdictReport("relay_pushed_stored", True)

    def test_stored_on_message_sent_without_pushed_is_still_a_confirmation(self):
        # `stored` alone on MessageSent says the mailbox also holds a frame
        # that was delivered to a session; nothing is parked for that.
        report = verdict_report(Verdict("id", None, OTHER_ADDRESS, stored=True))
        assert report.confirmed

    def test_stored_delivery_error_is_relay_stored_for_that_id(self):
        report = verdict_report(
            Verdict("id", "recipient_unreachable: asleep", OTHER_ADDRESS, stored=True)
        )
        assert report == VerdictReport("relay_stored", True)

    def test_unreachable_delivery_error_is_verbatim_and_watched(self):
        report = verdict_report(Verdict("id", "recipient_unreachable: no route", OTHER_ADDRESS))
        assert report == VerdictReport("recipient_unreachable: no route", True)

    def test_other_delivery_errors_are_verbatim_and_not_watched(self):
        for reason in ("attach_required", "backbone_timeout", "budget_exceeded"):
            report = verdict_report(Verdict("id", reason, OTHER_ADDRESS))
            assert report == VerdictReport(reason, False)

    def test_recipient_unreachable_is_matched_by_prefix_not_word(self):
        # The core matches the prefix; so does the watch decision.
        report = verdict_report(Verdict("id", "recipient_unreachable", None))
        assert report.watch_recipient


class TestParsePresence:
    def test_a_well_formed_answer(self):
        answer = parse_presence({"peer": OTHER_ADDRESS, "online": True, "last_seen_ms": 5})
        assert answer == PresenceAnswer(OTHER_ADDRESS, True, 5)

    def test_missing_peer_is_ignored(self):
        assert parse_presence({"online": True}) is None
        assert parse_presence({"peer": "", "online": True}) is None

    def test_a_missing_or_non_boolean_online_is_not_readable_as_offline(self):
        assert parse_presence({"peer": OTHER_ADDRESS}) is None
        assert parse_presence({"peer": OTHER_ADDRESS, "online": "false"}) is None
        assert parse_presence({"peer": OTHER_ADDRESS, "online": 0}) is None

    def test_last_seen_is_optional_and_must_be_a_number(self):
        assert parse_presence({"peer": OTHER_ADDRESS, "online": False}).last_seen_ms is None
        assert (
            parse_presence({"peer": OTHER_ADDRESS, "online": False, "last_seen_ms": "9"}).last_seen_ms
            is None
        )
        assert (
            parse_presence({"peer": OTHER_ADDRESS, "online": False, "last_seen_ms": True}).last_seen_ms
            is None
        )
        assert (
            parse_presence({"peer": OTHER_ADDRESS, "online": False, "last_seen_ms": 12.0}).last_seen_ms
            == 12
        )


def test_bounded_reason_cuts_remote_text_to_256_characters():
    assert policy.bounded_reason("x" * 1000) == "x" * 256
    assert policy.bounded_reason("short") == "short"


class TestLoneSurrogates:
    """``json.loads`` accepts ``"\\ud800"`` and hands back a str that cannot
    be UTF-8 encoded, and the FFI refuses it the same way when a string is
    lowered. A gateway can put one in any field; each parser treats it as
    the absence of that field, never as an exception."""

    SURROGATE = "\ud800"

    def test_utf8_bytes_returns_none_for_an_unencodable_string(self):
        assert policy.utf8_bytes("plain") == b"plain"
        assert policy.utf8_bytes("é") == "é".encode("utf-8")
        assert policy.utf8_bytes(self.SURROGATE) is None
        assert policy.utf8_bytes("abc" + self.SURROGATE) is None

    def test_a_json_escape_yields_the_surrogate(self):
        # The way it arrives: an escape the JSON parser accepts.
        assert json.loads('"\\ud800"') == self.SURROGATE

    def test_an_unencodable_capability_token_is_dropped(self):
        tokens = capability_tokens({"tokens": ["gateway_v1", self.SURROGATE, "x"]})
        assert tokens == ["gateway_v1", "x"]

    def test_an_unencodable_reason_is_reported_as_the_bare_failure(self):
        verdict = parse_verdict(
            {"message_id": "id", "recipient": OTHER_ADDRESS, "reason": "bad " + self.SURROGATE},
            "DeliveryError",
        )
        assert verdict == Verdict("id", "DeliveryError", OTHER_ADDRESS)
        assert not verdict_report(verdict).watch_recipient

    def test_an_unencodable_recipient_is_no_recipient(self):
        verdict = parse_verdict(
            {"message_id": "id", "recipient": self.SURROGATE, "reason": "recipient_unreachable"},
            "DeliveryError",
        )
        assert verdict.recipient is None
        assert verdict.reason == "recipient_unreachable"

    def test_an_unencodable_presence_peer_is_ignored(self):
        assert parse_presence({"peer": self.SURROGATE, "online": True}) is None
