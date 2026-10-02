#!/usr/bin/env python3
"""A Python peer-stream host for testing the iOS manager with one iPhone.

The Mac plays the second device: it advertises and browses
``_offlineprotocol._tcp`` on the LAN, proves its address with the preamble,
and logs every connect, message, loss and stream decision with a timestamp.
ADR 0027 keeps, of two streams for one address, the one the lower address
opened, so every check below is worth running twice: once with a Mac
identity whose address sorts below the iPhone's, once above it.

Setup, from the repository root::

    cd bindings/python && bash scripts/build-desktop.sh
    python3 -m venv .venv && . .venv/bin/activate
    pip install -e '.[lan]'

The iPhone runs the example app (appId ``offline-messenger``) with the
Wi-Fi Direct transport on and Internet off, so nothing reaches it through a
relay instead. Mac and iPhone join the same Wi-Fi network.

Usage::

    # Make two identities, one sorting below the iPhone's address, one above.
    python3 tools/peer-stream-host/peer_stream_host.py identities --iphone off1...

    # Run as one of them. Type `help` for the commands.
    python3 tools/peer-stream-host/peer_stream_host.py run --profile low

What each check should show:

1. One stream, no loop. `PEER UP` once, messages both ways with
   ``transport=wifiDirect``, and no `FLAP?` line over five minutes.
2. Many frames and a large transfer: `burst 500` and `file 2048`.
3. Clean kill: Ctrl-C here, the iPhone reports the peer lost once; start
   again, it reconnects.
4. Silent death: turn the Mac's Wi-Fi off while idle; the iPhone loses the
   peer within about thirty seconds (keepalive); Wi-Fi on, it reconnects.
5. Background: background the app and bring it back; here `PEER DOWN` once,
   then `PEER UP` again.
6. Permission denied: reinstall the app and deny local-network access; the
   app logs the denial as an error diagnostic, and nothing connects.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import os
import shutil
import sys
import time
from pathlib import Path
from typing import Any

from offline_protocol_sdk import ProtocolManager
from offline_protocol_sdk.offline_protocol import OverflowPolicy, ProtocolConfig

HOME = Path(os.environ.get("PEER_STREAM_HOST_HOME", Path.home() / ".offline-protocol-peer-stream-host"))
#: Seconds between stream-metric samples.
SAMPLE_SECONDS = 10
#: Consecutive samples with a new supersede or refusal before `FLAP?`.
FLAP_SAMPLES = 3


def log(tag: str, text: str = "") -> None:
    print(f"{time.strftime('%H:%M:%S')} {tag:<10} {text}", flush=True)


def config(profile: str, app_id: str) -> ProtocolConfig:
    return ProtocolConfig(
        app_id=app_id,
        profile=profile,
        ble_enabled=False,
        wifi_direct_enabled=True,
        internet_enabled=False,
        reticulum_enabled=False,
        nostr_enabled=False,
        prefer_online=False,
        initial_ttl=3,
        encryption_enabled=True,
        auto_key_exchange=True,
        store_pending=True,
        require_encryption=False,
        max_pending_per_peer=100,
        max_pending_global=1000,
        pending_ttl_ms=86_400_000,
        overflow_policy=OverflowPolicy.DROP_OLDEST,
    )


def manager(profile_dir: Path, app_id: str, handler: Any = None) -> ProtocolManager:
    """A manager on the file stores under `profile_dir`, so each identity is a
    directory and nothing touches the login keychain. The peer-stream slot is
    enabled (the core needs one transport) but runs only once started."""
    key_file = profile_dir / "store.key"
    if not key_file.exists():
        profile_dir.mkdir(parents=True, exist_ok=True)
        key_file.write_bytes(os.urandom(32))
        key_file.chmod(0o600)
    return ProtocolManager(
        # One profile name for every directory: the name selects the account
        # inside the store, so naming it after the directory would give a
        # renamed candidate a fresh identity.
        config("peer-stream-host", app_id),
        event_handler=handler,
        store_key=key_file.read_bytes(),
        mls_root=profile_dir / "mls",
        state_root=profile_dir / "state",
    )


async def address_of(profile_dir: Path, app_id: str) -> str:
    async with manager(profile_dir, app_id) as pm:
        address = pm.local_address
    if not address:
        raise SystemExit(f"{profile_dir} has no address: MLS did not initialise")
    return address


def is_lower(a: str, b: str) -> bool:
    """The order ADR 0027 uses: UTF-8 bytes, which is Python's `<` on str."""
    return a < b


