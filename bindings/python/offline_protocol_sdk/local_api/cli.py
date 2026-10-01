"""``offline-protocol-service``: run the reference server from the shell.

The process owns one engine over the built-in file stores (or the platform
keyring with ``--keyring``), serves the local API on a Unix domain socket by
default, and runs until it is told to stop.

Importing this module loads the native library, because the package's own
``__init__`` does; ``--help`` therefore needs the library present.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import logging
import os
import signal
import sys
from pathlib import Path
from typing import Any

from ..gateway_manager import DEFAULT_DAEMON_ADDRESS
from ..protocol_manager import ProtocolManager
from . import codec
from .authz import Policy
from .server import LocalApiServer

DEFAULT_STORE_KEY_ENV = "OFFLINE_PROTOCOL_STORE_KEY"


def default_socket_path() -> Path:
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    base = Path(runtime) if runtime else Path.home() / ".offline-protocol"
    return base / "offline-protocol" / "api.sock" if runtime else base / "api.sock"


def _load_json(path: str, what: str) -> Any:
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except OSError as exc:
        raise SystemExit(f"cannot read the {what} at {path}: {exc}") from None
    except ValueError as exc:
        raise SystemExit(f"the {what} at {path} is not JSON: {exc}") from None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="offline-protocol-service",
        description="Serve one Offline Protocol engine to local applications over the local API.",
    )
    parser.add_argument("--config", required=True, help="JSON file with the ProtocolConfig fields")
    carrier = parser.add_mutually_exclusive_group()
    carrier.add_argument("--socket", help=f"Unix socket path (default: {default_socket_path()})")
    carrier.add_argument("--tcp", type=int, metavar="PORT", help="serve on loopback TCP instead, with a token")
    parser.add_argument("--tcp-host", default="127.0.0.1", help="loopback address for --tcp (127.0.0.1 or ::1)")
    parser.add_argument("--token-file", help="where --tcp writes the per-launch token (mode 0600)")
    stores = parser.add_argument_group("storage")
    stores.add_argument("--mls-root", help="the sealed MLS store's directory (or OFFLINE_PROTOCOL_MLS_ROOT)")
    stores.add_argument("--state-root", help="the protocol-state directory (or OFFLINE_PROTOCOL_STATE_ROOT)")
    stores.add_argument(
        "--store-key-env",
        default=DEFAULT_STORE_KEY_ENV,
        help=f"environment variable holding the 32-byte store key (default: {DEFAULT_STORE_KEY_ENV})",
    )
    stores.add_argument(
        "--keyring",
        action="store_true",
        help="use the platform keyring and the application state store instead of the file stores",
    )
    mesh = parser.add_argument_group("peer stream")
    mesh.add_argument("--listen", metavar="HOST:PORT", help="where the peer-stream transport accepts streams")
    mesh.add_argument("--peer", action="append", default=[], metavar="ENTRY", help="a peer to keep a stream to: host:port or off1...@host:port")
    parser.add_argument(
        "--gateway",
        metavar="HOST:PORT",
        help=f"the gateway daemon to attach to when the config enables reticulum (default: {DEFAULT_DAEMON_ADDRESS})",
    )
    parser.add_argument("--policy", help="JSON file with the space allow-lists and method denials")
    parser.add_argument("--no-health", action="store_true", help="do not answer GET /health")
    parser.add_argument("--log-level", default="INFO")
    return parser


def build_manager(args: argparse.Namespace) -> ProtocolManager:
    raw = _load_json(args.config, "config")
    try:
        config = codec.decode("ProtocolConfig", raw, "config")
    except codec.RpcError as exc:
        raise SystemExit(f"config: {exc.message}") from None
    if args.keyring:
        manager = ProtocolManager(config, state_root=args.state_root)
    else:
        try:
            manager = ProtocolManager(
                config,
                mls_root=args.mls_root,
                state_root=args.state_root,
                store_key_env=args.store_key_env,
            )
        except ValueError as exc:
            raise SystemExit(str(exc)) from None
    if manager.peer_stream is not None and (args.listen or args.peer):
        listen_host, listen_port = None, ...
        if args.listen:
            host, _, port = args.listen.rpartition(":")
            if not host or not port.isdigit():
                raise SystemExit("--listen takes HOST:PORT")
            listen_host, listen_port = host.strip("[]"), int(port)
        manager.peer_stream.configure(listen_host=listen_host, listen_port=listen_port, peers=args.peer)
    if manager.gateway is not None:
        try:
            manager.gateway.configure(daemon_address=args.gateway or DEFAULT_DAEMON_ADDRESS)
        except ValueError as exc:
            raise SystemExit(f"--gateway: {exc}") from None
    elif args.gateway:
        raise SystemExit("--gateway needs reticulum_enabled in the config")
    return manager


def build_server(args: argparse.Namespace, manager: ProtocolManager) -> LocalApiServer:
    policy = Policy.from_dict(_load_json(args.policy, "policy")) if args.policy else Policy()
    if args.tcp is not None:
        if not args.token_file:
            raise SystemExit("--tcp requires --token-file")
        return LocalApiServer(
            manager,
            policy=policy,
            tcp_port=args.tcp,
            tcp_host=args.tcp_host,
            token_path=args.token_file,
            health=not args.no_health,
        )
    return LocalApiServer(
        manager,
        policy=policy,
        socket_path=args.socket or default_socket_path(),
        health=not args.no_health,
    )


async def run(server: LocalApiServer) -> None:
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, stop.set)
        except (NotImplementedError, RuntimeError):
            signal.signal(sig, lambda *_: stop.set())
    await server.start()
    try:
        await stop.wait()
    finally:
        await server.stop()


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    logging.basicConfig(level=args.log_level.upper(), format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    manager = build_manager(args)
    try:
        server = build_server(args, manager)
    except ValueError as exc:
        raise SystemExit(str(exc)) from None
    asyncio.run(run(server))
    return 0


if __name__ == "__main__":
    sys.exit(main())
