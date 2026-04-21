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


class TestProtocolManagerBroadcast:
    """Covers `send_message("*")` semantics — #3 in the PR review.

    The previous behaviour silently passed "*" through to the Rust core
    when no BLE peers were known; the core's BLE transport treats "*" as
    a literal peer-ID lookup and fails downstream with no surfaced
    error. We now raise ValueError so callers learn immediately.
    """

    @pytest.mark.asyncio
    async def test_broadcast_with_no_peers_raises(self):
        config = _make_config(ble_enabled=True)
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        pm = ProtocolManager(config)
        await pm.start()
        try:
            pm._protocol.send_message = MagicMock(return_value="should-not-be-called")
            with pytest.raises(ValueError, match="no BLE peers"):
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
            # Pre-populate the ble manager's peer map so _get_known_ble_peers
            # returns deterministic results.
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
            # Returns the id from the final call.
            assert last_id == "id-b"
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
    async def test_install_uninstall_telemetry_sink(self):
        from offline_protocol_sdk.offline_protocol import (
            DeviceCapabilitySnapshot,
            MetricsFrame,
            RoutingDecision,
            TelemetryConfig,
            TelemetrySink,
            TransportStateEvent,
        )
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        class RecordingSink(TelemetrySink):
            def on_protocol_event(self, event_json: str) -> None: ...
            def on_mls_event(self, event_json: str) -> None: ...
            def on_metrics_frame(self, frame: MetricsFrame) -> None: ...
            def on_transport_state(self, event: TransportStateEvent) -> None: ...
            def on_routing_decision(self, decision: RoutingDecision) -> None: ...
            def on_device_capability(
                self, snapshot: DeviceCapabilitySnapshot
            ) -> None: ...
            def on_extension(self, name: str, payload_json: str) -> None: ...

        config = _make_config()
        pm = ProtocolManager(config)
        await pm.start()
        try:
            sink = RecordingSink()
            pm.install_telemetry_sink(sink)
            assert sink in pm._prevent_gc
            assert pm._telemetry_sink is sink

            pm.uninstall_telemetry_sink()
            assert sink not in pm._prevent_gc
            assert pm._telemetry_sink is None

            # Custom TelemetryConfig round-trip
            pm.install_telemetry_sink(
                sink,
                TelemetryConfig(
                    scrub_ids=False,
                    mls_verbosity=None,
                    metrics_cadence_ms=500,
                    routing_diagnostic=True,
                    enable_poll_queue=True,
                ),
            )
            assert pm._telemetry_sink is sink
            pm.uninstall_telemetry_sink()
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_poll_telemetry_frame_passes_through(self):
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        config = _make_config()
        pm = ProtocolManager(config)
        await pm.start()
        try:
            pm._protocol.poll_telemetry_frame = MagicMock(
                return_value='{"kind":"metrics"}'
            )
            assert pm.poll_telemetry_frame() == '{"kind":"metrics"}'
            pm._protocol.poll_telemetry_frame.assert_called_once()
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_reinstall_releases_prior_sink_pin(self):
        # Re-installing must not leave the previous sink pinned in
        # _prevent_gc. Rust drops its old handle inside install_telemetry_sink,
        # so the Python pin would be the only remaining strong reference.
        from offline_protocol_sdk.offline_protocol import (
            DeviceCapabilitySnapshot,
            MetricsFrame,
            RoutingDecision,
            TelemetrySink,
            TransportStateEvent,
        )
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        class NoopSink(TelemetrySink):
            def on_protocol_event(self, event_json: str) -> None: ...
            def on_mls_event(self, event_json: str) -> None: ...
            def on_metrics_frame(self, frame: MetricsFrame) -> None: ...
            def on_transport_state(self, event: TransportStateEvent) -> None: ...
            def on_routing_decision(self, decision: RoutingDecision) -> None: ...
            def on_device_capability(
                self, snapshot: DeviceCapabilitySnapshot
            ) -> None: ...
            def on_extension(self, name: str, payload_json: str) -> None: ...

        config = _make_config()
        pm = ProtocolManager(config)
        await pm.start()
        try:
            sink_a = NoopSink()
            sink_b = NoopSink()

            pm.install_telemetry_sink(sink_a)
            assert pm._telemetry_sink is sink_a
            assert sink_a in pm._prevent_gc

            pm.install_telemetry_sink(sink_b)
            assert pm._telemetry_sink is sink_b
            assert sink_b in pm._prevent_gc
            assert sink_a not in pm._prevent_gc  # prior sink released

            pm.uninstall_telemetry_sink()
            assert pm._telemetry_sink is None
            assert sink_a not in pm._prevent_gc
            assert sink_b not in pm._prevent_gc
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_uninstall_is_idempotent(self):
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        config = _make_config()
        pm = ProtocolManager(config)
        await pm.start()
        try:
            # Uninstall with no prior install is a no-op.
            pm.uninstall_telemetry_sink()
            assert getattr(pm, "_telemetry_sink", None) is None

            # Double uninstall after install is also a no-op on the second call.
            pm._protocol.uninstall_telemetry_sink = MagicMock()
            pm._telemetry_sink = object()  # stand-in; won't be passed to Rust
            pm._prevent_gc.append(pm._telemetry_sink)

            pm.uninstall_telemetry_sink()
            assert pm._telemetry_sink is None
            pm.uninstall_telemetry_sink()  # must not raise
            assert pm._telemetry_sink is None
            assert pm._protocol.uninstall_telemetry_sink.call_count == 2
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_enable_poll_queue_false_is_passed_through(self):
        from offline_protocol_sdk.offline_protocol import (
            DeviceCapabilitySnapshot,
            MetricsFrame,
            RoutingDecision,
            TelemetryConfig,
            TelemetrySink,
            TransportStateEvent,
        )
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        class NoopSink(TelemetrySink):
            def on_protocol_event(self, event_json: str) -> None: ...
            def on_mls_event(self, event_json: str) -> None: ...
            def on_metrics_frame(self, frame: MetricsFrame) -> None: ...
            def on_transport_state(self, event: TransportStateEvent) -> None: ...
            def on_routing_decision(self, decision: RoutingDecision) -> None: ...
            def on_device_capability(
                self, snapshot: DeviceCapabilitySnapshot
            ) -> None: ...
            def on_extension(self, name: str, payload_json: str) -> None: ...

        config = _make_config()
        pm = ProtocolManager(config)
        await pm.start()
        try:
            pm._protocol.install_telemetry_sink = MagicMock()
            sink = NoopSink()
            pm.install_telemetry_sink(
                sink,
                TelemetryConfig(
                    scrub_ids=None,
                    mls_verbosity=None,
                    metrics_cadence_ms=None,
                    routing_diagnostic=None,
                    enable_poll_queue=False,
                ),
            )
            pm._protocol.install_telemetry_sink.assert_called_once()
            _, passed_config = pm._protocol.install_telemetry_sink.call_args.args
            assert passed_config.enable_poll_queue is False
        finally:
            await pm.stop()

    @pytest.mark.asyncio
    async def test_stop_uninstalls_telemetry_sink(self):
        # stop() must detach the sink so Rust drops its handle and the pull
        # queue drains; otherwise the sink leaks for the lifetime of the
        # ProtocolManager.
        from offline_protocol_sdk.offline_protocol import (
            DeviceCapabilitySnapshot,
            MetricsFrame,
            RoutingDecision,
            TelemetrySink,
            TransportStateEvent,
        )
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        class NoopSink(TelemetrySink):
            def on_protocol_event(self, event_json: str) -> None: ...
            def on_mls_event(self, event_json: str) -> None: ...
            def on_metrics_frame(self, frame: MetricsFrame) -> None: ...
            def on_transport_state(self, event: TransportStateEvent) -> None: ...
            def on_routing_decision(self, decision: RoutingDecision) -> None: ...
            def on_device_capability(
                self, snapshot: DeviceCapabilitySnapshot
            ) -> None: ...
            def on_extension(self, name: str, payload_json: str) -> None: ...

        config = _make_config()
        pm = ProtocolManager(config)
        await pm.start()
        sink = NoopSink()
        pm.install_telemetry_sink(sink)
        pm._protocol.uninstall_telemetry_sink = MagicMock()

        await pm.stop()

        pm._protocol.uninstall_telemetry_sink.assert_called_once()
        assert pm._telemetry_sink is None

    @pytest.mark.asyncio
    async def test_install_failure_preserves_prior_pin(self):
        # Pin-ordering regression guard: if the Rust-side install raises,
        # the previous sink must remain pinned (Rust still holds it) and
        # the new sink must NOT be pinned (Rust never installed it).
        from offline_protocol_sdk.offline_protocol import (
            DeviceCapabilitySnapshot,
            MetricsFrame,
            ProtocolError,
            RoutingDecision,
            TelemetrySink,
            TransportStateEvent,
        )
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        class NoopSink(TelemetrySink):
            def on_protocol_event(self, event_json: str) -> None: ...
            def on_mls_event(self, event_json: str) -> None: ...
            def on_metrics_frame(self, frame: MetricsFrame) -> None: ...
            def on_transport_state(self, event: TransportStateEvent) -> None: ...
            def on_routing_decision(self, decision: RoutingDecision) -> None: ...
            def on_device_capability(
                self, snapshot: DeviceCapabilitySnapshot
            ) -> None: ...
            def on_extension(self, name: str, payload_json: str) -> None: ...

        config = _make_config()
        pm = ProtocolManager(config)
        await pm.start()
        try:
            prev_sink = NoopSink()
            new_sink = NoopSink()

            pm.install_telemetry_sink(prev_sink)
            assert pm._telemetry_sink is prev_sink
            assert prev_sink in pm._prevent_gc

            pm._protocol.install_telemetry_sink = MagicMock(
                side_effect=ProtocolError.LockPoisoned()
            )
            with pytest.raises(ProtocolError.LockPoisoned):
                pm.install_telemetry_sink(new_sink)

            # Previous sink stays the live one; new sink is unpinned.
            assert pm._telemetry_sink is prev_sink
            assert prev_sink in pm._prevent_gc
            assert new_sink not in pm._prevent_gc
        finally:
            # Avoid invoking the mocked uninstall path during teardown.
            pm._protocol.uninstall_telemetry_sink = MagicMock()
            await pm.stop()

    @pytest.mark.asyncio
    async def test_stop_retains_pins_when_uninstall_raises(self):
        # If Rust-side teardown reports an error, ProtocolManager must
        # leave _prevent_gc populated — clearing it would drop the last
        # strong reference to objects Rust may still hold pointers to.
        from offline_protocol_sdk.offline_protocol import (
            DeviceCapabilitySnapshot,
            MetricsFrame,
            ProtocolError,
            RoutingDecision,
            TelemetrySink,
            TransportStateEvent,
        )
        from offline_protocol_sdk.protocol_manager import ProtocolManager

        class NoopSink(TelemetrySink):
            def on_protocol_event(self, event_json: str) -> None: ...
            def on_mls_event(self, event_json: str) -> None: ...
            def on_metrics_frame(self, frame: MetricsFrame) -> None: ...
            def on_transport_state(self, event: TransportStateEvent) -> None: ...
            def on_routing_decision(self, decision: RoutingDecision) -> None: ...
            def on_device_capability(
                self, snapshot: DeviceCapabilitySnapshot
            ) -> None: ...
            def on_extension(self, name: str, payload_json: str) -> None: ...

        config = _make_config()
        pm = ProtocolManager(config)
        await pm.start()
        sink = NoopSink()
        pm.install_telemetry_sink(sink)
        pinned_before = list(pm._prevent_gc)
        assert sink in pinned_before

        pm._protocol.uninstall_telemetry_sink = MagicMock(
            side_effect=ProtocolError.LockPoisoned()
        )

        await pm.stop()

        # Pins retained because Rust-side teardown didn't confirm release.
        assert pm._prevent_gc == pinned_before
        assert sink in pm._prevent_gc