async def identities(args: argparse.Namespace) -> None:
    iphone = args.iphone.strip()
    if not iphone.startswith("off1"):
        raise SystemExit("--iphone takes the iPhone's off1... address")
    wanted = {"low": lambda a: is_lower(a, iphone), "high": lambda a: is_lower(iphone, a)}
    for name, fits in wanted.items():
        target = HOME / name
        if target.exists():
            address = await address_of(target, args.app_id)
            if fits(address):
                log("KEEP", f"{name}: {address}")
                continue
            shutil.rmtree(target)
        # About half of all addresses fit, so this rarely takes more than two.
        for attempt in range(1, 33):
            candidate = HOME / f"candidate-{attempt}"
            shutil.rmtree(candidate, ignore_errors=True)
            address = await address_of(candidate, args.app_id)
            if fits(address):
                candidate.rename(target)
                log("MADE", f"{name}: {address}")
                break
            shutil.rmtree(candidate)
        else:
            raise SystemExit(f"no {name} identity after 32 tries")
    log("IPHONE", iphone)
    log("ORDER", "low < iPhone < high: with `low` the Mac's streams win, with `high` the iPhone's")


class Host:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.local = ""
        self.pm: ProtocolManager | None = None
        self.last_metrics: dict[str, Any] = {}
        self.flap_samples = 0
        # The core reports a neighbour again on every inbound body, so only
        # the first report after a loss is news.
        self.neighbors: set[str] = set()
        self.delivered = 0
        self.delivered_logged = 0

    # -- what the core and the stream layer report ----------------------------

    def on_event(self, event: dict[str, Any]) -> None:
        kind = event.get("type", "?")
        if kind == "message_received":
            transport = event.get("transport", "?")
            # Events spell the slot `wifiDirect`; other surfaces `wifi_direct`.
            on_slot = transport.replace("_", "").lower() == "wifidirect"
            warn = "" if on_slot else "  <-- NOT the peer stream"
            content = event.get("content", "")
            shown = content if len(content) <= 80 else f"{content[:77]}..."
            log("RECV", f"from {short(event.get('sender', '?'))} transport={transport} "
                        f"{len(content)}B {shown!r}{warn}")
        elif kind == "file_received":
            import base64

            data = base64.b64decode(event.get("file_data", ""))
            log("FILE", f"{event.get('file_name')} {len(data)}B sha256={hashlib.sha256(data).hexdigest()[:16]} "
                        f"(declared {event.get('file_size')}B)")
        elif kind == "neighbor_discovered":
            peer = event.get("peer_id", "?")
            if peer not in self.neighbors or self.args.verbose:
                self.neighbors.add(peer)
                log("NEIGHBOR+", f"{short(peer)} via {event.get('transport')}")
        elif kind == "neighbor_lost":
            peer = event.get("peer_id", "?")
            self.neighbors.discard(peer)
            log("NEIGHBOR-", short(peer))
        elif kind == "message_delivered" and not self.args.verbose:
            self.delivered += 1  # summarised by `watch`, a burst is hundreds
        elif kind in ("message_delivered", "message_failed", "transport_switched"):
            detail = {k: v for k, v in event.items() if k != "type"}
            log(kind.upper()[:10], str(detail))
        elif self.args.verbose:
            detail = {k: v for k, v in event.items() if k != "type"}
            log("EVENT", f"{kind} {str(detail)[:400]}")

    def on_diagnostic(self, level: str, message: str, context: dict[str, Any]) -> None:
        if level == "debug" and not self.args.verbose:
            return
        peer = context.get("peer")
        extra = f" peer={short(peer)} ({self.order(peer)})" if peer else ""
        rest = {k: v for k, v in context.items() if k != "peer"}
        log(f"STREAM:{level[:4]}", f"{message}{extra}{' ' + str(rest) if rest else ''}")

    def order(self, peer: str) -> str:
        if is_lower(self.local, peer):
            return "we are lower: the stream we open wins"
        return "we are higher: the stream it opens wins"

    # -- the run -------------------------------------------------------------

    async def run(self) -> None:
        profile_dir = HOME / self.args.profile
        if not profile_dir.exists():
            # Fine for a first run, which only learns the iPhone's address.
            log("NEW", f"no identity {profile_dir.name} yet; making one")
        async with manager(profile_dir, self.args.app_id, handler=self.on_event) as pm:
            self.pm = pm
            self.local = pm.local_address or ""
            stream = pm.peer_stream
            assert stream is not None
            stream.set_delegate(on_diagnostic=self.on_diagnostic)
            stream.configure(listen_port=self.args.port, advertise=True, discover=not self.args.no_discover)
            await stream.start()
            log("UP", f"{self.args.profile} {self.local} listening on {stream.listen_port}")
            log("HINT", "type `help` for commands")
            watcher = asyncio.create_task(self.watch())
            try:
                await self.commands()
            finally:
                watcher.cancel()

    async def watch(self) -> None:
        """Samples the stream layer. A supersede or refusal is normal once per
        reconnect; one in every sample is the reconnect loop ADR 0027 names."""
        assert self.pm is not None and self.pm.peer_stream is not None
        stream = self.pm.peer_stream
        peers: set[str] = set()
        while True:
            now = set(stream.connected_peers())
            for peer in sorted(now - peers):
                log("PEER UP", f"{peer} ({self.order(peer)})")
            for peer in sorted(peers - now):
                log("PEER DOWN", peer)
            peers = now
            metrics = stream.get_metrics()
            churn = sum(
                metrics[k] - self.last_metrics.get(k, metrics[k])
                for k in ("streams_superseded", "streams_refused")
            )
            self.flap_samples = self.flap_samples + 1 if churn else 0
            if self.flap_samples >= FLAP_SAMPLES:
                log("FLAP?", f"streams superseded or refused in {self.flap_samples} samples running: {metrics}")
            self.last_metrics = metrics
            if self.delivered != self.delivered_logged:
                log("DELIVERED", f"{self.delivered} acknowledged so far (+{self.delivered - self.delivered_logged})")
                self.delivered_logged = self.delivered
            await asyncio.sleep(SAMPLE_SECONDS if peers else 1)

    async def commands(self) -> None:
        loop = asyncio.get_running_loop()
        while True:
            line = await loop.run_in_executor(None, sys.stdin.readline)
            if not line:
                await asyncio.Event().wait()  # stdin closed: keep logging
            words = line.strip().split(maxsplit=1)
            if not words:
                continue
            try:
                if not self.command(words[0], words[1] if len(words) > 1 else ""):
                    return
            except Exception as exc:
                hint = ""
                if "SessionPending" in str(exc):
                    hint = " (the encrypted session with the peer is still forming; `send` something, then retry)"
                log("ERROR", f"{words[0]}: {exc}{hint}")

    def command(self, verb: str, rest: str) -> bool:
        assert self.pm is not None and self.pm.peer_stream is not None
        if verb in ("quit", "exit"):
            return False
        if verb == "help":
            print("  send TEXT      a message to the connected iPhone\n"
                  "  burst N        N small messages at once (many frames per read)\n"
                  "  file KB        a file of KB kibibytes (chunked by the core)\n"
                  "  peers          the addresses with a proved stream\n"
                  "  metrics        the stream layer's counters\n"
                  "  quit", flush=True)
        elif verb == "peers":
            log("PEERS", str(self.pm.peer_stream.connected_peers()))
        elif verb == "metrics":
            log("METRICS", str(self.pm.peer_stream.get_metrics()))
        elif verb == "send":
            log("SENT", self.pm.send_message(self.peer(), rest or "hello from the Mac"))
        elif verb == "burst":
            peer, count = self.peer(), int(rest or "100")
            for i in range(count):
                self.pm.send_message(peer, f"burst {i + 1}/{count}")
            log("SENT", f"{count} messages")
        elif verb == "file":
            data = os.urandom(int(rest or "1024") * 1024)
            file_id = self.pm.protocol.send_file(self.peer(), list(data), f"mac-{len(data)}.bin")
            log("SENT", f"file {file_id} {len(data)}B sha256={hashlib.sha256(data).hexdigest()[:16]}")
        else:
            log("ERROR", f"unknown command {verb!r}; try `help`")
        return True

    def peer(self) -> str:
        assert self.pm is not None and self.pm.peer_stream is not None
        peers = self.pm.peer_stream.connected_peers()
        if not peers:
            raise RuntimeError("no peer has a proved stream yet")
        if len(peers) > 1:
            log("NOTE", f"{len(peers)} peers connected; using {peers[0]}")
        return peers[0]


def short(address: str) -> str:
    return address if len(address) <= 20 else f"{address[:12]}...{address[-6:]}"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--app-id", default="offline-messenger",
                        help="must match the iPhone app's appId (default: the example app's)")
    sub = parser.add_subparsers(dest="command", required=True)
    make = sub.add_parser("identities", help="make a `low` and a `high` identity around the iPhone's address")
    make.add_argument("--iphone", required=True, help="the iPhone's off1... address")
    run = sub.add_parser("run", help="run as one identity and log everything")
    run.add_argument("--profile", default="low", help="`low`, `high`, or any identity directory name")
    run.add_argument("--port", type=int, default=0, help="listen port (default: any free one)")
    run.add_argument("--no-discover", action="store_true",
                     help="advertise only, never dial: only the iPhone can open a stream")
    run.add_argument("-v", "--verbose", action="store_true", help="also log debug diagnostics and every event")
    args = parser.parse_args()
    try:
        asyncio.run(identities(args) if args.command == "identities" else Host(args).run())
    except KeyboardInterrupt:
        log("DOWN", "stopped")


if __name__ == "__main__":
    main()
