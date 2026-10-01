"""The server over a real engine: the session, the framing, the refusals,
the carriers and the three server-side rules."""

from __future__ import annotations

import asyncio
import base64
import json
import os
import stat

import pytest
import websockets
from websockets.asyncio.client import connect

from offline_protocol_sdk.local_api import (
    API_VERSION,
    MAX_MESSAGE_SIZE,
    SERVER_NAME,
    LocalApiServer,
    Policy,
)
from offline_protocol_sdk.local_api import codec
from offline_protocol_sdk.local_api.server import FILE_SIZE_LIMIT
from offline_protocol_sdk.protocol_manager import ProtocolManager

from .conftest import RpcFailure, make_config


def code_of(variant: str) -> int:
    return codec.taxonomy_error(variant, "").code


# -- the session ---------------------------------------------------------------


async def test_hello_returns_the_server_and_the_shared_identity(harness):
    server = await harness.server()
    client = await harness.client(server)
    result = await client.hello("notes", client="tests/1")
    assert result["api_version"] == API_VERSION
    assert result["server"]["name"] == SERVER_NAME
    assert result["state"] == "Running"
    assert result["local_address"] == server.manager.local_address
    assert result["local_address"].startswith("off1")
    second = await harness.client(server)
    assert (await second.hello("other"))["local_address"] == result["local_address"]


async def test_methods_before_hello_are_invalid_state_except_the_instance_less_ones(harness):
    server = await harness.server()
    client = await harness.client(server)
    with pytest.raises(RpcFailure) as err:
        await client.call("get_state")
    assert err.value.variant == "InvalidState" and err.value.code == code_of("InvalidState")
    with pytest.raises(RpcFailure) as err:
        await client.call("subscribe", {"types": "all"})
    assert err.value.variant == "InvalidState"
    key = base64.b64encode(bytes(32)).decode()
    # Reached the engine, not the session gate.
    assert (await client.call("derive_address", {"public_key": key})).startswith("off1")
    with pytest.raises(RpcFailure) as err:
        await client.call("parse_invite", {"blob": "not an invite"})
    assert err.value.variant == "MlsError"
    with pytest.raises(RpcFailure) as err:
        await client.call("verify_identity_assertion", {"assertion": base64.b64encode(b"abc").decode()})
    assert err.value.variant == "MlsError"


async def test_a_second_hello_and_a_bad_app_id_are_refused(harness):
    server = await harness.server()
    client = await harness.client(server)
    await client.hello("notes")
    with pytest.raises(RpcFailure) as err:
        await client.hello("notes")
    assert err.value.variant == "InvalidState"
    for bad in ("", ".", "..", "a/b", "a:b", "a\\b", "a\x01b", "x" * 257):
        fresh = await harness.client(server)
        with pytest.raises(RpcFailure) as err:
            await fresh.hello(bad)
        assert err.value.variant == "InvalidArgument", bad
    with pytest.raises(RpcFailure) as err:
        await (await harness.client(server)).call("hello", {"app_id": "x", "extra": 1})
    assert err.value.code == codec.INVALID_PARAMS


# -- framing -------------------------------------------------------------------


