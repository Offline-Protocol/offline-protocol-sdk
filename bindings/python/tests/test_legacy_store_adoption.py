"""Tests for the pre-namespace secure-store adoption policy."""

from __future__ import annotations

from offline_protocol_sdk import legacy_store_adoption

NAMESPACE = "account-" + "a" * 64
OTHER = "account-" + "b" * 64


def test_unclaimed_legacy_store_is_adopted() -> None:
    # The upgrade case: an install that predates namespacing has an unclaimed
    # legacy store, and the account that launches first inherits it. Without
    # this the account would look brand new and mint a fresh MLS identity.
    assert legacy_store_adoption.decide(None, NAMESPACE).kind == "adopt"
    assert legacy_store_adoption.decide("", NAMESPACE).kind == "adopt"


def test_our_own_claim_resumes_read_through() -> None:
    # Adoption must be resumable: a launch after the claim was written but
    # before every entry was promoted still reads through.
    decision = legacy_store_adoption.decide(NAMESPACE, NAMESPACE)

    assert decision.kind == "resume"
    assert decision.allows_read_through


def test_foreign_claim_blocks_read_through() -> None:
    # The legacy store was shared by every account on the install, so only one
    # can inherit it. A second account is genuinely new — but that must be
    # reported, not silently rotated into.
    decision = legacy_store_adoption.decide(OTHER, NAMESPACE)

    assert decision.kind == "conflict"
    assert decision.claimed_by == OTHER
    assert not decision.allows_read_through


def test_adopt_and_resume_allow_read_through() -> None:
    assert legacy_store_adoption.ADOPT.allows_read_through
    assert legacy_store_adoption.RESUME.allows_read_through
    assert not legacy_store_adoption.NONE.allows_read_through


def test_claim_entry_is_never_read_through() -> None:
    # The claim entry is bookkeeping, not key material: promoting it into the
    # new store would make a later account read its own namespace back as an
    # inherited value.
    assert legacy_store_adoption.is_claim_entry(legacy_store_adoption.CLAIM_KEY_TYPE)
    assert not legacy_store_adoption.is_claim_entry("identity")
