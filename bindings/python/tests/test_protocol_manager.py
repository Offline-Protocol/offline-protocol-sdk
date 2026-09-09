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
    # `internet_enabled` defaults to True because the Rust-side validator
    # (offline-protocol/src/config.rs::validate) rejects configs with all
    # transports disabled. Tests that want internet off must enable another
    # transport in the same call (see `test_internet_none_when_disabled`).
    defaults = dict(
        app_id="test-app",
        profile="test-user",
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
    def test_default_stores_use_the_account_namespace(self):
        config = _make_config(app_id="test-app", profile="test-user-1")
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        with (
            patch(
                "offline_protocol_sdk.protocol_manager.SecureStorage"
            ) as secure_storage,
            patch(
                "offline_protocol_sdk.protocol_manager.AppStateStorage"
            ) as state_storage,
        ):
            ProtocolManager(config)

        expected = (
            "account-814873e0cbdb2a1f25f14b31625e7f904cf9923e55b415b91ca4b29b210c12a1"
        )
        secure_storage.assert_called_once_with(namespace=expected)
        state_storage.assert_called_once_with(root=None, namespace=expected)

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


class TestProtocolManagerBroadcast:
    """`send_message("*")` is a BLE-only fan-out convenience.

    BLE rejects a literal "*" peer-ID at the transport layer, so the
    Python wrapper expands "*" into one send per known BLE peer. An
    empty BLE peer set raises ValueError regardless of whether other
    transports are enabled — the wrapper does not attempt
    cross-transport broadcast.
    """

    @pytest.mark.asyncio
    async def test_broadcast_with_no_peers_raises(self):
        config = _make_config(ble_enabled=True, internet_enabled=False)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        try:
            pm._protocol.send_message = MagicMock(return_value="should-not-be-called")
            with pytest.raises(ValueError, match="BLE peers"):
                pm.send_message("*", "hi")
            pm._protocol.send_message.assert_not_called()
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_broadcast_raises_even_when_other_transports_enabled(self):
        # Pins the BLE-only scope: even with Internet/Nostr enabled, the
        # wrapper does not route "*" through them. Callers who want
        # Internet or Nostr broadcast must drive those transports directly.
        config = _make_config(
            ble_enabled=True,
            internet_enabled=True,
            nostr_enabled=True,
        )
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        try:
            pm._protocol.send_message = MagicMock(return_value="should-not-be-called")
            with pytest.raises(ValueError, match="BLE peers"):
                pm.send_message("*", "hi")
            pm._protocol.send_message.assert_not_called()
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_broadcast_with_peers_fans_out(self):
        config = _make_config(ble_enabled=True)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        try:
            assert pm.ble is not None
            with pm.ble._lock:
                pm.ble._peer_device_ids.update({
                    "addr-a": "peer-a",
                    "addr-b": "peer-b",
                })

            call_ids = iter(["id-a", "id-b"])
            pm._protocol.send_message = MagicMock(
                side_effect=lambda **_: next(call_ids)
            )

            last_id = pm.send_message("*", "hi")
            assert pm._protocol.send_message.call_count == 2
            recipients = sorted(
                c.kwargs["recipient"]
                for c in pm._protocol.send_message.call_args_list
            )
            assert recipients == ["peer-a", "peer-b"]
            assert last_id == "id-b"
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_broadcast_deduplicates_peers_across_central_and_peripheral(self):
        # A peer reachable both as a BLE central (discovered by our
        # scanner) and as a peripheral (connected to our advertised
        # GATT) must receive the broadcast exactly once.
        config = _make_config(ble_enabled=True)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        try:
            assert pm.ble is not None
            assert pm.ble_peripheral is not None
            with pm.ble._lock:
                pm.ble._peer_device_ids.update({
                    "addr-a": "peer-shared",
                    "addr-b": "peer-b",
                })
            with pm.ble_peripheral._lock:
                pm.ble_peripheral._central_to_user_id.update({
                    "central-1": "peer-shared",
                    "central-2": "peer-c",
                })

            pm._protocol.send_message = MagicMock(return_value="msg")
            pm.send_message("*", "hi")

            recipients = sorted(
                c.kwargs["recipient"]
                for c in pm._protocol.send_message.call_args_list
            )
            assert recipients == ["peer-b", "peer-c", "peer-shared"]
        finally:
            await pm.stop()


class TestProtocolManagerTransportCallbacks:
    @pytest.mark.asyncio
    async def test_nostr_and_reticulum_callbacks_registered_when_enabled(self):
        config = _make_config(nostr_enabled=True, reticulum_enabled=True)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        pm._protocol.set_nostr_transport_callback = MagicMock()
        pm._protocol.set_reticulum_transport_callback = MagicMock()
        try:
            await pm.start()
            pm._protocol.set_nostr_transport_callback.assert_called_once_with(
                pm._nostr_cb
            )
            pm._protocol.set_reticulum_transport_callback.assert_called_once_with(
                pm._reticulum_cb
            )
            assert pm._nostr_cb in pm._prevent_gc
            assert pm._reticulum_cb in pm._prevent_gc
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_nostr_and_reticulum_callbacks_skipped_when_disabled(self):
        # Matches the RN iOS/Android policy: stubs are not wired when the
        # transport is disabled in config, so apps enabling these manually
        # don't collide with a no-op stub holding the Rust-side slot.
        config = _make_config(nostr_enabled=False, reticulum_enabled=False)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        pm._protocol.set_nostr_transport_callback = MagicMock()
        pm._protocol.set_reticulum_transport_callback = MagicMock()
        try:
            await pm.start()
            pm._protocol.set_nostr_transport_callback.assert_not_called()
            pm._protocol.set_reticulum_transport_callback.assert_not_called()
            assert pm._nostr_cb is None
            assert pm._reticulum_cb is None
        finally:
            await pm.stop()


class TestProtocolManagerTelemetry:
    @pytest.mark.asyncio
    async def test_enable_telemetry_refuses_an_unusable_config_before_anything_starts(self):
        from offline_protocol_sdk.offline_protocol import ProtocolError
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(_make_config())
        await pm.start()
        try:
            assert pm.telemetry_stats() is None
            with pytest.raises(ProtocolError.TelemetryConfigInvalid) as refused:
                pm.enable_telemetry("", "app_1")
            assert "api_key" in str(refused.value)
            with pytest.raises(ProtocolError.TelemetryConfigInvalid) as refused:
                pm.enable_telemetry("mp_key", "app\u00e9")
            assert "app_id" in str(refused.value)
            # Nothing started: the counters stay absent.
            assert pm.telemetry_stats() is None
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_enable_telemetry_fills_the_platform_and_forwards_every_field(self):
        from offline_protocol_sdk.offline_protocol import AppState, TelemetryOs
        from offline_protocol_sdk.protocol_manager import ProtocolManager, _host_platform

        pm = ProtocolManager(_make_config())
        await pm.start()
        try:
            # The FFI call is mocked: a real enable would start the uploader
            # thread against the production ingest.
            pm._protocol.enable_telemetry = MagicMock()
            pm.enable_telemetry(
                "mp_key",
                "app_1",
                "1.2.3",
                debug=True,
                flush_interval_ms=5000,
                max_batch_bytes=4096,
                max_buffered_records=64,
                include_device_id=True,
                scrub_ids=False,
                metrics_cadence_ms=1000,
                routing_diagnostic=True,
                mls_sampling_bypass=True,
                app_state=AppState.BACKGROUND,
            )
            (config, state), _ = pm._protocol.enable_telemetry.call_args
            assert state is AppState.BACKGROUND
            assert config.api_key == "mp_key"
            assert config.app_id == "app_1"
            assert config.app_version == "1.2.3"
            expected_os, expected_major = _host_platform()
            assert config.os is expected_os
            assert config.os in (
                TelemetryOs.MACOS,
                TelemetryOs.LINUX,
                TelemetryOs.WINDOWS,
                TelemetryOs.OTHER,
            )
            assert config.os_major == expected_major
            assert config.debug is True
            assert config.flush_interval_ms == 5000
            assert config.max_batch_bytes == 4096
            assert config.max_buffered_records == 64
            assert config.include_device_id is True
            assert config.scrub_ids is False
            assert config.mls_verbosity is None
            assert config.metrics_cadence_ms == 1000
            assert config.routing_diagnostic is True
            assert config.mls_sampling_bypass is True
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_the_other_calls_pass_through(self):
        from offline_protocol_sdk.offline_protocol import AppState
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(_make_config())
        await pm.start()
        try:
            for name in (
                "flush_telemetry",
                "end_telemetry_session",
                "disable_telemetry",
            ):
                setattr(pm._protocol, name, MagicMock())
                getattr(pm, name)()
                getattr(pm._protocol, name).assert_called_once()
            pm._protocol.notify_app_state = MagicMock()
            pm.notify_app_state(AppState.BACKGROUND)
            pm._protocol.notify_app_state.assert_called_once_with(AppState.BACKGROUND)
            pm._protocol.set_telemetry_enabled = MagicMock()
            pm.set_telemetry_enabled(False)
            pm._protocol.set_telemetry_enabled.assert_called_once_with(False)
            pm._protocol.telemetry_stats = MagicMock(return_value=None)
            assert pm.telemetry_stats() is None
        finally:
            await pm.stop()

    def test_host_platform_names_this_machine(self):
        from offline_protocol_sdk.offline_protocol import TelemetryOs
        from offline_protocol_sdk.protocol_manager import _host_platform, _major

        os_kind, major = _host_platform()
        assert os_kind is not TelemetryOs.IOS
        assert os_kind is not TelemetryOs.ANDROID
        assert 0 <= major <= 65535
        assert _major("14.5") == 14
        assert _major("6.8.0-45-generic") == 6
        assert _major("") == 0
        assert _major("abc") == 0

    @pytest.mark.asyncio
    async def test_stop_disables_telemetry_for_the_final_flush(self):
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(_make_config())
        await pm.start()
        pm._protocol.disable_telemetry = MagicMock()
        await pm.stop()
        pm._protocol.disable_telemetry.assert_called_once()

    @pytest.mark.asyncio
    async def test_telemetry_install_id(self):
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(_make_config())
        # Secure storage is only wired during start(); before that the
        # scrub secret is session-local, so no stable id is exposed.
        assert pm.telemetry_install_id() is None
        await pm.start()
        try:
            install_id = pm.telemetry_install_id()
            assert install_id is not None
            assert len(install_id) == 32
            assert all(c in "0123456789abcdef" for c in install_id)
            # Stable across repeated calls within a session.
            assert pm.telemetry_install_id() == install_id
        finally:
            await pm.stop()