async def test_framing_refusals_use_the_standard_codes(harness):
    server = await harness.server()
    client = await harness.client(server)
    await client.hello("notes")
    positional = await client.call_raw({"jsonrpc": "2.0", "id": 7, "method": "get_state", "params": []})
    assert positional["error"]["code"] == codec.INVALID_PARAMS
    missing = await client.call_raw({"jsonrpc": "2.0", "id": 8, "method": "send_message", "params": {"recipient": "x"}})
    assert missing["error"]["code"] == codec.INVALID_PARAMS and "requires content" in missing["error"]["message"]
    unknown_param = await client.call_raw({"jsonrpc": "2.0", "id": 9, "method": "get_state", "params": {"x": 1}})
    assert unknown_param["error"]["code"] == codec.INVALID_PARAMS
    wrong_type = await client.call_raw(
        {"jsonrpc": "2.0", "id": 10, "method": "send_message", "params": {"recipient": "x", "content": "c", "priority": 3}}
    )
    assert wrong_type["error"]["code"] == codec.INVALID_PARAMS
    with pytest.raises(RpcFailure) as err:
        await client.call("no_such_method")
    assert err.value.code == codec.METHOD_NOT_FOUND
    # A platform operation gets the same answer as an unknown name.
    for platform_op in ("process", "receive_message", "stop", "data.wipe_all", "ble_peer_lost"):
        with pytest.raises(RpcFailure) as err:
            await client.call(platform_op)
        assert err.value.code == codec.METHOD_NOT_FOUND, platform_op
    # Values the decoders did not foresee are refusals on an open
    # connection, never a closed socket: a lone surrogate is valid JSON and
    # not valid UTF-8, and an integer JSON spells but a double cannot hold.
    surrogate = await harness.client(server)
    with pytest.raises(RpcFailure) as err:
        await surrogate.call("hello", {"app_id": "\ud800"})
    assert err.value.variant == "InvalidArgument"
    assert (await surrogate.hello("fine"))["api_version"] == API_VERSION
    huge = await client.call_raw(
        {
            "jsonrpc": "2.0",
            "id": 11,
            "method": "data.counter_increment",
            "params": {"space_id": "s", "doc_id": "d", "collection": "c", "amount": int("9" * 400)},
        }
    )
    assert huge["error"]["code"] == codec.INVALID_PARAMS
    assert await client.call("get_state") == "Running"
    # A fractional id is a number to JSON-RPC 2.0 and comes back unchanged.
    fractional = await client.call_raw({"jsonrpc": "2.0", "id": 1.5, "method": "get_state"})
    assert fractional["id"] == 1.5 and fractional["result"] == "Running"


async def test_parse_errors_batches_and_notifications(harness):
    server = await harness.server()
    client = await harness.client(server)
    await client.hello("notes")
    waiter = client.call_raw({"jsonrpc": "2.0", "id": "after", "method": "get_state"})
    await client.send_raw("{not json")
    await client.send_raw(json.dumps([{"jsonrpc": "2.0", "id": 1, "method": "get_state"}]))
    await client.send_raw(json.dumps({"jsonrpc": "1.0", "id": 2, "method": "get_state"}))
    await client.send_raw(json.dumps({"jsonrpc": "2.0", "method": "get_state"}))  # a notification: no answer
    assert (await waiter)["result"] == "Running"
    # The three answers with a null id arrived as events did not: they are
    # responses, so the client saw them as responses with id null.
    # (The harness client keys responses by id; a null id has no waiter, so
    # they are simply dropped there.) Read them directly instead:
    raw = await connect_raw(server)
    try:
        await raw.send("{")
        assert json.loads(await raw.recv())["error"]["code"] == codec.PARSE_ERROR
        await raw.send("[]")
        assert json.loads(await raw.recv())["error"]["code"] == codec.INVALID_REQUEST
        await raw.send(json.dumps({"jsonrpc": "2.0", "id": True, "method": "x"}))
        assert json.loads(await raw.recv())["error"]["code"] == codec.INVALID_REQUEST
        await raw.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": 5}))
        assert json.loads(await raw.recv())["error"]["code"] == codec.INVALID_REQUEST
    finally:
        await raw.close()


async def connect_raw(server: LocalApiServer):
    from websockets.asyncio.client import unix_connect

    return await unix_connect(str(server.socket_path), uri="ws://localhost/")


async def test_a_one_shot_call_is_one_connection(harness):
    server = await harness.server()
    for _ in range(3):
        raw = await connect_raw(server)
        await raw.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {"app_id": "shot"}}))
        assert "result" in json.loads(await raw.recv())
        await raw.send(json.dumps({"jsonrpc": "2.0", "id": 2, "method": "local_address"}))
        assert json.loads(await raw.recv())["result"].startswith("off1")
        await raw.close()
    # The server's side of a closed connection is detached on its next turn.
    for _ in range(100):
        if not server.router.sessions_for("shot"):
            break
        await asyncio.sleep(0.02)
    assert server.router.sessions_for("shot") == []


