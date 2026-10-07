"""The reference server: one engine, any number of local clients, JSON-RPC
2.0 over a WebSocket on a Unix domain socket or a loopback TCP port.

The server owns the :class:`~offline_protocol_sdk.ProtocolManager` (its run
loop and its drain), the ``MeshServices`` and ``DataStore`` handles, and the
routing tables. A connection runs one request at a time; every engine call
runs on the event loop's thread, so an event the engine emits inside a call
is attributed to that call's connection without any locking.

There is no HTTP request path. The pinned WebSocket library's handshake
parser accepts only ``GET`` and drops any other method before the request
hook runs (verified against ``websockets`` 16.1), so a ``POST`` would fail
with zero bytes back rather than with a JSON-RPC error. The request hook
serves one thing: ``GET /health``.
"""

from __future__ import annotations

import asyncio
import hmac
import http
import json
import logging
import os
import secrets
import stat
from pathlib import Path
from typing import Any

from websockets.asyncio.server import Server, ServerConnection, serve, unix_serve
from websockets.exceptions import ConnectionClosed
from websockets.http11 import Request, Response

from .. import offline_protocol as generated
from ..protocol_manager import ProtocolManager
from . import codec
from .authz import Policy, ServiceOwnership
from .codec import RpcError, taxonomy_error
from .dispatch import SESSION_METHODS, Dispatcher
from .mux import EventRouter
from .session import Session

logger = logging.getLogger(__name__)

API_VERSION = 1
SERVER_NAME = "offline-protocol-service"

#: The engine's file size limit (``file_transfer.rs``, ``max_file_size``),
#: which is not configurable through the interface.
FILE_SIZE_LIMIT = 100 * 1024 * 1024
#: The inbound frame limit: the file limit as base64, plus room for the
#: request around it. Below this a media send fails as a closed connection
#: instead of as the engine's own refusal.
MAX_MESSAGE_SIZE = FILE_SIZE_LIMIT * 4 // 3 + 1024 * 1024
TOKEN_BYTES = 32
#: WebSocket close code for a connection refused by policy.
POLICY_VIOLATION = 1008
#: The engine's own rule for an application id (``ProtocolConfig`` validation).
APP_ID_MAX_BYTES = 256
LOOPBACK_HOSTS = frozenset({"127.0.0.1", "::1"})
#: Seconds one transport may take to start before it counts as failed. The
#: Bluetooth backends go through D-Bus or CoreBluetooth with no deadline of
#: their own, and the API socket opens only after every transport has had
#: its turn: without this, a backend that hangs is a server that never
#: serves and a supervisor that restarts it forever.
CARRIER_START_TIMEOUT = 30.0


def _package_version() -> str:
    try:
        from importlib.metadata import version

        return version("offline-protocol-sdk")
    except Exception:  # not installed as a distribution
        return "0"


def validate_app_id(value: Any) -> str:
    """The engine's rule for an application id, applied to ``hello``."""
    if not isinstance(value, str):
        raise taxonomy_error("InvalidArgument", "app_id must be a string")
    if not value or value in (".", ".."):
        raise taxonomy_error("InvalidArgument", "app_id must not be empty, '.' or '..'")
    try:
        encoded = value.encode("utf-8")
    except UnicodeEncodeError:
        # A lone surrogate is valid JSON text and not valid UTF-8.
        raise taxonomy_error("InvalidArgument", "app_id is not valid UTF-8") from None
    if len(encoded) > APP_ID_MAX_BYTES:
        raise taxonomy_error("InvalidArgument", f"app_id is over {APP_ID_MAX_BYTES} bytes")
    if any(ord(ch) < 0x20 or ch == "\x7f" for ch in value):
        raise taxonomy_error("InvalidArgument", "app_id contains a control character")
    if any(ch in value for ch in "/\\:"):
        raise taxonomy_error("InvalidArgument", "app_id must not contain '/', '\\' or ':'")
    return value


