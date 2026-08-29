"""Tests for the InternetManager (WebSocket transport)."""

from __future__ import annotations

import asyncio
import base64
import json
from unittest.mock import AsyncMock, MagicMock

import pytest

from offline_protocol_sdk import address_declaration as policy
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


class TestInternetManagerPollAndSend:
    """Covers _poll_and_send_messages — regression guard for the attribute
    drift that left msg.recipient in place after the UDL renamed the field
    to recipient_id."""

    def _make_outgoing(
        self, message_id: str, recipient_id: str, data: bytes
    ) -> MagicMock:
        msg = MagicMock()
        msg.message_id = message_id
        msg.recipient_id = recipient_id
        msg.data = list(data)
        msg.reply_to_msg = None
        return msg

    @pytest.mark.asyncio
    async def test_poll_and_send_dispatches(
        self, mock_protocol: MagicMock
    ) -> None:
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        msg = self._make_outgoing("msg-1", "peer-1", b"hello")
        mock_protocol.internet_get_next_message = MagicMock(
            side_effect=[msg, None]
        )
        scheduled: list[tuple[str, str, bytes]] = []

        async def fake_send(mid: str, rcpt: str, data: bytes) -> None:
            scheduled.append((mid, rcpt, data))

        mgr._send_message = fake_send  # type: ignore[assignment]

        mgr._poll_and_send_messages()
        pending = list(mgr._send_tasks)
        assert len(pending) == 1
        await asyncio.gather(*pending)
        assert scheduled == [("msg-1", "peer-1", b"hello")]

    @pytest.mark.asyncio
    async def test_poll_and_send_stops_at_none(
        self, mock_protocol: MagicMock
    ) -> None:
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mock_protocol.internet_get_next_message = MagicMock(return_value=None)

        async def fake_send(mid: str, rcpt: str, data: bytes) -> None:
            pass

        mgr._send_message = fake_send  # type: ignore[assignment]
        mgr._poll_and_send_messages()
        assert not mgr._send_tasks

    @pytest.mark.asyncio
    async def test_poll_and_send_respects_concurrency_cap(
        self, mock_protocol: MagicMock
    ) -> None:
        from offline_protocol_sdk.internet_manager import _MAX_CONCURRENT_SENDS

        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")

        # Pre-fill _send_tasks to the cap with a never-completing task.
        async def _stuck() -> None:
            await asyncio.Event().wait()

        try:
            filler = [asyncio.ensure_future(_stuck()) for _ in range(_MAX_CONCURRENT_SENDS)]
            mgr._send_tasks.update(filler)

            mock_protocol.internet_get_next_message = MagicMock(
                return_value=self._make_outgoing("msg-x", "peer-x", b"data")
            )

            async def fake_send(mid: str, rcpt: str, data: bytes) -> None:
                pass

            mgr._send_message = fake_send  # type: ignore[assignment]
            mgr._poll_and_send_messages()

            # No new tasks were added — the filler count is still the cap.
            assert len(mgr._send_tasks) == _MAX_CONCURRENT_SENDS
            assert mock_protocol.internet_get_next_message.call_count == 0
        finally:
            for t in filler:
                t.cancel()
            await asyncio.gather(*filler, return_exceptions=True)


