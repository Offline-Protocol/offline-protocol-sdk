"""``offline-protocol-verify``: the command line over ``commands.py``.

Usage, on each device against its own service:
  offline-protocol-verify state --peer off1...
  offline-protocol-verify send off1... "hello"
  offline-protocol-verify await <message id> --until delivered --timeout 120
  offline-protocol-verify await <message id> --until received
  offline-protocol-verify pair off1... --timeout 60
  offline-protocol-verify ping off1... --every 2 --count 30
  offline-protocol-verify watch --log events.jsonl
"""

from __future__ import annotations

import argparse
import asyncio
import json
import sys
from pathlib import Path

from websockets.exceptions import WebSocketException

from ..local_api.cli import default_socket_path
from . import commands
from .client import DEFAULT_APP_ID, RpcFailure


def _positive(text: str) -> float:
    value = float(text)
    if not value > 0:
        raise argparse.ArgumentTypeError(f"must be positive, not {text}")
    return value


def _count(text: str) -> int:
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError(f"must be at least 1, not {text}")
    return value


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="offline-protocol-verify",
        description="Drive the local service and wait for the events that prove what happened.",
    )
    carrier = parser.add_mutually_exclusive_group()
    carrier.add_argument("--socket", help=f"the service's Unix socket (default: {default_socket_path()})")
    carrier.add_argument("--tcp", type=int, metavar="PORT", help="the service's loopback TCP port")
    parser.add_argument("--token-file", help="the token file the service wrote (with --tcp)")
    parser.add_argument(
        "--app-id",
        default=DEFAULT_APP_ID,
        help=f"the application id to declare; the same on every device (default: {DEFAULT_APP_ID})",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    state = sub.add_parser("state", help="address, carriers, neighbours, queues, relay counters, sessions")
    state.add_argument("--peer", action="append", default=[], help="an off1... address to report the session with")

    send = sub.add_parser("send", help="send one message and print its id")
    send.add_argument("recipient")
    send.add_argument("content")

    wait = sub.add_parser("await", help="wait for one message's delivery receipt, or its arrival")
    wait.add_argument("message_id")
    wait.add_argument("--until", choices=("delivered", "received"), default="delivered")
    wait.add_argument("--timeout", type=_positive, default=120.0)

    pair = sub.add_parser("pair", help="wait until the session with a peer is confirmed")
    pair.add_argument("peer")
    pair.add_argument("--timeout", type=_positive, default=60.0)

    ping = sub.add_parser("ping", help="send messages on a cadence and report the carrier of each")
    ping.add_argument("recipient")
    ping.add_argument("--every", type=_positive, default=2.0)
    ping.add_argument("--count", type=_count, default=10)
    ping.add_argument("--timeout", type=_positive, default=120.0, help="per message, from its send")

    watch = sub.add_parser("watch", help="print every event, across restarts of the service")
    watch.add_argument("--log", type=Path, help="also append each line to this file")
    watch.add_argument("--duration", type=_positive, help="stop after this many seconds")
    return parser


async def run(args: argparse.Namespace) -> int:
    target = commands.Target(
        socket_path=None if args.tcp is not None else (args.socket or str(default_socket_path())),
        tcp_port=args.tcp,
        token_file=args.token_file,
        app_id=args.app_id,
    )
    output = commands.Output()
    if args.command == "state":
        return await commands.state(target, args.peer, output)
    if args.command == "send":
        return await commands.send(target, args.recipient, args.content, output)
    if args.command == "await":
        return await commands.await_message(target, args.message_id, args.until, args.timeout, output)
    if args.command == "pair":
        return await commands.pair(target, args.peer, args.timeout, output)
    if args.command == "ping":
        return await commands.ping(target, args.recipient, args.every, args.count, args.timeout, output)
    return await commands.watch(target, args.log, args.duration, output)


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.tcp is not None and not args.token_file:
        parser.error("--tcp needs --token-file")
    try:
        return asyncio.run(run(args))
    except RpcFailure as exc:
        print(json.dumps({"error": {"code": exc.code, "variant": exc.variant, "message": str(exc)}}), file=sys.stderr)
        return commands.FAILED
    except (OSError, ConnectionError, WebSocketException) as exc:
        # Not a refusal from the engine: the socket, the token file, or the service went away.
        print(json.dumps({"error": {"message": str(exc) or type(exc).__name__}}), file=sys.stderr)
        return commands.FAILED
    except NotImplementedError:
        # asyncio has no Unix sockets on Windows, where the service listens on TCP.
        print(
            json.dumps({"error": {"message": "Unix sockets are not available on this platform; use --tcp and --token-file"}}),
            file=sys.stderr,
        )
        return commands.FAILED
    except KeyboardInterrupt:
        return commands.PASS if args.command == "watch" else commands.FAILED


if __name__ == "__main__":
    sys.exit(main())
