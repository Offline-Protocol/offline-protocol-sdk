"""``python -m offline_gateway --config gateway.toml``"""

from __future__ import annotations

import argparse
import asyncio
import logging
import signal
import sys

from .config import ConfigError, GatewayConfig, load
from .gateway import Gateway


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="offline-gateway",
        description="The Offline Protocol reference gateway daemon (docs/spec/gateway-contract.md).",
    )
    parser.add_argument("--config", help="a TOML file; see gateway/examples/gateway.toml")
    parser.add_argument("--listen", help="HOST:PORT to listen on, overriding the file")
    parser.add_argument("-v", "--verbose", action="store_true", help="debug logging")
    parser.add_argument(
        "--print-destination",
        action="store_true",
        help="print this gateway's Reticulum destination hash (what its peers provision) and exit",
    )
    args = parser.parse_args(argv)

    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    try:
        config = load(args.config) if args.config else GatewayConfig()
        if args.listen:
            host, _, port = args.listen.rpartition(":")
            config.listen_host = host.strip("[]") or config.listen_host
            config.listen_port = int(port)
        config.validate()
    except (ConfigError, OSError, ValueError) as exc:
        print(f"offline-gateway: {exc}", file=sys.stderr)
        return 2
    if args.print_destination:
        if config.reticulum is None:
            print("offline-gateway: the config has no [reticulum] section", file=sys.stderr)
            return 2
        from .reticulum import destination_hash_for

        print(destination_hash_for(config.reticulum.identity_path))
        return 0
    return asyncio.run(_run(config))


async def _run(config: GatewayConfig) -> int:
    backbone = None
    if config.reticulum is not None:
        from .reticulum import ReticulumBackbone

        backbone = ReticulumBackbone(config.reticulum, max_frame_bytes=config.max_backbone_frame_bytes)
    gateway = Gateway(config, backbone=backbone)
    await gateway.start()
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, stop.set)
        except NotImplementedError:  # pragma: no cover - Windows
            pass
    await stop.wait()
    await gateway.stop()
    return 0


if __name__ == "__main__":
    sys.exit(main())
