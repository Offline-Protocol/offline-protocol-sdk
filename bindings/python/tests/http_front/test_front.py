"""The HTTP front end to end: two engines with encryption on, joined by a
loopback peer stream, each served by the local API with a front on top, and
a stub provider application behind the far front.
"""

from __future__ import annotations

import asyncio
import json
import os
import time

import pytest

aiohttp = pytest.importorskip("aiohttp")
from aiohttp import web  # noqa: E402

from offline_protocol_sdk.http_front import FRONT_APP_ID, Aliases, HttpFront  # noqa: E402
from offline_protocol_sdk.offline_protocol import derive_address  # noqa: E402
from offline_protocol_sdk.protocol_manager import ProtocolManager  # noqa: E402

from local_api.conftest import harness, make_config  # noqa: E402,F401
from local_api.test_examples import until  # noqa: E402

ENCRYPTED = dict(
    encryption_enabled=True,
    require_encryption=True,
    auto_key_exchange=True,
    wifi_direct_enabled=True,
    internet_enabled=False,
)
DOMAIN = "offline.protocol.internal"


class Provider:
    """A plain HTTP application: it knows nothing of the SDK."""

    def __init__(self) -> None:
        self.calls: list[dict] = []
        self.port = 0
        self._runner: web.AppRunner | None = None

    async def start(self) -> None:
        app = web.Application()
        app.router.add_route("*", "/big", self._big)
        app.router.add_route("*", "/slow", self._slow)
        app.router.add_route("*", "/{tail:.*}", self._echo)
        self._runner = web.AppRunner(app)
        await self._runner.setup()
        site = web.TCPSite(self._runner, "127.0.0.1", 0)
        await site.start()
        self.port = site._server.sockets[0].getsockname()[1]

    async def stop(self) -> None:
        if self._runner is not None:
            await self._runner.cleanup()

    async def _echo(self, request: web.Request) -> web.Response:
        body = await request.read()
        call = {
            "method": request.method,
            "path": request.path_qs,
            "headers": {k.lower(): v for k, v in request.headers.items()},
            "body": body.decode("utf-8", "replace"),
        }
        self.calls.append(call)
        return web.json_response(call, status=201, headers={"X-Echo": "yes", "Set-Cookie": "never=crossed"})

    async def _big(self, request: web.Request) -> web.Response:
        return web.Response(body=b"x" * (128 * 1024 + 1))

    async def _slow(self, request: web.Request) -> web.Response:
        await asyncio.sleep(30)
        return web.Response(text="late")

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"


class Hosts:
    def __init__(self, harness, tmp_path) -> None:
        self.harness = harness
        self.tmp_path = tmp_path
        self.fronts: list[HttpFront] = []
        self.provider = Provider()
        self.http: aiohttp.ClientSession | None = None

    async def start(self) -> "Hosts":
        self.manager_a = ProtocolManager(make_config(profile="alice", **ENCRYPTED))
        self.manager_a.peer_stream.configure(listen_host="127.0.0.1", listen_port=0)
        self.server_a = await self.harness.server(manager=self.manager_a)
        self.manager_b = ProtocolManager(make_config(profile="bob", **ENCRYPTED))
        self.manager_b.peer_stream.configure(
            listen_host="127.0.0.1", listen_port=0, peers=[f"127.0.0.1:{self.manager_a.peer_stream.listen_port}"]
        )
        self.server_b = await self.harness.server(manager=self.manager_b)
        await until(lambda: self.manager_a.local_address in self.manager_b.peer_stream.connected_peers())
        await until(lambda: self.manager_b.local_address in self.manager_a.peer_stream.connected_peers())
        self.a = await self.front(self.server_a, aliases=Aliases({"bob": self.manager_b.local_address}))
        self.b = await self.front(self.server_b, registry=self.tmp_path / "b-services.json")
        await self.provider.start()
        self.http = aiohttp.ClientSession()
        return self

    async def front(self, server, *, aliases=None, registry=None, token=None) -> HttpFront:
        front = HttpFront(
            socket_path=server.socket_path,
            port=0,
            aliases=aliases,
            registry_path=registry,
            token_path=token,
        )
        await front.start()
        self.fronts.append(front)
        return front

    async def close(self) -> None:
        if self.http is not None:
            await self.http.close()
        for front in self.fronts:
            await front.stop()
        await self.provider.stop()

    async def register(self, front: HttpFront, service: str = "timeofday", callback: str | None = None) -> None:
        async with self.http.put(
            f"http://127.0.0.1:{front.port}/services/{service}",
            json={"callback": callback or self.provider.url, "version": "1.0", "capabilities": {"tz": "utc"}},
        ) as reply:
            assert reply.status == 200, await reply.text()

    async def request(
        self, front: HttpFront, host: str, path: str = "/now?tz=utc", *, method="POST", body=b"{}", headers=None
    ):
        merged = {"Host": host, "Content-Type": "application/json", **(headers or {})}
        async with self.http.request(
            method, f"http://127.0.0.1:{front.port}{path}", data=body, headers=merged
        ) as reply:
            return reply.status, reply.headers.copy(), await reply.read()