async def test_the_frame_limit_covers_the_engine_file_limit_as_base64():
    assert FILE_SIZE_LIMIT == 100 * 1024 * 1024
    assert MAX_MESSAGE_SIZE >= FILE_SIZE_LIMIT * 4 // 3


# -- errors --------------------------------------------------------------------


async def test_engine_errors_carry_the_variant_and_the_positional_code(harness):
    server = await harness.server()
    client = await harness.client(server)
    await client.hello("notes")
    with pytest.raises(RpcFailure) as err:
        await client.call("get_member_role", {"group_id": "nope", "user_id": "x"})
    assert err.value.variant == "GroupNotFound"
    assert err.value.code == code_of("GroupNotFound") == -32017
    assert "nope" in err.value.message
    # An optional result is null, not an error.
    assert await client.call("get_group_info", {"group_id": "nope"}) is None


async def test_send_message_rich_refuses_a_client_supplied_app_id(harness):
    server = await harness.server()
    client = await harness.client(server)
    await client.hello("notes")
    with pytest.raises(RpcFailure) as err:
        await client.call(
            "send_message_rich",
            {"recipient": "off1zzzz", "content": "hi", "options": {"app_id": "other"}},
        )
    assert err.value.variant == "InvalidArgument"


# -- subscriptions -------------------------------------------------------------


async def test_subscribe_and_unsubscribe(harness):
    server = await harness.server()
    client = await harness.client(server)
    await client.hello("notes")
    assert await client.call("subscribe", {"types": ["message_received", "not_a_tag_yet"]}) is True
    assert await client.call("unsubscribe", {"types": ["message_received"]}) is True
    assert await client.call("subscribe", {"types": "all"}) is True
    with pytest.raises(RpcFailure) as err:
        await client.call("subscribe", {"types": "some"})
    assert err.value.code == codec.INVALID_PARAMS


# -- the carriers --------------------------------------------------------------


async def test_the_unix_socket_is_owner_only(harness):
    server = await harness.server()
    mode = stat.S_IMODE(os.stat(server.socket_path).st_mode)
    assert mode == 0o600
    assert stat.S_IMODE(os.stat(server.socket_path.parent).st_mode) == 0o700


async def test_tcp_requires_the_per_launch_token(harness):
    server = await harness.server(tcp=True)
    assert server.token and len(server.token) == 64
    if os.name != "nt":  # Windows has no owner-only file mode
        assert stat.S_IMODE(os.stat(server.token_path).st_mode) == 0o600
    assert server.token_path.read_text().strip() == server.token
    # Without the token: PermissionDenied, then the connection closes 1008.
    ws = await connect(f"ws://127.0.0.1:{server.port}/")
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {"app_id": "a"}}))
    reply = json.loads(await ws.recv())
    assert reply["error"]["data"]["variant"] == "PermissionDenied"
    with pytest.raises(websockets.exceptions.ConnectionClosed) as closed:
        await ws.recv()
    assert closed.value.rcvd.code == 1008
    # A wrong token, the same.
    ws = await connect(f"ws://127.0.0.1:{server.port}/")
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {"app_id": "a", "token": "0" * 64}}))
    assert json.loads(await ws.recv())["error"]["data"]["variant"] == "PermissionDenied"
    with pytest.raises(websockets.exceptions.ConnectionClosed):
        await ws.recv()
    # A token that is not ASCII is wrong, not an internal error: the
    # constant-time compare takes ASCII only and would raise on it.
    ws = await connect(f"ws://127.0.0.1:{server.port}/")
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "hello", "params": {"app_id": "a", "token": "é" * 64}}))
    assert json.loads(await ws.recv())["error"]["data"]["variant"] == "PermissionDenied"
    with pytest.raises(websockets.exceptions.ConnectionClosed) as closed:
        await ws.recv()
    assert closed.value.rcvd.code == 1008
    # The token from the file works.
    client = await harness.client(server)
    result = await client.hello("a", token=server.token_path.read_text().strip())
    assert result["state"] == "Running"