class TestInternetManagerSendFrame:
    @pytest.mark.asyncio
    async def test_send_message_frame_shape(
        self, mock_protocol: MagicMock
    ) -> None:
        """Pins the relay SendMessage frame contract, message_id included —
        the id is what lets a message_id-aware relay dedup push
        notifications across retries of one logical message."""
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        ws = MagicMock()
        ws.send = AsyncMock()
        mgr._ws = ws

        await mgr._send_message("msg-1", "peer-1", b"hello")

        frame = json.loads(ws.send.call_args.args[0])
        assert frame == {
            "type": "SendMessage",
            "recipient": "peer-1",
            "content": "hello",
            "message_id": "msg-1",
        }
        mock_protocol.internet_confirm_sent.assert_called_once_with(
            message_id="msg-1"
        )

    @pytest.mark.asyncio
    async def test_records_in_flight_before_the_wire_write(
        self, mock_protocol: MagicMock
    ) -> None:
        """The in-flight entry must exist BEFORE `await ws.send` returns.

        `await ws.send` can suspend under backpressure and a relay
        MessageSent/DeliveryError for the frame can be processed on the recv
        task before it resumes; the entry has to already be present so the
        exact id resolves rather than best-effort popping a different still-
        stuck one. Mirrors the iOS/Android record-before-write ordering.
        """
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        seen_at_write: list[str] = []

        async def capture(_payload: str) -> None:
            # Snapshot what the tracker holds at the instant of the write.
            seen_at_write.extend(mgr._inflight.drain_recipient("peer-1", mgr._now_ms()))
            # Put it back so the rest of the send path is unaffected.
            mgr._inflight.record_sent("peer-1", "msg-1", mgr._now_ms())

        ws = MagicMock()
        ws.send = AsyncMock(side_effect=capture)
        mgr._ws = ws

        await mgr._send_message("msg-1", "peer-1", b"hello")

        assert seen_at_write == ["msg-1"], (
            "the send must be recorded before `await ws.send`, not after"
        )

    @pytest.mark.asyncio
    async def test_unrecords_in_flight_when_the_write_fails(
        self, mock_protocol: MagicMock
    ) -> None:
        """If `await ws.send` raises, the frame never went out, so its
        optimistic in-flight entry must be taken back — otherwise a later
        recipient-keyed DeliveryError would false-fail a message never sent."""
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        ws = MagicMock()
        ws.send = AsyncMock(side_effect=RuntimeError("socket broke"))
        mgr._ws = ws

        await mgr._send_message("msg-1", "peer-1", b"hello")

        # No residual in-flight entry for the recipient.
        assert mgr._inflight.drain_recipient("peer-1", mgr._now_ms()) == []
        # And the failure was reported for the message.
        mock_protocol.internet_send_failed_with_reason.assert_called()
        assert (
            mock_protocol.internet_send_failed_with_reason.call_args.kwargs["message_id"]
            == "msg-1"
        )

    @pytest.mark.asyncio
    async def test_post_wire_failure_keeps_the_in_flight_entry(
        self, mock_protocol: MagicMock
    ) -> None:
        """A failure AFTER `await ws.send` succeeded (e.g. the internet_confirm_sent
        FFI call raising) must NOT unrecord the entry: the frame is genuinely on
        the wire and a later relay DeliveryError needs it to fast-fail. unrecord
        fires only when the wire write itself never landed."""
        mgr = InternetManager(mock_protocol, "dev-1", server_url="ws://x.com")
        mgr._connected = True
        ws = MagicMock()
        ws.send = AsyncMock()  # the write SUCCEEDS
        mgr._ws = ws
        # ...but the post-write FFI confirm raises.
        mock_protocol.internet_confirm_sent.side_effect = RuntimeError("ffi boom")

        await mgr._send_message("msg-1", "peer-1", b"hello")

        # The entry survives so a later DeliveryError can still fast-fail it.
        assert mgr._inflight.drain_recipient("peer-1", mgr._now_ms()) == ["msg-1"]


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


