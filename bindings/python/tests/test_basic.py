"""Core protocol smoke tests."""

from __future__ import annotations

import pytest

from offline_protocol_sdk.offline_protocol import (
    OfflineProtocol,
    OverflowPolicy,
    ProtocolConfig,
    ProtocolState,
)

from conftest import InMemoryStorage


def test_create_protocol(default_config: ProtocolConfig) -> None:
    """Protocol can be instantiated and starts in STOPPED state."""
    proto = OfflineProtocol(default_config)
    assert proto.get_state() == ProtocolState.STOPPED


def test_start_stop_lifecycle(default_config: ProtocolConfig) -> None:
    """Protocol transitions STOPPED -> RUNNING -> STOPPED."""
    proto = OfflineProtocol(default_config)
    assert proto.get_state() == ProtocolState.STOPPED

    proto.start()
    assert proto.get_state() == ProtocolState.RUNNING

    proto.stop()
    assert proto.get_state() == ProtocolState.STOPPED


def test_initialize_mls(default_config: ProtocolConfig) -> None:
    """MLS can be initialised with an in-memory storage provider."""
    proto = OfflineProtocol(default_config)
    storage = InMemoryStorage()
    proto.initialize_mls(storage)
    # Should not raise; MLS is now ready


def test_process_does_not_crash(default_config: ProtocolConfig) -> None:
    """Calling process() on a running protocol should not raise."""
    proto = OfflineProtocol(default_config)
    proto.start()
    try:
        proto.process()
    finally:
        proto.stop()


def test_receive_message_returns_none_when_empty(
    default_config: ProtocolConfig,
) -> None:
    """receive_message() returns None when the inbox is empty."""
    proto = OfflineProtocol(default_config)
    proto.start()
    try:
        assert proto.receive_message() is None
    finally:
        proto.stop()


def test_poll_event_returns_none_initially(
    default_config: ProtocolConfig,
) -> None:
    """poll_event() returns None when no events have been emitted."""
    proto = OfflineProtocol(default_config)
    assert proto.poll_event() is None
