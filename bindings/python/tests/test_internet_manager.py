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