class TestAddressDeclaration:
    """The relay address handshake, as wired into the manager.

    The signed bytes themselves are pinned in ``test_address_declaration.py``
    against the relay's own hex vector. What is pinned here is the wiring the
    pure policy cannot see: the order the call sites run in, which name gets
    signed, which address is declared, and where the relay's answers land.
    """

    CHALLENGE = bytes(range(32))
    CHALLENGE_B64 = base64.b64encode(CHALLENGE).decode("ascii")
    ADDRESS = "off1q9eqy0ww55qxm8ve0jv8gxpxknay7fkj9veg0swe"

    @pytest.fixture
    def declaring_protocol(self, mock_protocol: MagicMock) -> MagicMock:
        mock_protocol.local_address = MagicMock(return_value=self.ADDRESS)
        mock_protocol.get_identity_public_key = MagicMock(
            return_value=list(bytes(range(32)))
        )
        mock_protocol.sign_data = MagicMock(return_value=list(bytes(range(64))))
        mock_protocol.internet_relay_capabilities = MagicMock()
        mock_protocol.internet_address_declared = MagicMock()
        mock_protocol.internet_address_declaration_refused = MagicMock()
        return mock_protocol

    @staticmethod
    def _manager(proto: MagicMock) -> tuple[InternetManager, AsyncMock]:
        mgr = InternetManager(proto, "dev-1", server_url="ws://x.com")
        ws = AsyncMock()
        mgr._ws = ws
        return mgr, ws

    @staticmethod
    async def _settle(mgr: InternetManager) -> None:
        """Drain dispatch tasks and stop the loops authentication starts."""
        pending = list(mgr._process_tasks)
        if pending:
            await asyncio.gather(*pending)
        loops = [t for t in (mgr._poll_task, mgr._ping_task) if t is not None]
        for task in loops:
            task.cancel()
        if loops:
            await asyncio.gather(*loops, return_exceptions=True)

    def _authenticated(self, **overrides: object) -> bytes:
        frame: dict[str, object] = {
            "type": "Authenticated",
            "user_id": "u-1",
            "username": "alice",
            "capabilities": ["group_delivery_v3", "address_routing_v1"],
            "address_challenge": self.CHALLENGE_B64,
        }
        frame.update(overrides)
        return json.dumps(frame).encode()

    @pytest.mark.asyncio
    async def test_declares_address_before_flipping_status(
        self, declaring_protocol: MagicMock
    ) -> None:
        """The relay attributes each inbound frame by whatever this connection
        has proved when it reads that frame, and never re-stamps retroactively.
        `internet_status_changed(True)` triggers the outbox flush, so anything
        it sends before the declaration lands is attributed by account name for
        good, and its address-stamped `Message.sender` then fails the
        receiver's `validate_transport_sender` at hop 0. Capabilities must
        likewise precede the flip, which reads the group broadcast gate.
        """
        mgr, ws = self._manager(declaring_protocol)
        order: list[str] = []
        ws.send.side_effect = lambda frame: order.append("declare")
        declaring_protocol.internet_relay_capabilities.side_effect = (
            lambda **_: order.append("capabilities")
        )
        declaring_protocol.internet_status_changed.side_effect = (
            lambda **_: order.append("status")
        )

        await mgr._handle_authenticated(
            "u-1", "alice", ["address_routing_v1"], self.CHALLENGE_B64, ws
        )
        await self._settle(mgr)

        assert order == ["declare", "capabilities", "status"]

    @pytest.mark.asyncio
    async def test_declaration_frame_matches_the_relay_contract(
        self, declaring_protocol: MagicMock
    ) -> None:
        mgr, ws = self._manager(declaring_protocol)

        mgr._process_received(self._authenticated())
        await self._settle(mgr)

        frame = json.loads(ws.send.call_args.args[0])
        assert frame["type"] == "DeclareAddress"
        assert frame["address"] == self.ADDRESS
        # The bytes signed are the policy's, over the relay's account name.
        signed = bytes(declaring_protocol.sign_data.call_args.args[0])
        assert signed == policy.proof_payload("alice", self.CHALLENGE)

    @pytest.mark.asyncio
    async def test_declares_local_address_not_a_re_derived_one(
        self, declaring_protocol: MagicMock
    ) -> None:
        """The core's lockstep check compares the relay's `AddressDeclared`
        echo against `local_address()`. Declaring anything else would make this
        node report RELAY_ADDRESS_BINDING_MISMATCH against its own proof."""
        declaring_protocol.local_address.return_value = "off1sentinel"
        mgr, ws = self._manager(declaring_protocol)

        mgr._process_received(self._authenticated())
        await self._settle(mgr)

        assert json.loads(ws.send.call_args.args[0])["address"] == "off1sentinel"

    @pytest.mark.asyncio
    @pytest.mark.parametrize("omit_key", [True, False])
    async def test_never_signs_the_device_id_when_the_relay_sends_no_username(
        self, declaring_protocol: MagicMock, omit_key: bool
    ) -> None:
        """`username` is optional on the relay's `Authenticated`. The relay
        verifies the proof against the name *it* resolved, so signing the local
        device id produces a signature that cannot verify and is
        indistinguishable, in the relay's logs, from an attack.

        Both wire shapes are covered because only one of them can catch the
        defect. The relay declares `username: Option<String>` without
        `skip_serializing_if`, so a nameless account arrives today as an
        explicit null, and `dict.get(key, fallback)` returns None for that
        without ever consulting the fallback. It is the *absent* key that makes
        a `.get("username", self._device_id)` answer with the local profile, so
        a test that only sends null passes against the bug.
        """
        mgr, ws = self._manager(declaring_protocol)
        frame = json.loads(self._authenticated())
        if omit_key:
            del frame["username"]
        else:
            frame["username"] = None

        mgr._process_received(json.dumps(frame).encode())
        await self._settle(mgr)

        declaring_protocol.sign_data.assert_not_called()
        ws.send.assert_not_called()
        # Authentication itself is unaffected.
        declaring_protocol.internet_status_changed.assert_called_once_with(
            is_connected=True
        )

    @pytest.mark.asyncio
    async def test_skips_quietly_when_the_relay_lacks_the_capability(
        self, declaring_protocol: MagicMock
    ) -> None:
        mgr, ws = self._manager(declaring_protocol)

        mgr._process_received(
            self._authenticated(capabilities=[], address_challenge=None)
        )
        await self._settle(mgr)

        ws.send.assert_not_called()
        declaring_protocol.sign_data.assert_not_called()
        assert mgr._authenticated is True
        # The empty list is still injected, so a stale set from a previous
        # relay cannot leak across connections.
        declaring_protocol.internet_relay_capabilities.assert_called_once_with(
            capabilities=[]
        )

    @pytest.mark.asyncio
    async def test_skips_when_mls_is_not_initialized(
        self, declaring_protocol: MagicMock
    ) -> None:
        """An app running with encryption disabled has no identity to prove and
        stays in account-name space by construction. That is a clean skip, not
        a signing failure."""
        declaring_protocol.local_address.return_value = None
        mgr, ws = self._manager(declaring_protocol)

        mgr._process_received(self._authenticated())
        await self._settle(mgr)

        declaring_protocol.sign_data.assert_not_called()
        ws.send.assert_not_called()

    @pytest.mark.asyncio
    async def test_a_malformed_challenge_is_not_signed(
        self, declaring_protocol: MagicMock
    ) -> None:
        mgr, ws = self._manager(declaring_protocol)

        mgr._process_received(
            self._authenticated(
                address_challenge=base64.b64encode(b"\x00" * 16).decode("ascii")
            )
        )
        await self._settle(mgr)

        declaring_protocol.sign_data.assert_not_called()
        ws.send.assert_not_called()

    @pytest.mark.asyncio
    async def test_a_replaced_socket_does_not_inherit_the_challenge(
        self, declaring_protocol: MagicMock
    ) -> None:
        """The challenge is minted per connection. A socket swapped in after
        the frame arrived has its own `Authenticated` coming, so declaring the
        predecessor's challenge on it can only earn a refusal."""
        mgr, ws = self._manager(declaring_protocol)
        successor = AsyncMock()

        mgr._process_received(self._authenticated())
        mgr._ws = successor  # reconnect lands before the spawned task runs
        await self._settle(mgr)

        ws.send.assert_not_called()
        successor.send.assert_not_called()
        declaring_protocol.sign_data.assert_not_called()

    @pytest.mark.asyncio
    async def test_a_failed_declaration_leaves_the_connection_authenticated(
        self, declaring_protocol: MagicMock
    ) -> None:
        """An undeclared connection is a working connection: the relay simply
        attributes it the legacy way. Failing it over a refused declaration
        would turn a degraded path into no path."""
        declaring_protocol.sign_data.side_effect = RuntimeError("no identity")
        mgr, ws = self._manager(declaring_protocol)

        mgr._process_received(self._authenticated())
        await self._settle(mgr)

        ws.send.assert_not_called()
        assert mgr._authenticated is True
        assert mgr.state == TransportState.RUNNING
        declaring_protocol.internet_status_changed.assert_called_once_with(
            is_connected=True
        )

    def test_address_declared_reaches_the_lockstep_check(
        self, declaring_protocol: MagicMock
    ) -> None:
        """The echo goes to a dedicated FFI entry point, where the core
        compares the bound address against `local_address()`. Logging it
        instead would silently disable RELAY_ADDRESS_BINDING_MISMATCH."""
        mgr, _ = self._manager(declaring_protocol)

        mgr._process_received(
            json.dumps({"type": "AddressDeclared", "address": self.ADDRESS}).encode()
        )

        declaring_protocol.internet_address_declared.assert_called_once_with(
            address=self.ADDRESS
        )

    def test_the_refusal_frame_is_address_error(
        self, declaring_protocol: MagicMock
    ) -> None:
        """The relay's refusal variant is `AddressError`, not the name of the
        FFI entry point it feeds. Matching the wrong tag leaves every refusal
        in the unhandled-message branch, so RELAY_ADDRESS_DECLARATION_REFUSED
        never fires."""
        mgr, _ = self._manager(declaring_protocol)

        mgr._process_received(
            json.dumps({"type": "AddressError", "reason": "address_taken"}).encode()
        )

        declaring_protocol.internet_address_declaration_refused.assert_called_once_with(
            reason="address_taken"
        )


