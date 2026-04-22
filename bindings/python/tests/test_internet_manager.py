"""Tests for the InternetManager (WebSocket transport)."""

from __future__ import annotations

import asyncio
import json
from unittest.mock import AsyncMock, MagicMock

import pytest

from offline_protocol_sdk.internet_manager import InternetManager
from offline_protocol_sdk.transport_manager import TransportError, TransportState


@pytest.fixture
def mock_protocol() -> MagicMock:
    proto = MagicMock()
    proto.internet_status_changed = MagicMock()
    proto.internet_message_received = MagicMock()
    proto.internet_get_next_message = MagicMock(return_value=None)
    proto.internet_confirm_sent = MagicMock()
    proto.internet_send_failed = MagicMock()
    proto.internet_send_failed_with_reason = MagicMock()
    return proto


class TestInternetManagerSetup:
    def test_initial_state(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        assert mgr.state == TransportState.STOPPED
        assert not mgr.is_available()

    def test_is_available_after_configure(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        mgr.configure(server_url="ws://localhost:8080")
        assert mgr.is_available()

    def test_configure_sets_url(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://a.com")
        assert mgr.is_available()
        assert mgr._server_url == "ws://a.com"

    @pytest.mark.asyncio
    async def test_start_without_url_raises(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        with pytest.raises(TransportError, match="Server URL not configured"):
            await mgr.start()

    def test_get_metrics_defaults(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        m = mgr.get_metrics()
        assert m["bytes_sent"] == 0
        assert m["is_connected"] is False

    def test_auth_token(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        mgr.set_auth_token("my-token")
        assert mgr._auth_token == "my-token"


class TestInternetManagerMessageProcessing:
    def test_handle_incoming_message(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        msg = {
            "type": "MessageReceived",
            "sender": "peer-1",
            "content": "hello",
            "message_id": "msg-123",
        }
        mgr._process_received(json.dumps(msg).encode())
        mock_protocol.internet_message_received.assert_called_once()
        call_args = mock_protocol.internet_message_received.call_args
        assert call_args.kwargs["sender_id"] == "peer-1"

    def test_handle_connection_request(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        msg = {
            "type": "ConnectionRequestReceived",
            "sender": "peer-2",
            "sender_name": "Alice",
        }
        mgr._process_received(json.dumps(msg).encode())
        mock_protocol.internet_message_received.assert_called_once()

    def test_handle_connection_accepted(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        msg = {
            "type": "ConnectionAccepted",
            "accepted_by": "peer-2",
            "accepted_by_name": "Alice",
        }
        mgr._process_received(json.dumps(msg).encode())
        mock_protocol.internet_message_received.assert_called_once()

    def test_handle_connection_rejected(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        msg = {
            "type": "ConnectionRejected",
            "rejected_by": "peer-3",
        }
        mgr._process_received(json.dumps(msg).encode())
        mock_protocol.internet_message_received.assert_called_once()

    def test_handle_unknown_type_no_crash(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        msg = {"type": "SomeFutureMessageType", "data": "test"}
        mgr._process_received(json.dumps(msg).encode())
        mock_protocol.internet_message_received.assert_not_called()

    def test_handle_invalid_json(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        mgr._process_received(b"not json at all")
        mock_protocol.internet_message_received.assert_not_called()


class TestInternetManagerAppId:
    def test_default_app_id(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        assert mgr._app_id == "offline-messenger"

    def test_custom_app_id(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1", app_id="my-app")
        assert mgr._app_id == "my-app"

    def test_custom_app_id_in_internal_messages(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1", app_id="custom-app")
        result = mgr._build_internal_message("sender", "content")
        assert result["app_id"] == "custom-app"

    def test_custom_app_id_in_incoming_message_fallback(self, mock_protocol: MagicMock) -> None:
        """When content is not full-message JSON, the fallback dict uses app_id."""
        mgr = InternetManager(mock_protocol, "dev-1", app_id="custom-app")
        msg = {
            "type": "MessageReceived",
            "sender": "peer-1",
            "content": "plain text",
            "message_id": "msg-1",
        }
        mgr._process_received(json.dumps(msg).encode())
        call_args = mock_protocol.internet_message_received.call_args
        data = json.loads(bytes(call_args.kwargs["data"]))
        assert data["app_id"] == "custom-app"


class TestInternetManagerConnectionClosedGuard:
    @pytest.mark.asyncio
    async def test_double_handle_connection_closed_is_noop(
        self, mock_protocol: MagicMock
    ) -> None:
        """Second call to _handle_connection_closed returns immediately."""
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        mgr._authenticated = True
        mgr._state = TransportState.RUNNING

        await mgr._handle_connection_closed(None)
        assert mgr._connected is False

        # Reset protocol mock to track second call
        mock_protocol.internet_status_changed.reset_mock()

        # Second call should be a no-op (guard fires)
        await mgr._handle_connection_closed(None)
        mock_protocol.internet_status_changed.assert_not_called()

    @pytest.mark.asyncio
    async def test_handle_connection_closed_when_never_connected(
        self, mock_protocol: MagicMock
    ) -> None:
        """Calling on a fresh (never-connected) manager is a no-op."""
        mgr = InternetManager(mock_protocol, "dev-1")
        await mgr._handle_connection_closed(RuntimeError("test"))
        mock_protocol.internet_status_changed.assert_not_called()

    @pytest.mark.asyncio
    async def test_handle_connection_closed_schedules_reconnect_on_initial_failure(
        self, mock_protocol: MagicMock
    ) -> None:
        """When initial connect fails (_connected never set True), reconnect is still scheduled."""
        mgr = InternetManager(
            mock_protocol, "dev-1",
            server_url="ws://x.com",
            auto_reconnect=True,
        )
        mgr._loop = asyncio.get_running_loop()
        # Simulate state after start() calls _connect() which sets STARTING
        mgr._state = TransportState.STARTING
        # _connected and _authenticated remain False (connect failed before handshake)

        await mgr._handle_connection_closed(RuntimeError("connect failed"))

        # Should NOT be stuck in STARTING — should schedule reconnect
        # which transitions to STARTING via _schedule_reconnect
        assert mgr._reconnect_attempts == 1
        assert mgr._state == TransportState.STARTING

        # Clean up the timer handle
        if mgr._reconnect_handle is not None:
            mgr._reconnect_handle.cancel()


class TestInternetManagerLifecycle:
    """Regression guards for the WS teardown path — #2 in the review.

    Before the fix, `_handle_connection_closed` left `self._ws` open and
    `self._recv_task` running; on reconnect `_connect` overwrote
    `self._recv_task` with a new loop, leaving a zombie recv task reading
    from the old socket.
    """

    @pytest.mark.asyncio
    async def test_handle_connection_closed_closes_ws(
        self, mock_protocol: MagicMock
    ) -> None:
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        mgr._authenticated = True
        mgr._state = TransportState.RUNNING

        ws = MagicMock()
        ws.close = AsyncMock()
        mgr._ws = ws

        await mgr._handle_connection_closed(None)

        ws.close.assert_awaited_once()
        assert mgr._ws is None

    @pytest.mark.asyncio
    async def test_handle_connection_closed_cancels_recv_task(
        self, mock_protocol: MagicMock
    ) -> None:
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        mgr._authenticated = True
        mgr._state = TransportState.RUNNING

        async def _stuck() -> None:
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                return

        mgr._recv_task = asyncio.ensure_future(_stuck())
        mgr._poll_task = asyncio.ensure_future(_stuck())
        mgr._ping_task = asyncio.ensure_future(_stuck())

        snapshot = (mgr._recv_task, mgr._poll_task, mgr._ping_task)

        await mgr._handle_connection_closed(None)

        # All three nulled out and cancelled.
        assert mgr._recv_task is None
        assert mgr._poll_task is None
        assert mgr._ping_task is None
        for t in snapshot:
            # Give the loop a tick to propagate cancellation.
            try:
                await asyncio.wait_for(t, timeout=0.2)
            except asyncio.CancelledError:
                pass
            assert t.done()

    @pytest.mark.asyncio
    async def test_handle_connection_closed_self_cancel_safe(
        self, mock_protocol: MagicMock
    ) -> None:
        """When called from inside _recv_task, we must not cancel ourselves."""
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        mgr._authenticated = True
        mgr._state = TransportState.RUNNING

        caught_self_cancel = False

        async def recv_like() -> None:
            nonlocal caught_self_cancel
            try:
                # Simulate recv_loop catching ConnectionClosed and calling
                # _handle_connection_closed from within itself.
                await mgr._handle_connection_closed(None)
            except asyncio.CancelledError:
                caught_self_cancel = True
                raise

        task = asyncio.ensure_future(recv_like())
        mgr._recv_task = task

        await task

        assert not caught_self_cancel
        # recv_task is None by the time _handle_connection_closed finishes,
        # even though we skipped cancelling it.
        assert mgr._recv_task is None

    @pytest.mark.asyncio
    async def test_reconnect_does_not_accumulate_recv_tasks(
        self, mock_protocol: MagicMock
    ) -> None:
        """Two disconnect+reconnect cycles should leave exactly one recv task."""
        mgr = InternetManager(
            mock_protocol, "dev-1",
            server_url="ws://x.com",
            auto_reconnect=True,
        )
        mgr._loop = asyncio.get_running_loop()
        mgr._state = TransportState.RUNNING
        mgr._connected = True
        mgr._authenticated = True

        # First disconnect: recv_task exists, should be cancelled+nulled.
        async def _stuck() -> None:
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                return

        mgr._recv_task = asyncio.ensure_future(_stuck())
        first = mgr._recv_task
        await mgr._handle_connection_closed(None)
        assert mgr._recv_task is None
        try:
            await asyncio.wait_for(first, timeout=0.2)
        except asyncio.CancelledError:
            pass

        # Simulate reconnect installing a fresh recv_task.
        mgr._connected = True
        mgr._recv_task = asyncio.ensure_future(_stuck())
        second = mgr._recv_task
        assert second is not first

        # Second disconnect: same guarantees.
        await mgr._handle_connection_closed(None)
        assert mgr._recv_task is None
        try:
            await asyncio.wait_for(second, timeout=0.2)
        except asyncio.CancelledError:
            pass

        if mgr._reconnect_handle is not None:
            mgr._reconnect_handle.cancel()

    @pytest.mark.asyncio
    async def test_process_received_tasks_are_retained(
        self, mock_protocol: MagicMock
    ) -> None:
        """AuthError / Authenticated dispatch must keep strong refs to tasks."""
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")

        async def noop(*args, **kwargs) -> None:
            await asyncio.sleep(0)

        mgr._safe_handle_authenticated = noop  # type: ignore[assignment]
        mgr._safe_handle_connection_closed = noop  # type: ignore[assignment]

        mgr._process_received(json.dumps({"type": "Authenticated"}).encode())
        assert len(mgr._process_tasks) == 1
        mgr._process_received(json.dumps({"type": "AuthError"}).encode())
        assert len(mgr._process_tasks) == 2

        # Drain — done callbacks discard.
        pending = list(mgr._process_tasks)
        await asyncio.gather(*pending)
        assert len(mgr._process_tasks) == 0

    @pytest.mark.asyncio
    async def test_concurrent_teardown_callers_schedule_single_reconnect(
        self, mock_protocol: MagicMock
    ) -> None:
        """AuthError process-task racing recv-loop ConnectionClosed must
        produce exactly one reconnect, not two orphan timers.

        Before the re-entrancy guard + stale-handle cancel, both callers
        passed the state-machine guard (state == STARTING during reconnect)
        and each called `_schedule_reconnect`, bumping `_reconnect_attempts`
        twice and overwriting `_reconnect_handle` with the second timer
        while the first kept firing.
        """
        mgr = InternetManager(
            mock_protocol, "dev-1",
            server_url="ws://x.com",
            auto_reconnect=True,
        )
        mgr._loop = asyncio.get_running_loop()
        mgr._state = TransportState.RUNNING
        mgr._connected = True
        mgr._authenticated = True

        # `AsyncMock` returns without yielding to the event loop, which would
        # let Task A run to completion before Task B even starts — masking
        # the race. Use a real coroutine that awaits sleep(0) so teardown
        # actually yields during the close, matching the production handshake.
        close_calls = 0

        async def fake_close() -> None:
            nonlocal close_calls
            close_calls += 1
            await asyncio.sleep(0)

        ws = MagicMock()
        ws.close = fake_close
        mgr._ws = ws

        # Fire two teardown callers concurrently — simulates the
        # AuthError-process-task + recv-loop-ConnectionClosed race.
        await asyncio.gather(
            mgr._handle_connection_closed(None),
            mgr._handle_connection_closed(None),
        )

        # The second caller must bail on the re-entrancy flag, so close ran
        # exactly once despite two concurrent callers.
        assert close_calls == 1

        # Exactly one reconnect scheduled, not two.
        assert mgr._reconnect_attempts == 1
        # And the close path ran exactly once — not double-notified.
        mock_protocol.internet_status_changed.assert_called_once_with(
            is_connected=False
        )
        # Re-entrancy flag clears after teardown finishes.
        assert mgr._teardown_in_progress is False

        if mgr._reconnect_handle is not None:
            mgr._reconnect_handle.cancel()

    @pytest.mark.asyncio
    async def test_schedule_reconnect_cancels_stale_handle(
        self, mock_protocol: MagicMock
    ) -> None:
        """Back-to-back `_schedule_reconnect` calls must cancel the prior
        timer so it doesn't orphan-fire alongside the replacement.
        """
        mgr = InternetManager(
            mock_protocol, "dev-1",
            server_url="ws://x.com",
            auto_reconnect=True,
        )
        mgr._loop = asyncio.get_running_loop()
        mgr._state = TransportState.STARTING

        mgr._schedule_reconnect()
        first = mgr._reconnect_handle
        assert first is not None

        mgr._schedule_reconnect()
        second = mgr._reconnect_handle
        assert second is not None
        assert second is not first
        # The first timer must have been cancelled so only `second` will fire.
        assert first.cancelled()

        if mgr._reconnect_handle is not None:
            mgr._reconnect_handle.cancel()


class TestInternetManagerSendMessageTOCTOU:
    @pytest.mark.asyncio
    async def test_send_fails_gracefully_when_ws_becomes_none(
        self, mock_protocol: MagicMock
    ) -> None:
        """If ws becomes None after initial check, send reports failure."""
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        mgr._ws = MagicMock()

        # Simulate disconnection during semaphore wait: set ws to None
        # We replace the semaphore with one that disconnects before yielding
        original_semaphore = mgr._send_semaphore

        class DisconnectingSemaphore:
            async def __aenter__(self_sem):
                mgr._ws = None
                mgr._connected = False
                return self_sem

            async def __aexit__(self_sem, *args):
                pass

        mgr._send_semaphore = DisconnectingSemaphore()

        await mgr._send_message("msg-1", "peer-1", b"hello")

        mock_protocol.internet_send_failed_with_reason.assert_called_once()
        call_args = mock_protocol.internet_send_failed_with_reason.call_args
        assert call_args.kwargs["message_id"] == "msg-1"
        assert "Disconnected" in call_args.kwargs["reason"]
