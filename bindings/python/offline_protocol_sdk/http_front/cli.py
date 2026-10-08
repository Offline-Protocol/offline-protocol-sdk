"""``offline-protocol-http-front``: run the HTTP front apart from the service.

The usual way to run the front is ``offline-protocol-service --http``, which
starts it in the service's own process. This command runs it as a separate
process against a service that is already running, on its Unix socket or
on its loopback TCP port with the launch token file.
"""

from __future__ import annotations

import argparse
import asyncio
import logging
import signal
import sys
from typing import Any

from .front import HttpFront
from .hostname import DEFAULT_DOMAIN, Aliases
from .registry import Registry


def parse_listen(value: str) -> tuple[str, int]:
    host, _, port = value.rpartition(":")
    if not host or not port.isdigit():
        raise SystemExit("--http takes HOST:PORT")
    return host.strip("[]"), int(port)


def add_front_arguments(parser: argparse.ArgumentParser, *, standalone: bool) -> None:
    group = parser.add_argument_group("HTTP front")
    flag = "--listen" if standalone else "--http"
    group.add_argument(
        flag,
        dest="http",
        metavar="HOST:PORT",
        required=standalone,
        help="serve the HTTP front here (loopback unless --http-token-file is given)",
    )
    group.add_argument("--http-domain", default=DEFAULT_DOMAIN, help=f"the hostname suffix (default: {DEFAULT_DOMAIN})")
    group.add_argument("--http-aliases", metavar="FILE", help='device aliases: {"aliases": {"name": "off1..."}}')
    group.add_argument("--http-registry", metavar="FILE", help="where registered services are kept across restarts")
    group.add_argument(
        "--http-token-file",
        metavar="FILE",
        help="write a per-launch token here and require it in X-Offline-Protocol-Token",
    )


def front_options(args: argparse.Namespace) -> dict[str, Any]:
    """The :class:`HttpFront` keyword arguments the shared flags select."""
    host, port = parse_listen(args.http)
    try:
        aliases = Aliases.load(args.http_aliases) if args.http_aliases else Aliases()
    except (OSError, ValueError) as exc:
        raise SystemExit(f"--http-aliases: {exc}") from None
    return {
        "host": host,
        "port": port,
        "domain": args.http_domain,
        "aliases": aliases,
        "registry_path": args.http_registry,
        "token_path": args.http_token_file,
    }


def check_registry(path: str | None) -> None:
    """Refuses a registry file the front could not load, before anything
    starts: found only once the service is serving, it would stop it."""
    if path is None:
        return
    try:
        Registry(path)
    except (OSError, ValueError) as exc:
        raise SystemExit(f"--http-registry: {exc}") from None


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="offline-protocol-http-front",
        description="Serve plain HTTP for a running offline-protocol-service.",
    )
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--socket", help="the service's Unix socket")
    target.add_argument("--tcp", type=int, metavar="PORT", help="the service's loopback TCP port")
    parser.add_argument("--token-file", help="the service's token file, with --tcp")
    add_front_arguments(parser, standalone=True)
    parser.add_argument("--log-level", default="INFO")
    return parser


def build_front(args: argparse.Namespace) -> HttpFront:
    if args.tcp is not None and not args.token_file:
        raise SystemExit("--tcp requires --token-file")
    options = front_options(args)
    check_registry(options["registry_path"])
    try:
        return HttpFront(
            socket_path=args.socket,
            tcp_port=args.tcp,
            local_api_token_file=args.token_file,
            **options,
        )
    except ValueError as exc:
        raise SystemExit(str(exc)) from None


async def run(front: HttpFront) -> None:
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, stop.set)
        except (NotImplementedError, RuntimeError):
            signal.signal(sig, lambda *_: stop.set())
    await front.start()
    try:
        await stop.wait()
    finally:
        await front.stop()


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    logging.basicConfig(level=args.log_level.upper(), format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    asyncio.run(run(build_front(args)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