# ---------------------------------------------------------------------------
# RecipientInFlightTracker + recipient-keyed DeliveryError correlation.
# Ports the iOS/Android bridge behavior; these pin it per the repo's C9 policy
# ("cargo test proves nothing about the bridges — each binding has its own
# tests"). The highest-risk new logic is the tracker: a subtle error would
# false-fail a delivered message, so the MessageSent-resolve path is tested
# explicitly.
# ---------------------------------------------------------------------------
from offline_protocol_sdk.internet_manager import _RecipientInFlightTracker


class TestRecipientInFlightTracker:
    def test_record_cap_and_fifo(self) -> None:
        t = _RecipientInFlightTracker(ttl_ms=1000, max_per_recipient=3)
        for m in ("m1", "m2", "m3", "m4"):
            t.record_sent("r1", m, 0)
        # oldest dropped at the cap; FIFO order preserved
        assert t.drain_recipient("r1", 0) == ["m2", "m3", "m4"]

    def test_resolve_exact_prevents_false_fail(self) -> None:
        # A delivered frame (relay echoed our id on MessageSent) must be
        # resolved OUT so a later recipient-keyed DeliveryError cannot fail it.
        t = _RecipientInFlightTracker(ttl_ms=10_000)
        t.record_sent("r", "a", 0)
        t.record_sent("r", "b", 0)
        t.resolve_on_relay_accepted("r", "a", 0)  # 'a' delivered
        assert t.drain_recipient("r", 0) == ["b"]  # only 'b' still in flight

    def test_resolve_best_effort_oldest_when_no_id(self) -> None:
        # Older relay: MessageSent carries no matching id -> drop oldest.
        t = _RecipientInFlightTracker(ttl_ms=10_000)
        t.record_sent("r", "a", 0)
        t.record_sent("r", "b", 0)
        t.resolve_on_relay_accepted("r", None, 0)
        assert t.drain_recipient("r", 0) == ["b"]

    def test_ttl_expiry(self) -> None:
        t = _RecipientInFlightTracker(ttl_ms=1000)
        t.record_sent("r", "a", 0)
        assert t.drain_recipient("r", 5000) == []  # older than TTL -> not failed

    def test_prune_and_clear(self) -> None:
        t = _RecipientInFlightTracker(ttl_ms=1000)
        t.record_sent("r", "a", 0)
        t.prune(5000)
        assert t.drain_recipient("r", 0) == []
        t.record_sent("r2", "b", 0)
        t.clear()
        assert t.drain_recipient("r2", 0) == []

    def test_unrecord_undoes_a_failed_send(self) -> None:
        # When `await ws.send` raises, the frame never hit the wire and its
        # optimistic entry must be taken back, or a later recipient-keyed
        # DeliveryError would false-fail a message that was never sent.
        t = _RecipientInFlightTracker(ttl_ms=10_000)
        t.record_sent("r", "a", 0)
        t.record_sent("r", "b", 0)
        t.unrecord("r", "b")  # 'b' failed to send
        assert t.drain_recipient("r", 0) == ["a"]

    def test_unrecord_removes_only_the_newest_matching_id(self) -> None:
        # A retry re-records the same id; unrecording the just-recorded send
        # must leave the older same-id entry intact (FIFO retry semantics).
        t = _RecipientInFlightTracker(ttl_ms=10_000)
        t.record_sent("r", "a", 0)
        t.record_sent("r", "a", 1)
        t.unrecord("r", "a")
        assert t.drain_recipient("r", 0) == ["a"]

    def test_unrecord_missing_is_a_noop(self) -> None:
        t = _RecipientInFlightTracker(ttl_ms=10_000)
        t.record_sent("r", "a", 0)
        t.unrecord("r", "absent")   # id not present
        t.unrecord("nobody", "a")   # recipient not present
        assert t.drain_recipient("r", 0) == ["a"]
        # dropping the last id for a recipient prunes the empty bucket
        t.unrecord("r", "a")
        assert t.drain_recipient("r", 0) == []


