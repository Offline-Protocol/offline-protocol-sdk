"""Tests for the application-level service wrappers.

The generated ``MeshServices`` is a mock: what is under test is the copy of
the registry this side keeps, the order in which the engine and the copy
change, and the closed status set.
"""

from __future__ import annotations

import subprocess
import sys
from unittest.mock import MagicMock

import pytest

from offline_protocol_sdk.services import (
    SOURCE_LAN,
    SOURCE_LOCAL,
    VALID_STATUSES,
    ServiceRecord,
    Services,
)


class Recorder:
    """A listener that writes down what it hears, in order."""

    def __init__(self) -> None:
        self.calls: list[tuple[str, ServiceRecord]] = []

    def on_registered(self, record: ServiceRecord) -> None:
        self.calls.append(("registered", record))

    def on_unregistered(self, record: ServiceRecord) -> None:
        self.calls.append(("unregistered", record))


@pytest.fixture
def mesh() -> MagicMock:
    m = MagicMock()
    m.register_service = MagicMock(return_value=None)
    m.unregister_service = MagicMock(return_value=True)
    m.discover_services = MagicMock(return_value="query-1")
    m.send_service_request = MagicMock(return_value="request-1")
    m.respond_to_service_request = MagicMock(return_value="message-1")
    return m


@pytest.fixture
def services(mesh: MagicMock) -> Services:
    return Services(MagicMock(), mesh_services=mesh)


class TestLiterals:
    def test_the_status_set_is_the_engines_three(self):
        # As literals, not read from the module: a test that read them would
        # agree with any edit. The engine's VALID_SERVICE_STATUSES.
        assert VALID_STATUSES == ("ok", "not_found", "error")

    def test_the_sources(self):
        assert SOURCE_LOCAL == "local"
        assert SOURCE_LAN == "lan"


class TestRecord:
    def test_capabilities_are_copied(self):
        caps = {"format": "json"}
        record = ServiceRecord("weather.v1", "2.0", caps)
        caps["format"] = "xml"
        assert record.capabilities == {"format": "json"}
        assert record.is_local
        assert record.provider is None

    def test_a_lan_record_is_not_local(self):
        record = ServiceRecord("weather.v1", provider="off1abc", source=SOURCE_LAN)
        assert not record.is_local


class TestRegistry:
    def test_register_reaches_the_engine_first_and_then_the_copy(self, services, mesh):
        listener = Recorder()
        services.add_listener(listener)
        record = services.register("weather.v1", "2.0", {"format": "json"})
        mesh.register_service.assert_called_once_with("weather.v1", "2.0", {"format": "json"})
        assert services.registered() == [record]
        assert services.get("weather.v1") == record
        assert listener.calls == [("registered", record)]

    def test_an_engine_refusal_leaves_the_copy_untouched(self, services, mesh):
        listener = Recorder()
        services.add_listener(listener)
        mesh.register_service.side_effect = RuntimeError("refused")
        with pytest.raises(RuntimeError):
            services.register("__reserved")
        assert services.registered() == []
        assert listener.calls == []

    def test_registering_again_replaces_and_says_so(self, services):
        listener = Recorder()
        services.add_listener(listener)
        first = services.register("weather.v1", "1.0")
        second = services.register("weather.v1", "2.0")
        assert services.registered() == [second]
        assert listener.calls == [
            ("registered", first),
            ("unregistered", first),
            ("registered", second),
        ]

    def test_unregister_returns_the_engines_answer_and_drops_the_copy(self, services, mesh):
        listener = Recorder()
        record = services.register("weather.v1")
        services.add_listener(listener)
        assert services.unregister("weather.v1") is True
        mesh.unregister_service.assert_called_once_with("weather.v1")
        assert services.registered() == []
        assert listener.calls == [("unregistered", record)]

    def test_unregister_of_an_id_the_engine_lost_still_drops_the_copy(self, services, mesh):
        services.register("weather.v1")
        mesh.unregister_service.return_value = False
        assert services.unregister("weather.v1") is False
        assert services.registered() == []

    def test_unregister_of_an_unknown_id_notifies_nobody(self, services, mesh):
        listener = Recorder()
        services.add_listener(listener)
        mesh.unregister_service.return_value = False
        assert services.unregister("nothing") is False
        assert listener.calls == []

    def test_registration_order_is_kept(self, services):
        a = services.register("a")
        b = services.register("b")
        c = services.register("c")
        assert services.registered() == [a, b, c]

    def test_a_failing_listener_does_not_undo_the_registration(self, services, caplog):
        broken = MagicMock()
        broken.on_registered.side_effect = RuntimeError("boom")
        services.add_listener(broken)
        record = services.register("weather.v1")
        assert services.registered() == [record]
        assert "failed in on_registered" in caplog.text

    def test_a_listener_is_added_once_and_can_leave(self, services):
        listener = Recorder()
        services.add_listener(listener)
        services.add_listener(listener)
        services.register("a")
        assert len(listener.calls) == 1
        services.remove_listener(listener)
        services.remove_listener(listener)
        services.register("b")
        assert len(listener.calls) == 1


class TestCalls:
    def test_discover_and_request_delegate(self, services, mesh):
        assert services.discover() == "query-1"
        mesh.discover_services.assert_called_once_with(None)
        assert services.discover("weather.v1") == "query-1"
        assert services.request("off1peer", "weather.v1", "get", "{}") == "request-1"
        mesh.send_service_request.assert_called_once_with("off1peer", "weather.v1", "get", "{}")

    @pytest.mark.parametrize("status", ["ok", "not_found", "error"])
    def test_a_valid_status_crosses(self, services, mesh, status):
        assert services.respond("r-1", "off1peer", "weather.v1", status, "{}") == "message-1"
        mesh.respond_to_service_request.assert_called_once_with("r-1", "off1peer", "weather.v1", status, "{}")

    @pytest.mark.parametrize("status", ["OK", "success", "", "not-found"])
    def test_a_status_the_engine_would_refuse_is_refused_here_with_the_reason(self, services, mesh, status):
        with pytest.raises(ValueError, match="ok, not_found, error"):
            services.respond("r-1", "off1peer", "weather.v1", status, "{}")
        mesh.respond_to_service_request.assert_not_called()


class TestBaseInstall:
    def test_the_wrappers_import_without_zeroconf(self):
        # The base install has no zeroconf; both modules must import, and
        # only the responder's constructor may say what is missing. In a
        # fresh interpreter, because blocking the import here would reload
        # the modules under every other test.
        script = (
            "import sys\n"
            "sys.modules['zeroconf'] = None\n"
            "sys.modules['zeroconf.asyncio'] = None\n"
            "from offline_protocol_sdk import services, dnssd_bridge\n"
            "try:\n"
            "    dnssd_bridge.ZeroconfBackend()\n"
            "except ImportError as exc:\n"
            "    assert 'offline-protocol-sdk[lan]' in str(exc), exc\n"
            "    print('refused')\n"
            "else:\n"
            "    raise SystemExit('the responder was built without zeroconf')\n"
        )
        done = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, timeout=120)
        assert done.returncode == 0, done.stderr
        assert done.stdout.strip() == "refused"
