"""Tests for ProtocolManager — high-level protocol orchestrator."""

from __future__ import annotations

import asyncio
from unittest.mock import MagicMock, patch

import pytest

from offline_protocol_sdk.offline_protocol import (
    OverflowPolicy,
    ProtocolConfig,
    ProtocolState,
)


def _make_config(**overrides) -> ProtocolConfig:
    defaults = dict(
        app_id="test-app",
        user_id="test-user",
        ble_enabled=False,
        wifi_direct_enabled=False,
        internet_enabled=False,
        reticulum_enabled=False,
        prefer_online=True,
        initial_ttl=3,
        encryption_enabled=False,
        auto_key_exchange=False,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=60000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )
    defaults.update(overrides)
    return ProtocolConfig(**defaults)


class TestProtocolManagerLifecycle:
    @pytest.mark.asyncio
    async def test_start_stop(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        assert pm._running is True
        assert pm._process_task is not None

        await pm.stop()
        assert pm._running is False
        assert pm._process_task is None

    @pytest.mark.asyncio
    async def test_async_context_manager(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        async with ProtocolManager(config) as pm:
            assert pm._running is True
        assert pm._running is False

    @pytest.mark.asyncio
    async def test_double_start_is_noop(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        task = pm._process_task
        await pm.start()  # should not raise or create new task
        assert pm._process_task is task
        await pm.stop()

    @pytest.mark.asyncio
    async def test_double_stop_is_noop(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        await pm.stop()
        await pm.stop()  # should not raise


class TestProtocolManagerEvents:
    @pytest.mark.asyncio
    async def test_event_handler_receives_events(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        events = []
        pm = ProtocolManager(config, event_handler=events.append)
        await pm.start()

        # Manually invoke the callback to test routing
        pm._event_cb.on_event('{"type": "test_event", "data": 123}')

        assert len(events) == 1
        assert events[0]["type"] == "test_event"
        assert events[0]["data"] == 123
        await pm.stop()

    @pytest.mark.asyncio
    async def test_on_event_replaces_handler(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        events_a = []
        events_b = []
        pm = ProtocolManager(config, event_handler=events_a.append)
        await pm.start()

        pm.on_event(events_b.append)
        pm._event_cb.on_event('{"type": "after_swap"}')

        assert len(events_a) == 0
        assert len(events_b) == 1
        await pm.stop()

    @pytest.mark.asyncio
    async def test_event_handler_invalid_json(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        events = []
        pm = ProtocolManager(config, event_handler=events.append)
        await pm.start()

        pm._event_cb.on_event("not json")

        assert len(events) == 1
        assert events[0] == {"raw": "not json"}
        await pm.stop()


class TestProtocolManagerTransports:
    def test_ble_created_when_enabled(self):
        config = _make_config(ble_enabled=True)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        assert pm.ble is not None
        assert pm.ble_peripheral is not None

    def test_ble_none_when_disabled(self):
        config = _make_config(ble_enabled=False)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        assert pm.ble is None
        assert pm.ble_peripheral is None

    def test_internet_created_when_enabled(self):
        config = _make_config(internet_enabled=True)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        assert pm.internet is not None

    def test_internet_none_when_disabled(self):
        config = _make_config(internet_enabled=False)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        assert pm.internet is None


class TestProtocolManagerConvenience:
    @pytest.mark.asyncio
    async def test_send_message(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        try:
            msg_id = pm.send_message("recipient-1", "hello")
            assert isinstance(msg_id, str)
            assert len(msg_id) > 0
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_get_state(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        try:
            state = pm.get_state()
            assert state == ProtocolState.RUNNING
        finally:
            await pm.stop()

    def test_protocol_property(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        assert pm.protocol is pm._protocol
