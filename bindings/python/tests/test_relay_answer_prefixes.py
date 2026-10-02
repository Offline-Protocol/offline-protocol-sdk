"""Pins the "a relay answer reaches the core unattributed" rule.

Mirrors ``RelayAnswerPrefixesTests.swift`` and ``RelayAnswerPrefixesTest.kt``
against the core's ``RELAY_ANSWER_PREFIXES``. Python used to hold no copy at
all, and the injection used prefixes the registry did not contain (#368): a
frame under an unregistered prefix is refused as unsigned, invisibly.
"""

from __future__ import annotations

from offline_protocol_sdk.relay_answer_prefixes import (
    RELAY_ANSWER_PREFIXES,
    attributable_actor,
    is_relay_answer,
)


def test_the_set_matches_the_core_constant() -> None:
    # Written out, not derived: a prefix here the core does not exempt is
    # dropped as unsigned, and one the core exempts that is missing here is
    # attributed and dropped the same way.
    assert RELAY_ANSWER_PREFIXES == {
        "__GROUP_CREATED__",
        "__GROUP_MEMBER_ADDED__",
        "__GROUP_MEMBER_REMOVED__",
        "__GROUP_INFO__",
        "__USER_GROUPS__",
        "__GROUP_ERROR__",
    }


def test_membership_answers_are_never_attributed() -> None:
    for prefix in ("__GROUP_MEMBER_ADDED__", "__GROUP_MEMBER_REMOVED__"):
        assert attributable_actor(prefix, "alice") is None


def test_every_relay_answer_drops_its_actor() -> None:
    for prefix in RELAY_ANSWER_PREFIXES:
        assert attributable_actor(prefix, "alice") is None
        assert attributable_actor(prefix, None) is None


def test_data_plane_and_peer_prefixes_keep_their_actor() -> None:
    # Scoped, not blanket: `__GROUP_MSG__` is data plane and its attribution
    # is the reachability signal for a relayed sender.
    for prefix in ("__GROUP_MSG__", "__CONN_REQ__", "__MLS_ENC__"):
        assert attributable_actor(prefix, "alice") == "alice"


def test_is_relay_answer_discriminates() -> None:
    assert is_relay_answer("__GROUP_CREATED__")
    assert not is_relay_answer("__GROUP_MSG__")
    assert not is_relay_answer("__GROUP_CREATED__extra")