@pytest.fixture
async def hosts(harness, tmp_path):
    hosts = Hosts(harness, tmp_path)
    try:
        yield await hosts.start()
    finally:
        await hosts.close()


def host_for(service: str, device: str) -> str:
    return f"{service}.{device}.{DOMAIN}"


async def test_a_request_crosses_sealed_and_comes_back(hosts):
    await hosts.register(hosts.b)
    status, headers, body = await hosts.request(
        hosts.a,
        host_for("timeofday", hosts.manager_b.local_address),
        headers={"X-Trace": "t1", "Cookie": "kept=home", "X-Offline-Protocol-Forged": "no"},
        body=b'{"zone":"utc"}',
    )
    assert status == 201
    assert headers["X-Echo"] == "yes"
    assert "Set-Cookie" not in headers
    seen = json.loads(body)
    assert seen["method"] == "POST"
    assert seen["path"] == "/now?tz=utc"
    assert seen["body"] == '{"zone":"utc"}'
    assert seen["headers"]["x-offline-protocol-sender"] == hosts.manager_a.local_address
    assert seen["headers"]["x-offline-protocol-service"] == "timeofday"
    assert seen["headers"]["x-trace"] == "t1"
    assert "cookie" not in seen["headers"]
    assert "x-offline-protocol-forged" not in seen["headers"]


async def test_an_alias_names_the_device(hosts):
    await hosts.register(hosts.b)
    status, _, body = await hosts.request(hosts.a, host_for("timeofday", "bob"), method="GET", body=None)
    assert status == 201
    assert json.loads(body)["method"] == "GET"


async def test_a_request_to_this_device_is_served_locally(hosts):
    await hosts.register(hosts.a)
    status, _, body = await hosts.request(hosts.a, host_for("timeofday", hosts.manager_a.local_address))
    assert status == 201
    assert json.loads(body)["headers"]["x-offline-protocol-sender"] == hosts.manager_a.local_address


async def test_an_unknown_service_is_a_404_from_the_far_side(hosts):
    status, headers, _ = await hosts.request(hosts.a, host_for("nothing", hosts.manager_b.local_address))
    assert status == 404
    assert headers["X-Offline-Protocol-Error"] == "unknown_service"


@pytest.mark.parametrize(
    "host, status, token",
    [
        (host_for("timeofday", "carol"), 404, "unknown_device"),
        ("a.b.c." + DOMAIN, 400, "bad_host"),
    ],
)
async def test_a_host_that_names_nobody_is_refused_here(hosts, host, status, token):
    got, headers, _ = await hosts.request(hosts.a, host)
    assert (got, headers["X-Offline-Protocol-Error"]) == (status, token)


async def test_a_body_over_the_limit_never_leaves(hosts):
    status, headers, _ = await hosts.request(
        hosts.a, host_for("timeofday", hosts.manager_b.local_address), body=b"x" * (128 * 1024 + 1)
    )
    assert (status, headers["X-Offline-Protocol-Error"]) == (413, "body_too_large")


