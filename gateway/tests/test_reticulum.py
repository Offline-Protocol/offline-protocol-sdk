"""Two daemon processes, each with its own Reticulum instance, joined by a
Reticulum TCP interface, and two real SDK engines attached one to each.

This is a LoRa deployment with the radio swapped for TCP: everything above
the interface (identities, announces, paths, links, resources, the request
handlers, presence snapshots) is what runs over an RNode. Reticulum is one
instance per process, which is why the daemons are subprocesses.

Skipped without ``rns`` or without an SDK build that has the verifier.
"""

from __future__ import annotations

import asyncio
import os
import socket
import sys
import textwrap
from pathlib import Path

import pytest

RNS = pytest.importorskip("RNS")
sdk = pytest.importorskip("offline_protocol_sdk.offline_protocol")
if not hasattr(sdk, "verify_gateway_address_proof"):  # pragma: no cover
    pytest.skip("this SDK build has no verify_gateway_address_proof", allow_module_level=True)

from offline_gateway.reticulum import APP_NAME, ASPECTS  # noqa: E402

from test_end_to_end import Device, _pair_and_talk  # noqa: E402

GATEWAY_ROOT = Path(__file__).resolve().parent.parent


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _identity(path: Path) -> str:
    identity = RNS.Identity()
    identity.to_file(str(path))
    return RNS.Destination.hash(identity, APP_NAME, *ASPECTS).hex()


def _rns_config(directory: Path, interface: str) -> None:
    directory.mkdir(parents=True)
    (directory / "config").write_text(
        textwrap.dedent(
            """\
            [reticulum]
              enable_transport = No
              share_instance = No
              panic_on_interface_error = No

            [logging]
              loglevel = 4

            # A real gateway keeps Reticulum's default announce stamp, a
            # proof of work it mints once and stores (seconds to minutes,
            # depending on the CPU). A test cannot wait for two of them.
            [stamps]
              destination_value = 8
              destination_cost = 8

            [interfaces]
            """
        )
        + textwrap.indent(textwrap.dedent(interface), "  ")
    )


def _gateway_config(directory: Path, port: int, peer: str) -> Path:
    path = directory / "gateway.toml"
    path.write_text(
        textwrap.dedent(
            f"""\
            listen_host = "127.0.0.1"
            listen_port = {port}

            [reticulum]
            configdir = "rns"
            identity_path = "gateway.identity"
            peers = ["{peer}"]
            reconnect_interval = 2.0
            announce_interval = 30.0
            loglevel = 4
            """
        )
    )
    return path


class Daemon:
    def __init__(self, config: Path) -> None:
        self.config = config
        self.lines: list[str] = []
        self.proc: asyncio.subprocess.Process | None = None
        self._reader: asyncio.Task[None] | None = None

    async def start(self) -> None:
        env = dict(os.environ, PYTHONPATH=str(GATEWAY_ROOT), PYTHONUNBUFFERED="1")
        self.proc = await asyncio.create_subprocess_exec(
            sys.executable,
            "-m",
            "offline_gateway",
            "--config",
            str(self.config),
            "-v",
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.STDOUT,
            env=env,
        )
        self._reader = asyncio.ensure_future(self._read())

    async def _read(self) -> None:
        assert self.proc is not None and self.proc.stdout is not None
        while line := await self.proc.stdout.readline():
            self.lines.append(line.decode(errors="replace").rstrip())

    async def wait_for(self, text: str, secs: float) -> None:
        for _ in range(int(secs * 10)):
            if any(text in line for line in self.lines):
                return
            if self.proc is not None and self.proc.returncode is not None:
                break
            await asyncio.sleep(0.1)
        raise AssertionError(f"{self.config.parent.name} never logged {text!r}:\n" + "\n".join(self.lines[-40:]))

    async def stop(self) -> None:
        if self.proc is not None and self.proc.returncode is None:
            self.proc.terminate()
            try:
                await asyncio.wait_for(self.proc.wait(), 10)
            except asyncio.TimeoutError:
                self.proc.kill()
        if self._reader is not None:
            await asyncio.gather(self._reader, return_exceptions=True)


async def test_two_engines_talk_across_a_reticulum_backbone(tmp_path: Path) -> None:
    a_dir, b_dir = tmp_path / "a", tmp_path / "b"
    a_dir.mkdir()
    b_dir.mkdir()
    a_hash = _identity(a_dir / "gateway.identity")
    b_hash = _identity(b_dir / "gateway.identity")
    rns_port = _free_port()
    _rns_config(
        a_dir / "rns",
        f"""\
        [[Backbone]]
          type = TCPServerInterface
          enabled = Yes
          listen_ip = 127.0.0.1
          listen_port = {rns_port}
        """,
    )
    _rns_config(
        b_dir / "rns",
        f"""\
        [[Backbone]]
          type = TCPClientInterface
          enabled = Yes
          target_host = 127.0.0.1
          target_port = {rns_port}
        """,
    )
    a_port, b_port = _free_port(), _free_port()
    a = Daemon(_gateway_config(a_dir, a_port, b_hash))
    b = Daemon(_gateway_config(b_dir, b_port, a_hash))
    alice, bob = Device("rns-alice", a_port), Device("rns-bob", b_port)
    try:
        await a.start()
        await b.start()
        await a.wait_for(f"destination is {a_hash}", 20)
        await b.wait_for(f"destination is {b_hash}", 20)
        await a.wait_for(f"backbone link to {b_hash} established", 60)
        await b.wait_for(f"backbone link to {a_hash} established", 60)

        for d in (alice, bob):
            await d.start()
        await a.wait_for("bound", 15)
        await b.wait_for("bound", 15)
        await _pair_and_talk(alice, bob, secs=90)
        # The frames crossed the backbone, not some other path.
        assert any("MessageReceived" in line for line in b.lines)
    except AssertionError:
        print("--- gateway a ---\n" + "\n".join(a.lines[-80:]))
        print("--- gateway b ---\n" + "\n".join(b.lines[-80:]))
        raise
    finally:
        for d in (alice, bob):
            await d.stop()
        await a.stop()
        await b.stop()