class _CloseAfter(RpcError):
    """An error after which the connection is closed with a policy code."""


class LocalApiServer:
    """Fronts one :class:`ProtocolManager` for local clients.

    Parameters
    ----------
    manager:
        A manager that has not been started. The server registers itself as
        its event handler, starts it, and then starts every transport the
        configuration enables and can run (see :meth:`_start_carriers`).
    policy:
        The space allow-lists and method denials; empty by default.
    socket_path:
        The Unix domain socket to serve on (the default carrier). Created
        ``0600``. A directory the server creates for it is made ``0700``; a
        directory that already exists must be this user's with no group or
        other permissions, and is refused otherwise rather than narrowed. A
        stale socket file at the path is removed first; anything else at
        the path is refused.

    Every engine call a client makes runs on the event loop's default
    executor behind one server-wide lock: calls are serialised, so the
    caller rule's attribution stays exact, and the loop keeps ticking
    (``process()``, the drain, other connections' framing, ``GET /health``)
    while one runs. A media send marshals its bytes per element in pure
    Python, about 1.5 s per MiB, which is the cost that rule pays for.
    ``hello``, ``subscribe`` and ``unsubscribe`` run on the loop; the two
    engine reads in ``hello`` hold the engine's lock for microseconds.
    tcp_port, tcp_host, token_path:
        The loopback TCP alternative: ``tcp_port`` (``0`` picks a free port,
        readable as :attr:`port`), a loopback ``tcp_host``, and the file the
        per-launch token is written to, ``0600``. A client presents the
        token in ``hello``.
    health:
        Serve ``GET /health`` through the request hook.
    max_size:
        The inbound frame limit.
    carrier_start_timeout:
        Seconds one transport may take to start before it is logged as
        failed and left stopped.
    """

    def __init__(
        self,
        manager: ProtocolManager,
        *,
        policy: Policy | None = None,
        socket_path: str | Path | None = None,
        tcp_port: int | None = None,
        tcp_host: str = "127.0.0.1",
        token_path: str | Path | None = None,
        health: bool = True,
        max_size: int = MAX_MESSAGE_SIZE,
        carrier_start_timeout: float = CARRIER_START_TIMEOUT,
    ) -> None:
        if (socket_path is None) == (tcp_port is None):
            raise ValueError("pass exactly one of socket_path or tcp_port")
        if tcp_port is not None:
            if token_path is None:
                raise ValueError("tcp_port requires token_path: TCP is served only with the token")
            if tcp_host not in LOOPBACK_HOSTS:
                raise ValueError(f"tcp_host must be loopback ({sorted(LOOPBACK_HOSTS)}), not {tcp_host!r}")
        self._manager = manager
        self._policy = policy or Policy()
        self._ownership = ServiceOwnership()
        self._router = EventRouter(self._policy, self._ownership)
        self.socket_path = Path(socket_path) if socket_path is not None else None
        self._tcp_port = tcp_port
        self._tcp_host = tcp_host
        self.token_path = Path(token_path) if token_path is not None else None
        self._health = health
        self._max_size = max_size
        self._carrier_start_timeout = carrier_start_timeout
        self._loop: asyncio.AbstractEventLoop | None = None
        self._server: Server | None = None
        self._dispatcher: Dispatcher | None = None
        self._sessions: set[Session] = set()
        self.token: str | None = None
        self.port: int | None = None
        self.version = _package_version()

    # -- lifecycle ------------------------------------------------------------

    @property
    def carrier(self) -> str:
        return "unix" if self.socket_path is not None else "tcp"

    @property
    def manager(self) -> ProtocolManager:
        return self._manager

    @property
    def router(self) -> EventRouter:
        return self._router

    async def start(self) -> None:
        self._loop = asyncio.get_running_loop()
        if self.socket_path is not None:
            # Refused before the engine starts: a directory or a path the
            # server may not use is the operator's to fix, and no reason to
            # have opened the stores.
            _prepare_socket_directory(self.socket_path.parent)
            _remove_stale_socket(self.socket_path)
        self._manager.on_event(self._on_engine_event)
        await self._manager.start()
        try:
            await self._start_carriers()
            engine = self._manager.protocol
            services = generated.MeshServices(engine)
            data: Any
            try:
                data = generated.DataStore(engine)
            except generated.ProtocolError as exc:
                # The data layer is off or has no storage; every `data.*`
                # call answers with this same refusal.
                data = exc
            self._call_lock = asyncio.Lock()
            self._dispatcher = Dispatcher(
                engine, services, data, self._router, self._policy, self._ownership, self._call_lock
            )
            if self.socket_path is not None:
                self._server = await self._serve_unix(self.socket_path)
            else:
                self._server = await self._serve_tcp()
        except BaseException:
            # The two handles hold the engine, and the engine holds the
            # storage callbacks; freed here, before the interpreter is on
            # its way out, rather than from a finaliser at exit.
            self._dispatcher = None
            await self._stop_manager()
            raise
        logger.info("local API serving on %s", self.socket_path or f"{self._tcp_host}:{self.port}")

    async def _serve_unix(self, path: Path) -> Server:
        # The directory and the path were checked in `start()`, before the
        # engine came up.
        server = await unix_serve(
            self._handle,
            path=str(path),
            process_request=self._process_request,
            max_size=self._max_size,
        )
        # The credential on this carrier is the file's mode: whoever can
        # open the socket is one of the operator's applications. The peer's
        # credentials (its uid, as the kernel could report them) are not
        # read, and nothing here tells one local process from another.
        os.chmod(path, stat.S_IRUSR | stat.S_IWUSR)
        return server

    async def _serve_tcp(self) -> Server:
        assert self.token_path is not None and self._tcp_port is not None
        self.token = secrets.token_hex(TOKEN_BYTES)
        self.token_path.parent.mkdir(parents=True, exist_ok=True)
        # Created with its mode in one step, never created and then
        # narrowed: a file that exists world-readable for even a moment has
        # already published the token. A stale file from an earlier launch
        # is removed first; its token died with that process.
        try:
            self.token_path.unlink()
        except FileNotFoundError:
            pass
        fd = os.open(
            self.token_path,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL,
            stat.S_IRUSR | stat.S_IWUSR,
        )
        with os.fdopen(fd, "w", encoding="ascii") as handle:
            handle.write(self.token + "\n")
        server = await serve(
            self._handle,
            self._tcp_host,
            self._tcp_port,
            process_request=self._process_request,
            max_size=self._max_size,
        )
        self.port = server.sockets[0].getsockname()[1]
        return server

    def _configured_carriers(self) -> list[tuple[str, Any]]:
        """The transports this server starts, in order, as (name, manager).

        A transport is listed when the configuration built its manager and
        the manager can run here: the peer stream always, the Bluetooth
        central and peripheral where the platform has a backend, the
        internet relay and the gateway client only once an address to dial
        was configured (unconfigured, each has nothing to connect to and
        would refuse to start).
        """
        manager = self._manager
        carriers: list[tuple[str, Any]] = []
        if manager.peer_stream is not None:
            carriers.append(("peer stream", manager.peer_stream))
        for name, transport in (
            ("Bluetooth LE central", manager.ble),
            ("Bluetooth LE peripheral", manager.ble_peripheral),
            ("internet relay", manager.internet),
            ("gateway", manager.gateway),
        ):
            if transport is not None and transport.is_available():
                carriers.append((name, transport))
        return carriers

    async def _start_carriers(self) -> None:
        """Starts every configured transport; fails only when none starts.

        The engine is already running, so this device has the address the
        peer stream and the peripheral prove. A transport that fails to
        start is logged and left stopped, and the others run: a radio that
        another process holds must not take the LAN path down with it. When
        every configured transport failed, the server has no way to reach
        anyone and the first failure is raised.
        """
        carriers = self._configured_carriers()
        failures: list[Exception] = []
        for name, transport in carriers:
            try:
                # A start cut off by the deadline leaves the transport in
                # STARTING, which every transport's stop() accepts.
                await asyncio.wait_for(transport.start(), self._carrier_start_timeout)
            except asyncio.CancelledError:
                raise
            except asyncio.TimeoutError:
                logger.error("the %s did not start within %s s and is stopped", name, self._carrier_start_timeout)
                failures.append(TimeoutError(f"the {name} did not start within {self._carrier_start_timeout} s"))
                await self._stop_late_carrier(name, transport)
            except Exception as exc:
                logger.error("the %s did not start and stays stopped: %s", name, exc)
                failures.append(exc)
            else:
                logger.info("the %s is running", name)
        if carriers and len(failures) == len(carriers):
            raise failures[0]

    async def _stop_late_carrier(self, name: str, transport: Any) -> None:
        """Tears down a transport whose start ran past the deadline.

        The cancel landed at whatever the backend was awaiting, and the
        backend may still finish it: a Bluetooth peripheral has its GATT
        callbacks wired before it awaits the advertisement, so one that
        un-wedges later would advertise and take writes with nothing
        watching it until shutdown. Stopped now, under the same deadline.
        """
        try:
            await asyncio.wait_for(transport.stop(), self._carrier_start_timeout)
        except asyncio.CancelledError:
            raise
        except asyncio.TimeoutError:
            logger.error("the %s did not stop within %s s either", name, self._carrier_start_timeout)
        except Exception as exc:
            logger.error("the %s could not be stopped either: %s", name, exc)

    async def stop(self) -> None:
        """Closes every connection, then the engine and its transports."""
        server, self._server = self._server, None
        if server is not None:
            server.close()
            await server.wait_closed()
        for session in list(self._sessions):
            self._router.detach(session)
        self._sessions.clear()
        self._dispatcher = None
        await self._stop_manager()
        if self.socket_path is not None:
            try:
                _remove_stale_socket(self.socket_path)
            except ValueError:
                logger.warning("%s is no longer this server's socket; left in place", self.socket_path)
        if self.token_path is not None:
            try:
                self.token_path.unlink()
            except FileNotFoundError:
                pass
        self.token = None

    async def _stop_manager(self) -> None:
        if self._manager._file_stores is not None:
            await self._manager.close()
        else:
            await self._manager.stop()

    # -- events ---------------------------------------------------------------

    def _on_engine_event(self, event: dict[str, Any]) -> None:
        """The manager's one event handler, on whatever thread emits."""
        loop = self._loop
        if loop is None or loop.is_closed():
            return
        try:
            on_loop = asyncio.get_running_loop() is loop
        except RuntimeError:
            on_loop = False
        if on_loop:
            # The run loop or the drain, on this thread: never a client's
            # call, whatever call is in flight on the executor right now.
            self._route(event)
        else:
            # The executor thread executing one client's call. Queued behind
            # whatever is already on the loop and ahead of the call's own
            # completion, so it is routed while that client is the caller.
            loop.call_soon_threadsafe(self._route, event, True)

    def _route(self, event: dict[str, Any], in_call: bool = False) -> None:
        if not isinstance(event, dict):
            return
        if event.get("type") == "message_received" and "message_id" not in event:
            # The manager's drain wraps the pull-style `receive_message()`
            # JSON as a second `message_received` (keyed `id`). The engine
            # emitted the real event inside that same `receive_message()`
            # call, so this copy is dropped rather than delivered twice.
            return
        self._router.route(event, in_call=in_call)

    # -- the request hook -----------------------------------------------------

    def _process_request(self, connection: ServerConnection, request: Request) -> Response | None:
        if not self._health or request.path != "/health":
            return None
        if "upgrade" in request.headers.get("Connection", "").lower():
            return None
        body = json.dumps(
            {
                "server": {"name": SERVER_NAME, "version": self.version},
                "api_version": API_VERSION,
                "carrier": self.carrier,
            }
        )
        response = connection.respond(http.HTTPStatus.OK, body + "\n")
        response.headers["Content-Type"] = "application/json"
        return response

    # -- connections ----------------------------------------------------------

    async def _handle(self, websocket: ServerConnection) -> None:
        session = Session(self.carrier)
        self._sessions.add(session)
        sender = asyncio.ensure_future(session.sender(websocket))
        try:
            async for raw in websocket:
                # One request at a time per connection, in arrival order;
                # the call itself runs on the executor behind the server's
                # one lock, so the loop keeps ticking while it runs.
                await self._handle_frame(session, raw)
        except ConnectionClosed:
            pass
        finally:
            self._router.detach(session)
            self._sessions.discard(session)
            session.closed = True
            sender.cancel()
            await asyncio.gather(sender, return_exceptions=True)

    async def _handle_frame(self, session: Session, raw: str | bytes) -> None:
        if isinstance(raw, bytes):
            session.push(self._error(None, RpcError(codec.INVALID_REQUEST, "text frames only")))
            return
        try:
            request = json.loads(raw)
        except ValueError:
            session.push(self._error(None, RpcError(codec.PARSE_ERROR, "parse error")))
            return
        if isinstance(request, list):
            session.push(
                self._error(None, RpcError(codec.INVALID_REQUEST, "batches are not supported"))
            )
            return
        if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
            session.push(self._error(None, RpcError(codec.INVALID_REQUEST, "not a JSON-RPC 2.0 request")))
            return
        if "id" not in request:
            # A notification: a client never sends one, and it gets no answer.
            return
        request_id = request["id"]
        # A string or a number (JSON-RPC 2.0 allows a fractional one); `true`
        # is an int to Python and not a number to the framing.
        if not (request_id is None or isinstance(request_id, (str, int, float))) or isinstance(request_id, bool):
            session.push(self._error(None, RpcError(codec.INVALID_REQUEST, "id must be a string or a number")))
            return
        method = request.get("method")
        if not isinstance(method, str):
            session.push(self._error(request_id, RpcError(codec.INVALID_REQUEST, "method must be a string")))
            return
        params = request.get("params")
        if params is not None and not isinstance(params, dict):
            session.push(
                self._error(request_id, RpcError(codec.INVALID_PARAMS, "params must be an object of named parameters"))
            )
            return
        try:
            if method == "hello":
                result = self._hello(session, params or {})
                session.push({"jsonrpc": "2.0", "id": request_id, "result": result})
                # Registered only now: the hello result is queued ahead of
                # the held events, and the held events ahead of anything the
                # router delivers from here on.
                for event in self._router.attach(session):
                    session.push_event(event)
                return
            if method in SESSION_METHODS:
                result = self._subscription(session, method, params or {})
            else:
                assert self._dispatcher is not None
                result = await self._dispatcher.call(session, method, params)
        except _CloseAfter as exc:
            session.push(self._error(request_id, exc))
            session.push_close(POLICY_VIOLATION, "policy violation")
            return
        except RpcError as exc:
            session.push(self._error(request_id, exc))
            return
        except Exception:
            # A value the decoders did not foresee, or a failure of the
            # server's own: the server's log gets the traceback, the client
            # gets an error object, and the connection stays open. Without
            # this the library closes the socket with 1011 and the client
            # learns nothing.
            logger.exception("internal error handling %s", method)
            session.push(
                self._error(request_id, RpcError(codec.INTERNAL_ERROR, f"{method}: internal error"))
            )
            return
        session.push({"jsonrpc": "2.0", "id": request_id, "result": result})

    @staticmethod
    def _error(request_id: Any, error: RpcError) -> dict[str, Any]:
        return {"jsonrpc": "2.0", "id": request_id, "error": error.to_json()}

    # -- the session methods --------------------------------------------------

    def _hello(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if session.app_id is not None:
            raise taxonomy_error("InvalidState", "hello was already sent on this connection")
        unknown = sorted(set(params) - {"app_id", "client", "token"})
        if unknown:
            raise codec.invalid_params(f"hello has no parameter {unknown[0]!r}")
        if "app_id" not in params:
            raise codec.invalid_params("hello requires app_id")
        app_id = validate_app_id(params["app_id"])
        client = params.get("client")
        if client is not None and not isinstance(client, str):
            raise codec.invalid_params("hello.client must be a string")
        if self.carrier == "tcp":
            token = params.get("token")
            # `compare_digest` takes ASCII strings only and raises on
            # anything else; a token that is not ASCII is simply wrong.
            if (
                not isinstance(token, str)
                or not token.isascii()
                or self.token is None
                or not hmac.compare_digest(token, self.token)
            ):
                raise _CloseAfter(*_permission_denied("hello.token is missing or wrong"))
        # Once any rule is configured, an id no rule names is refused, so a
        # rule cannot be stepped around by reconnecting under another name.
        self._policy.check_admission(app_id)
        session.app_id = app_id
        session.client = client
        engine = self._manager.protocol
        return {
            "api_version": API_VERSION,
            "server": {"name": SERVER_NAME, "version": self.version},
            "state": codec.encode("ProtocolState", engine.get_state()),
            "local_address": engine.local_address(),
        }

    @staticmethod
    def _subscription(session: Session, method: str, params: dict[str, Any]) -> bool:
        if session.app_id is None:
            raise taxonomy_error("InvalidState", "hello has not been sent on this connection")
        unknown = sorted(set(params) - {"types"})
        if unknown:
            raise codec.invalid_params(f"{method} has no parameter {unknown[0]!r}")
        types = params.get("types")
        if types != "all":
            if not isinstance(types, list) or not all(isinstance(t, str) for t in types):
                raise codec.invalid_params(f"{method}.types is a list of tags or \"all\"")
        if method == "subscribe":
            session.subscribe(types)
        else:
            session.unsubscribe(types)
        return True


def _permission_denied(message: str) -> tuple[int, str, dict[str, Any]]:
    error = taxonomy_error("PermissionDenied", message)
    return error.code, error.message, error.data or {}


def _prepare_socket_directory(directory: Path) -> None:
    """The socket's directory, owner-only, without touching what the server
    did not create.

    A directory the server creates is made ``0700``. One that already exists
    is required to be this user's with no group or other bits, and is
    otherwise refused by name: narrowing an operator's ``0755`` directory
    (or ``/tmp``) to ``0700`` is not the server's to do, and a socket inside
    a directory others can enter is not the credential the chapter says it
    is.
    """
    try:
        found = directory.stat()
    except FileNotFoundError:
        directory.mkdir(parents=True)
        os.chmod(directory, stat.S_IRWXU)
        return
    if not stat.S_ISDIR(found.st_mode):
        raise ValueError(f"{directory} exists and is not a directory")
    mode = stat.S_IMODE(found.st_mode)
    if found.st_uid != os.getuid() or mode & 0o077:
        raise ValueError(
            f"{directory} must be owned by this user with no group or other "
            f"permissions (found mode {mode:04o}); the server does not narrow a "
            "directory it did not create"
        )


def _remove_stale_socket(path: Path) -> None:
    """Removes a socket file left by an earlier launch, and nothing else."""
    try:
        found = os.lstat(path)
    except FileNotFoundError:
        return
    if not stat.S_ISSOCK(found.st_mode):
        raise ValueError(f"{path} exists and is not a socket; refusing to remove it")
    path.unlink()