class TestDeliveryErrorRecipientKeyed:
    def test_delivery_error_fails_all_in_flight_for_recipient(
        self, mock_protocol: MagicMock
    ) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        # two frames in flight to the same recipient
        mgr._inflight.record_sent("offX", "id-1", mgr._now_ms())
        mgr._inflight.record_sent("offX", "id-2", mgr._now_ms())
        # relay says recipient offline; older relays send no message_id
        mgr._process_received(
            json.dumps({
                "type": "DeliveryError",
                "recipient": "offX",
                "reason": "Recipient is offline",
            }).encode()
        )
        failed = {
            c.kwargs.get("message_id")
            for c in mock_protocol.internet_send_failed_with_reason.call_args_list
        }
        assert failed == {"id-1", "id-2"}
        # every fail is tagged with the core's classification prefix
        for c in mock_protocol.internet_send_failed_with_reason.call_args_list:
            assert c.kwargs.get("reason", "").startswith("recipient_unreachable:")
        # and the peer is fed as offline so the core parks + watches for return
        mock_protocol.internet_peer_presence.assert_called_once()
        assert mock_protocol.internet_peer_presence.call_args.kwargs["peer_id"] == "offX"
        assert mock_protocol.internet_peer_presence.call_args.kwargs["online"] is False

    def test_message_sent_prevents_false_fail_of_delivered(
        self, mock_protocol: MagicMock
    ) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        mgr._inflight.record_sent("offY", "delivered", mgr._now_ms())
        mgr._inflight.record_sent("offY", "stuck", mgr._now_ms())
        # relay confirms 'delivered' was forwarded
        mgr._process_received(
            json.dumps({
                "type": "MessageSent",
                "recipient": "offY",
                "message_id": "delivered",
            }).encode()
        )
        # later DeliveryError for the recipient must NOT fail the delivered one
        mgr._process_received(
            json.dumps({
                "type": "DeliveryError",
                "recipient": "offY",
                "reason": "offline",
            }).encode()
        )
        failed = {
            c.kwargs.get("message_id")
            for c in mock_protocol.internet_send_failed_with_reason.call_args_list
        }
        assert "delivered" not in failed
        assert "stuck" in failed


class TestAdaptiveSendDrain:
    """Pins the drain-until-empty contract the activity-adaptive send loop relies
    on: _poll_and_send_messages must report how many frames it moved so the loop
    can re-drain immediately under load and relax only when the queue is empty.
    """

    @pytest.mark.asyncio
    async def test_poll_returns_drained_count(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        mgr._send_message = AsyncMock()  # type: ignore[method-assign]
        msgs = [MagicMock(message_id=f"m{i}", recipient_id="r", data=b"x") for i in range(3)]
        mock_protocol.internet_get_next_message = MagicMock(side_effect=[*msgs, None])
        drained = mgr._poll_and_send_messages()
        assert drained == 3, "must report the number of frames drained"

    @pytest.mark.asyncio
    async def test_poll_empty_queue_returns_zero(self, mock_protocol: MagicMock) -> None:
        mgr = InternetManager(mock_protocol, "dev-1")
        mgr._send_message = AsyncMock()  # type: ignore[method-assign]
        mock_protocol.internet_get_next_message = MagicMock(return_value=None)
        assert mgr._poll_and_send_messages() == 0, "empty queue drains nothing (loop then relaxes)"
