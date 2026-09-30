"""The custody section and the two custody calls over the Python binding.

Python has no config parser of its own: the generated ``ProtocolConfig``
record carries the section, so what this pins is that the section is there,
that it reaches the core's validation, and that the counters and the erase
come back through the generated class. The behaviour itself is pinned by the
engine's tests; this file is the binding's half of C9.
"""

import pytest

from offline_protocol_sdk import (
    CustodyConfig,
    OfflineProtocol,
    OverflowPolicy,
    ProtocolConfig,
    ProtocolError,
)


def _config(custody: CustodyConfig | None) -> ProtocolConfig:
    return ProtocolConfig(
        app_id="test-app",
        profile="custody-user",
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=True,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=True,
        initial_ttl=3,
        encryption_enabled=True,
        auto_key_exchange=True,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
        custody=custody,
    )


def test_custody_is_absent_by_default_and_the_counters_start_at_zero() -> None:
    # No section at all: the core's default is off, and every counter reads
    # zero, including the refusal counters the acceptance table names.
    proto = OfflineProtocol(_config(None))
    stats = proto.get_custody_stats()
    assert stats.held == 0
    assert stats.held_bytes == 0
    assert stats.accepted == 0
    assert stats.refused_disabled == 0
    assert stats.refused_stranger == 0
    proto.erase_custody()


def test_a_partial_section_reaches_the_core_and_validates() -> None:
    # Every field is optional and an omitted one keeps the core default:
    # enabling custody alone is a valid configuration.
    proto = OfflineProtocol(_config(CustodyConfig(enabled=True)))
    assert proto.get_custody_stats().held == 0


def test_the_core_refuses_a_hold_that_outlives_the_outbox() -> None:
    # The bound is the core's, not the binding's: a hold that outlives the
    # outbox would deliver frames whose sender already reported them failed.
    with pytest.raises(ProtocolError.InvalidConfiguration, match="custody.hold_ms"):
        OfflineProtocol(_config(CustodyConfig(enabled=True, hold_ms=10**15)))


def test_the_core_refuses_a_stranger_tier_set_by_one_dial() -> None:
    with pytest.raises(ProtocolError.InvalidConfiguration, match="stranger"):
        OfflineProtocol(_config(CustodyConfig(enabled=True, stranger_max_entries=4)))


def test_every_counter_the_acceptance_table_names_is_a_field() -> None:
    proto = OfflineProtocol(_config(CustodyConfig(enabled=True)))
    stats = proto.get_custody_stats()
    for name in (
        "refused_disabled",
        "refused_no_request",
        "refused_unknown_class",
        "refused_not_sealed",
        "refused_unproven_peer",
        "refused_not_depositor",
        "duplicates",
        "refused_stranger",
        "refused_depositor_full",
        "refused_store_full",
        "refused_battery",
    ):
        assert getattr(stats, name) == 0