async def test_tcp_binds_loopback_only():
    manager = ProtocolManager(make_config(profile="tcp-only"))
    with pytest.raises(ValueError):
        LocalApiServer(manager, tcp_port=0, tcp_host="0.0.0.0", token_path="/tmp/x")
    with pytest.raises(ValueError):
        LocalApiServer(manager, tcp_port=0)
    with pytest.raises(ValueError):
        LocalApiServer(manager)


async def test_health_is_served_on_a_plain_get_and_nothing_else_is_http(harness):
    server = await harness.server(tcp=True)
    reader, writer = await asyncio.open_connection("127.0.0.1", server.port)
    writer.write(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
    await writer.drain()
    head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
    assert head.startswith(b"HTTP/1.1 200")
    assert b"application/json" in head.lower()
    body = json.loads(await asyncio.wait_for(reader.readline(), 5))
    assert body == {"server": {"name": SERVER_NAME, "version": server.version}, "api_version": API_VERSION, "carrier": "tcp"}
    writer.close()
    # Another path: the library's default answer.
    reader, writer = await asyncio.open_connection("127.0.0.1", server.port)
    writer.write(b"GET /other HTTP/1.1\r\nHost: localhost\r\n\r\n")
    await writer.drain()
    head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
    assert head.startswith(b"HTTP/1.1 426")
    writer.close()


async def test_a_post_receives_nothing_at_all(harness):
    """Pins the library fact the chapter's "Why there is no HTTP" rests on,
    verified on websockets 16.1 (the lock; pyproject accepts >=12,<17). A
    version that starts answering a POST turns this red, which is the
    moment to revisit the chapter rather than to relax this test."""
    assert websockets.__version__.split(".")[0] == "16", websockets.__version__
    server = await harness.server(tcp=True)
    reader, writer = await asyncio.open_connection("127.0.0.1", server.port)
    body = b'{"jsonrpc":"2.0","id":1,"method":"local_address"}'
    writer.write(
        b"POST /rpc HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n"
        + b"Content-Length: %d\r\n\r\n" % len(body)
        + body
    )
    await writer.drain()
    data = await asyncio.wait_for(reader.read(), 5)
    assert data == b""
    writer.close()


async def test_health_can_be_turned_off(harness):
    server = await harness.server(tcp=True, health=False)
    reader, writer = await asyncio.open_connection("127.0.0.1", server.port)
    writer.write(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
    await writer.drain()
    head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
    assert head.startswith(b"HTTP/1.1 426")
    writer.close()


# -- the server-side rules -----------------------------------------------------


async def test_service_ownership_is_by_application_id(harness):
    server = await harness.server()
    a = await harness.client(server)
    b = await harness.client(server)
    a2 = await harness.client(server)
    await a.hello("app-a")
    await b.hello("app-b")
    await a2.hello("app-a")
    await a.call("services.register_service", {"service_id": "printer", "version": "1", "capabilities": {"k": "v"}})
    with pytest.raises(RpcFailure) as err:
        await b.call("services.unregister_service", {"service_id": "printer"})
    assert err.value.variant == "PermissionDenied"
    with pytest.raises(RpcFailure) as err:
        await b.call("services.register_service", {"service_id": "printer", "version": "2", "capabilities": {}})
    assert err.value.variant == "PermissionDenied"
    with pytest.raises(RpcFailure) as err:
        await b.call(
            "services.respond_to_service_request",
            {"request_id": "r", "requester": "off1x", "service_id": "printer", "status": "ok", "body": "{}"},
        )
    assert err.value.variant == "PermissionDenied"
    # Another client of the same application may release it, and a rival
    # may claim the id once it is free.
    assert await a2.call("services.unregister_service", {"service_id": "printer"}) is True
    await b.call("services.register_service", {"service_id": "printer", "version": "2", "capabilities": {}})


async def test_a_refused_registration_leaves_no_claim(harness):
    server = await harness.server()
    a = await harness.client(server)
    b = await harness.client(server)
    await a.hello("app-a")
    await b.hello("app-b")
    with pytest.raises(RpcFailure) as err:
        await a.call("services.register_service", {"service_id": "", "version": "1", "capabilities": {}})
    assert err.value.variant == "InvalidConfiguration"
    # `b` is not blocked by a claim the engine never backed.
    with pytest.raises(RpcFailure) as err:
        await b.call("services.register_service", {"service_id": "", "version": "1", "capabilities": {}})
    assert err.value.variant == "InvalidConfiguration"


async def test_service_ownership_never_turns_admission_into_default_deny(harness):
    server = await harness.server()
    for app_id in ("app-a", "app-b", "app-c"):
        client = await harness.client(server)
        await client.hello(app_id)
        await client.call(
            "services.register_service", {"service_id": f"svc-{app_id}", "version": "1", "capabilities": {}}
        )
    # Ownership is a runtime shadow, not a configured rule: with no rules
    # configured, an id nobody has seen is still admitted.
    newcomer = await harness.client(server)
    assert (await newcomer.hello("never-seen"))["api_version"] == API_VERSION
    with pytest.raises(RpcFailure) as err:
        await newcomer.call("services.unregister_service", {"service_id": "svc-app-a"})
    assert err.value.variant == "PermissionDenied"


async def test_space_scoping_gates_calls_and_filters_the_listing(harness):
    policy = Policy(spaces={"notes": ["notes-*"]}, applications=["mail"])
    server = await harness.server(config=make_config(profile="scoped", data_enabled=True), policy=policy)
    notes = await harness.client(server)
    mail = await harness.client(server)
    await notes.hello("notes")
    await mail.hello("mail")
    await notes.call("data.create_doc", {"space_id": "notes-1", "doc_id": "todo"})
    with pytest.raises(RpcFailure) as err:
        await notes.call("data.create_doc", {"space_id": "mail-1", "doc_id": "inbox"})
    assert err.value.variant == "PermissionDenied"
    await mail.call("data.create_doc", {"space_id": "mail-1", "doc_id": "inbox"})
    assert await notes.call("data.list_spaces") == ["notes-1"]
    assert sorted(await mail.call("data.list_spaces")) == ["mail-1", "notes-1"]
    with pytest.raises(RpcFailure) as err:
        await notes.call("data.doc_json", {"space_id": "mail-1", "doc_id": "inbox"})
    assert err.value.variant == "PermissionDenied"


async def test_data_calls_answer_with_the_engine_refusal_when_the_layer_is_off(harness):
    server = await harness.server(config=make_config(profile="nodata", data_enabled=False))
    client = await harness.client(server)
    await client.hello("notes")
    with pytest.raises(RpcFailure) as err:
        await client.call("data.list_spaces")
    assert err.value.variant == "DataDisabled"


async def test_method_groups_are_denied_by_application_id(harness):
    policy = Policy(denied={"kiosk": ["sign_data", "tuning", "data.flush_all"]}, applications=["admin"])
    server = await harness.server(policy=policy)
    kiosk = await harness.client(server)
    admin = await harness.client(server)
    await kiosk.hello("kiosk")
    await admin.hello("admin")
    for method, params in (
        ("sign_data", {"data": base64.b64encode(b"x").decode()}),
        ("release_transport_lock", None),
        ("data.flush_all", None),
    ):
        with pytest.raises(RpcFailure) as err:
            await kiosk.call(method, params)
        assert err.value.variant == "PermissionDenied", method
        assert err.value.code != codec.METHOD_NOT_FOUND
    # Not denied elsewhere: the admin signs.
    signature = await admin.call("sign_data", {"data": base64.b64encode(b"x").decode()})
    assert len(base64.b64decode(signature)) == 64
    assert await admin.call("release_transport_lock") is None


async def test_once_rules_exist_an_unnamed_application_id_is_refused(harness):
    policy = Policy(denied={"kiosk": ["sign_data"]})
    server = await harness.server(policy=policy)
    kiosk = await harness.client(server)
    await kiosk.hello("kiosk")
    with pytest.raises(RpcFailure) as err:
        await kiosk.call("sign_data", {"data": base64.b64encode(b"x").decode()})
    assert err.value.variant == "PermissionDenied"
    # Reconnecting under a name no rule mentions does not step around it.
    again = await harness.client(server)
    with pytest.raises(RpcFailure) as err:
        await again.hello("kiosk-2")
    assert err.value.variant == "PermissionDenied"
    assert server.router.sessions_for("kiosk-2") == []


async def test_with_no_rules_every_application_id_is_admitted(harness):
    server = await harness.server()
    for app_id in ("anything", "at-all", "x" * 256):
        client = await harness.client(server)
        assert (await client.hello(app_id))["api_version"] == API_VERSION


async def test_policy_from_dict_refuses_unknown_sections_and_names():
    with pytest.raises(ValueError):
        Policy.from_dict({"nope": {}})
    with pytest.raises(ValueError):
        Policy.from_dict({"denied": {"a": ["not a method!"]}})
    policy = Policy.from_dict({"spaces": {"a": ["a-*"]}, "denied": {"b": ["tuning"]}, "applications": ["c"]})
    assert policy.admits("a") and policy.admits("b") and policy.admits("c") and not policy.admits("d")
    assert Policy.from_dict(None).admits("anyone")
    # A list of applications on its own configures no rule, so it restricts nothing.
    assert Policy.from_dict({"applications": ["c"]}).admits("anyone")
    assert policy.denies("b", "force_transport") and not policy.denies("a", "force_transport")
    # A deny that names nothing on the wire is refused at load, naming the
    # entry: it would otherwise deny nothing while the operator believed
    # the signing oracle withheld.
    with pytest.raises(ValueError, match="sign_dat"):
        Policy.from_dict({"denied": {"kiosk": ["sign_dat", "tuning"]}})
    with pytest.raises(ValueError, match="tunning"):
        Policy.from_dict({"denied": {"kiosk": ["sign_data", "tunning"]}})
    with pytest.raises(ValueError, match="process"):
        Policy.from_dict({"denied": {"kiosk": ["process"]}})
    accepted = Policy.from_dict({"denied": {"kiosk": ["data.flush_all", "manual_mls", "sign_data"]}})
    assert accepted.denies("kiosk", "mls_decrypt") and accepted.denies("kiosk", "data.flush_all")


# -- failures the decoders did not foresee ------------------------------------


async def test_a_failure_inside_the_server_is_an_error_object_on_an_open_connection(harness, caplog):
    server = await harness.server()
    client = await harness.client(server)
    await client.hello("notes")

    async def broken(session, method, params):
        raise RuntimeError("secret detail")

    server._dispatcher.call = broken
    with caplog.at_level("ERROR", logger="offline_protocol_sdk.local_api.server"):
        with pytest.raises(RpcFailure) as err:
            await client.call("get_state")
    assert err.value.code == codec.INTERNAL_ERROR
    assert err.value.message == "get_state: internal error"
    assert "secret detail" not in err.value.message and err.value.variant is None
    assert any("secret detail" in (r.exc_text or "") for r in caplog.records)
    # The connection is still open and still answers.
    del server._dispatcher.call
    assert await client.call("get_state") == "Running"


# -- the socket's directory and path ------------------------------------------


@pytest.mark.skipif(os.name == "nt", reason="Unix sockets and POSIX modes only")
async def test_a_wide_pre_existing_directory_is_refused_and_not_narrowed(harness):
    wide = harness._tmp / "wide"
    wide.mkdir()
    os.chmod(wide, 0o755)
    manager = ProtocolManager(make_config(profile="unix-wide"))
    server = LocalApiServer(manager, socket_path=wide / "api.sock")
    with pytest.raises(ValueError, match="no group or other"):
        await server.start()
    assert stat.S_IMODE(os.stat(wide).st_mode) == 0o755
    assert not (wide / "api.sock").exists()


@pytest.mark.skipif(os.name == "nt", reason="Unix sockets and POSIX modes only")
async def test_a_regular_file_at_the_socket_path_is_refused_and_kept(harness):
    directory = harness._tmp / "kept"
    directory.mkdir(mode=0o700)
    path = directory / "api.sock"
    path.write_text("not a socket")
    manager = ProtocolManager(make_config(profile="unix-file"))
    server = LocalApiServer(manager, socket_path=path)
    with pytest.raises(ValueError, match="not a socket"):
        await server.start()
    assert path.read_text() == "not a socket"


@pytest.mark.skipif(os.name == "nt", reason="Unix sockets and POSIX modes only")
async def test_an_owner_only_pre_existing_directory_is_used_as_is(harness):
    directory = harness._tmp / "mine"
    directory.mkdir(mode=0o700)
    os.chmod(directory, 0o700)
    manager = ProtocolManager(make_config(profile="unix-mine"))
    server = LocalApiServer(manager, socket_path=directory / "api.sock")
    await server.start()
    try:
        assert stat.S_IMODE(os.stat(directory).st_mode) == 0o700
        assert stat.S_IMODE(os.stat(server.socket_path).st_mode) == 0o600
        client = await harness.client(server)
        assert (await client.hello("a"))["state"] == "Running"
    finally:
        await server.stop()
    assert not server.socket_path.exists()


# -- the loop keeps ticking while a call runs ---------------------------------


async def test_a_slow_call_leaves_the_loop_ticking_and_other_calls_waiting(harness):
    import time

    server = await harness.server(tcp=True)
    engine = server.manager.protocol
    ticks = [0]
    real_process = engine.process

    def counting_process():
        ticks[0] += 1
        return real_process()

    def slow():
        time.sleep(1.0)
        return 0

    engine.process = counting_process
    engine.get_pending_ack_count = slow
    # Held for an application whose client arrives during the stall.
    server._on_engine_event({"type": "message_received", "message_id": "held-1", "app_id": "late"})

    loop = asyncio.get_running_loop()
    a = await harness.client(server)
    await a.hello("a", token=server.token)
    slow_call = asyncio.ensure_future(a.call("get_pending_ack_count"))
    await asyncio.sleep(0.15)
    assert not slow_call.done()
    ticks_before = ticks[0]

    # During the stall: health answers, a late client is greeted and gets
    # its held event, and the loop ticks the engine.
    reader, writer = await asyncio.wait_for(asyncio.open_connection("127.0.0.1", server.port), 0.5)
    writer.write(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
    await writer.drain()
    head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 0.5)
    assert head.startswith(b"HTTP/1.1 200")
    writer.close()
    late = await harness.client(server)
    await asyncio.wait_for(late.hello("late", token=server.token), 0.5)
    held = await late.wait_event("message_received", timeout=0.5, message_id="held-1")
    assert held["message_id"] == "held-1"
    assert not slow_call.done()

    # Another client's engine call is serialised behind the slow one.
    b = await harness.client(server)
    await b.hello("b", token=server.token)
    started = loop.time()
    assert await b.call("get_state") == "Running"
    waited = loop.time() - started
    assert slow_call.done() and await slow_call == 0
    assert waited >= 0.3, waited
    assert ticks[0] - ticks_before >= 3, ticks


async def test_a_loop_event_for_the_calls_own_id_reaches_only_the_caller(harness):
    """The window between the executor's completion and the wakeup that
    records the call's result, driven without timing: the fake engine call
    schedules a loop-thread `message_sent` for the id it is about to return,
    ahead of its own completion, exactly as a `process()` tick landing in
    that window would emit it."""
    server = await harness.server()
    loop = asyncio.get_running_loop()
    engine = server.manager.protocol

    def fake_send(**kwargs):
        loop.call_soon_threadsafe(
            server._route,
            {"type": "message_sent", "message_id": "fake-1", "sender": "a", "recipient": "b", "content": "private"},
        )
        return "fake-1"

    engine.send_message_rich = fake_send
    notes = await harness.client(server)
    other = await harness.client(server)
    await notes.hello("notes")
    await other.hello("other")
    message_id = await notes.call("send_message", {"recipient": "off1qb", "content": "private", "priority": "Low"})
    assert message_id == "fake-1"
    sent = await notes.wait_event("message_sent", timeout=5, message_id="fake-1")
    assert sent["content"] == "private"
    await asyncio.sleep(0.3)
    assert other.events_of("message_sent") == []
    assert server.router.parked_count() == 0


async def test_a_configured_gateway_is_started_with_the_server(harness):
    """`ProtocolManager` builds the gateway client when the config enables
    the slot and leaves starting it to its owner; the server is the owner,
    so a configured client that the server never started is a dead carrier."""
    dialled = asyncio.Event()

    async def on_connection(reader, writer):
        dialled.set()
        writer.close()

    daemon = await asyncio.start_server(on_connection, "127.0.0.1", 0)
    try:
        port = daemon.sockets[0].getsockname()[1]
        manager = ProtocolManager(make_config(profile="gateway-user", reticulum_enabled=True))
        manager.gateway.configure(daemon_address=f"127.0.0.1:{port}", auto_reconnect=False)
        await harness.server(manager=manager)
        await asyncio.wait_for(dialled.wait(), 5)
    finally:
        daemon.close()


async def test_an_unconfigured_gateway_does_not_stop_the_server(harness):
    manager = ProtocolManager(make_config(profile="idle-gateway-user", reticulum_enabled=True))
    server = await harness.server(manager=manager)
    client = await harness.client(server)
    result = await client.hello("notes")
    assert result["state"] == "Running"
    assert not manager.gateway.is_available()


def _cli_args(tmp_path, *extra, **config):
    from offline_protocol_sdk.local_api import cli

    fields = {
        "app_id": "server-app",
        "profile": "cli-user",
        "ble_enabled": False,
        "wifi_direct_enabled": False,
        "internet_enabled": True,
        "reticulum_enabled": False,
        "nostr_enabled": False,
        "prefer_online": True,
        "initial_ttl": 3,
        "encryption_enabled": False,
        "require_encryption": False,
        "auto_key_exchange": False,
        "store_pending": True,
        "max_pending_per_peer": 100,
        "max_pending_global": 1000,
        "pending_ttl_ms": 60000,
        "overflow_policy": "DropOldest",
    }
    fields.update(config)
    path = tmp_path / "config.json"
    path.write_text(json.dumps(fields))
    os.environ["OP_TEST_CLI_KEY"] = "ab" * 32
    argv = [
        "--config", str(path),
        "--mls-root", str(tmp_path / "mls"),
        "--state-root", str(tmp_path / "state"),
        "--store-key-env", "OP_TEST_CLI_KEY",
        *extra,
    ]
    return cli, cli.build_parser().parse_args(argv)


async def test_the_cli_configures_the_gateway_from_its_option(tmp_path):
    cli, args = _cli_args(tmp_path, "--gateway", "127.0.0.1:4343", reticulum_enabled=True)
    manager = cli.build_manager(args)
    try:
        assert manager.gateway.is_available()
        assert (manager.gateway._daemon_host, manager.gateway._daemon_port) == ("127.0.0.1", 4343)
    finally:
        await manager.close()


async def test_the_cli_refuses_a_gateway_the_config_does_not_enable(tmp_path):
    cli, args = _cli_args(tmp_path, "--gateway", "127.0.0.1:4343")
    with pytest.raises(SystemExit, match="reticulum_enabled"):
        cli.build_manager(args)