async def test_a_device_that_never_answers_times_out(hosts):
    nobody = derive_address(list(os.urandom(32)))
    started = time.monotonic()
    status, headers, _ = await hosts.request(
        hosts.a, host_for("timeofday", nobody), headers={"X-Offline-Protocol-Timeout": "1"}
    )
    assert (status, headers["X-Offline-Protocol-Error"]) == (504, "deadline_exceeded")
    assert time.monotonic() - started < 5


async def test_a_callback_that_refuses_is_a_502(hosts):
    await hosts.register(hosts.b, callback="http://127.0.0.1:9")
    status, headers, _ = await hosts.request(hosts.a, host_for("timeofday", hosts.manager_b.local_address))
    assert (status, headers["X-Offline-Protocol-Error"]) == (502, "callback_failed")


async def test_a_response_over_the_limit_is_refused_on_the_far_side(hosts):
    await hosts.register(hosts.b)
    status, headers, _ = await hosts.request(
        hosts.a, host_for("timeofday", hosts.manager_b.local_address), "/big", method="GET", body=None
    )
    assert (status, headers["X-Offline-Protocol-Error"]) == (502, "response_too_large")


async def test_browse_reports_a_provider_from_signed_discovery(hosts):
    await hosts.register(hosts.b)
    async with hosts.http.get(
        f"http://127.0.0.1:{hosts.a.port}/browse/timeofday", headers={"Host": DOMAIN}
    ) as stream:
        assert stream.headers["Content-Type"].startswith("text/event-stream")

        async def first_event():
            kind = None
            async for raw in stream.content:
                line = raw.decode().strip()
                if line.startswith("event: "):
                    kind = line[len("event: "):]
                elif line.startswith("data: "):
                    return kind, json.loads(line[len("data: "):])

        kind, entry = await asyncio.wait_for(first_event(), 15)
    assert kind == "added"
    assert entry["device"] == hosts.manager_b.local_address
    assert entry["alias"] == "bob"
    assert entry["source"] == "mesh"
    assert entry["version"] == "1.0"
    assert entry["capabilities"] == {"tz": "utc"}


async def test_registrations_survive_a_restart_of_the_front(hosts):
    await hosts.register(hosts.b)
    await hosts.b.stop()
    hosts.fronts.remove(hosts.b)
    hosts.b = await hosts.front(hosts.server_b, registry=hosts.tmp_path / "b-services.json")
    async with hosts.http.get(f"http://127.0.0.1:{hosts.b.port}/services") as reply:
        listed = await reply.json()
    assert [s["service"] for s in listed["services"]] == ["timeofday"]
    status, _, _ = await hosts.request(hosts.a, host_for("timeofday", hosts.manager_b.local_address))
    assert status == 201


async def test_a_name_another_local_application_owns_is_a_conflict(hosts):
    other = await hosts.harness.client(hosts.server_b)
    await other.hello("someone-else")
    await other.call("services.register_service", {"service_id": "timeofday", "version": "", "capabilities": {}})
    async with hosts.http.put(
        f"http://127.0.0.1:{hosts.b.port}/services/timeofday", json={"callback": hosts.provider.url}
    ) as reply:
        assert reply.status == 409


async def test_unregistering_makes_the_service_unknown(hosts):
    await hosts.register(hosts.b)
    async with hosts.http.delete(f"http://127.0.0.1:{hosts.b.port}/services/timeofday") as reply:
        assert reply.status == 200
    status, headers, _ = await hosts.request(hosts.a, host_for("timeofday", hosts.manager_b.local_address))
    assert (status, headers["X-Offline-Protocol-Error"]) == (404, "unknown_service")


@pytest.mark.parametrize(
    "doc",
    [{"callback": "ftp://x"}, {"callback": 3}, {"callback": "http://x", "extra": 1}, {"callback": "http://x", "capabilities": {"a": 1}}],
)
async def test_a_malformed_registration_is_refused(hosts, doc):
    async with hosts.http.put(f"http://127.0.0.1:{hosts.b.port}/services/timeofday", json=doc) as reply:
        assert reply.status == 400


