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
        internet_enabled=True,
        reticulum_enabled=False,
        nostr_enabled=False,
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
        events.clear()  # drop anything the process loop delivered during start()

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
        events_a.clear()

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
        events.clear()

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
        config = _make_config(internet_enabled=False, ble_enabled=True)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        assert pm.internet is None

    def test_internet_inherits_app_id(self):
        config = _make_config(internet_enabled=True, app_id="custom-app")
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        assert pm.internet is not None
        assert pm.internet._app_id == "custom-app"


class TestProtocolManagerMessageDrain:
    @pytest.mark.asyncio
    async def test_drain_dispatches_messages_to_event_handler(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        events = []
        pm = ProtocolManager(config, event_handler=events.append)
        await pm.start()
        events.clear()

        # Simulate protocol returning a message then None
        msg_json = '{"sender": "alice", "content": "hi"}'
        pm._protocol.receive_message = MagicMock(side_effect=[msg_json, None])

        pm._drain_incoming_messages()

        assert len(events) == 1
        assert events[0]["sender"] == "alice"
        assert events[0]["content"] == "hi"
        assert events[0].get("type") == "message_received"
        await pm.stop()

    @pytest.mark.asyncio
    async def test_drain_without_handler_does_not_crash(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config, event_handler=None)
        await pm.start()

        pm._protocol.receive_message = MagicMock(side_effect=["{}",  None])

        # Should not raise
        pm._drain_incoming_messages()
        await pm.stop()

    @pytest.mark.asyncio
    async def test_drain_handles_invalid_json(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        events = []
        pm = ProtocolManager(config, event_handler=events.append)
        await pm.start()
        events.clear()

        pm._protocol.receive_message = MagicMock(
            side_effect=["not-json", None]
        )
        pm._drain_incoming_messages()

        assert len(events) == 1
        assert events[0]["type"] == "message_received"
        assert events[0]["raw"] == "not-json"
        await pm.stop()

    @pytest.mark.asyncio
    async def test_drain_caps_at_max_messages(self):
        config = _make_config()
        from offline_protocol_sdk.protocol_manager import ProtocolManager
        from offline_protocol_sdk.protocol_manager import _MAX_MESSAGES_PER_TICK

        events = []
        pm = ProtocolManager(config, event_handler=events.append)
        await pm.start()
        events.clear()

        # Return messages forever (more than the cap)
        pm._protocol.receive_message = MagicMock(return_value='{"type":"msg"}')

        pm._drain_incoming_messages()

        assert len(events) == _MAX_MESSAGES_PER_TICK
        await pm.stop()


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
