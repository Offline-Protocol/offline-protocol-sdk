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