async def test_health_reports_the_connection_and_the_address(hosts):
    async with hosts.http.get(f"http://127.0.0.1:{hosts.a.port}/health") as reply:
        doc = await reply.json()
    assert doc["name"] == "offline-protocol-http-front"
    assert doc["connected"] is True
    assert doc["local_address"] == hosts.manager_a.local_address


async def test_with_a_token_every_request_must_carry_it(hosts):
    front = await hosts.front(hosts.server_a, token=hosts.tmp_path / "front.token")
    token = (hosts.tmp_path / "front.token").read_text().strip()
    assert oct((hosts.tmp_path / "front.token").stat().st_mode & 0o777) == "0o600"
    async with hosts.http.get(f"http://127.0.0.1:{front.port}/health") as reply:
        assert reply.status == 401
    async with hosts.http.get(
        f"http://127.0.0.1:{front.port}/health", headers={"X-Offline-Protocol-Token": token}
    ) as reply:
        assert reply.status == 200


def test_off_loopback_the_front_refuses_to_run_without_a_token():
    with pytest.raises(ValueError, match="token"):
        HttpFront(socket_path="/tmp/x.sock", host="0.0.0.0")


async def test_a_plaintext_envelope_is_never_acted_on(hosts):
    """Invariant 2: only what the engine decrypted names its sender."""
    hosts.b._on_message(
        {
            "type": "message_received",
            "app_id": FRONT_APP_ID,
            "encrypted": False,
            "sender": hosts.manager_a.local_address,
            "content": json.dumps(
                {"op": "req", "v": 1, "id": "x", "svc": "timeofday", "m": "GET", "p": "/", "dl": int(time.time()) + 30}
            ),
        }
    )
    assert not hosts.b._tasks
    assert hosts.provider.calls == []


async def test_a_response_from_another_device_is_ignored(hosts):
    loop = asyncio.get_running_loop()
    future = loop.create_future()
    hosts.a._pending["req1"] = (hosts.manager_b.local_address, future)
    hosts.a._on_message(
        {
            "type": "message_received",
            "app_id": FRONT_APP_ID,
            "encrypted": True,
            "sender": derive_address(list(os.urandom(32))),
            "content": json.dumps({"op": "resp", "v": 1, "id": "req1", "s": 200}),
        }
    )
    assert not future.done()
    hosts.a._pending.pop("req1")


@pytest.mark.parametrize("method, path, host", [("POST", "/now", None), ("PUT", "/services/timeofday", "127.0.0.1")])
async def test_a_request_from_a_browser_page_is_refused(hosts, method, path, host):
    """A page can reach a loopback front by DNS rebinding or a wildcard
    route; the Origin header only a browser sends is what tells it apart."""
    await hosts.register(hosts.b)
    calls = len(hosts.provider.calls)
    async with hosts.http.request(
        method,
        f"http://127.0.0.1:{hosts.a.port}{path}",
        json={"callback": "http://127.0.0.1:1"},
        headers={
            "Host": host or host_for("timeofday", hosts.manager_b.local_address),
            "Origin": "http://evil.example",
        },
    ) as reply:
        assert (reply.status, reply.headers["X-Offline-Protocol-Error"]) == (401, "unauthorized")
    assert len(hosts.provider.calls) == calls
    assert hosts.a.registry.get("timeofday") is None


@pytest.mark.parametrize("host, status", [("evil.example:8080", 400), ("localhost:1", 200), ("[::1]:1", 200), (DOMAIN, 200)])
async def test_without_a_token_the_own_endpoints_answer_only_names_that_cannot_be_rebound(hosts, host, status):
    async with hosts.http.get(f"http://127.0.0.1:{hosts.a.port}/health", headers={"Host": host}) as reply:
        assert reply.status == status


async def test_with_a_token_the_own_endpoints_answer_any_host(hosts):
    front = await hosts.front(hosts.server_a, token=hosts.tmp_path / "front.token")
    token = (hosts.tmp_path / "front.token").read_text().strip()
    async with hosts.http.get(
        f"http://127.0.0.1:{front.port}/health", headers={"Host": "front.lan:8080", "X-Offline-Protocol-Token": token}
    ) as reply:
        assert reply.status == 200

