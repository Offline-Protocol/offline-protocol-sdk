#!/usr/bin/env python3
"""Runs the offline-first scenarios against three devices and prints a table.

Each check is offline-protocol-verify on the device itself (docs/local-api.md),
so a scenario passes on the engine's own events, not on a log read by eye:

  1 store and forward   B off, A sends, B on: B receives it, A gets B's receipt
  2 sender restarts     B off, A sends, A killed and restarted, B on: same
  4 through the middle  A and C meet once, then share no network: C receives
                        A's message one hop away and B reports carrying it

With --docker (the default) the devices are the containers of compose.yml in
this directory, and switching a device off is `docker stop`. With --ssh each
device is a host running the service, and the operator switches it off and on
when asked. Standard library only.

Usage:
  python3 run.py                        # build, start, run every scenario
  python3 run.py --fresh                # new identities first (compose down -v)
  python3 run.py --scenario 1 --scenario 2
  python3 run.py --ssh a=pi@10.0.0.11 --ssh b=pi@10.0.0.12 --ssh c=pi@10.0.0.13
"""

from __future__ import annotations

import argparse
import json
import secrets
import shlex
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOCKET = "/run/offline-protocol/api.sock"
PROJECT = "offline-first"

#: Pass criteria, from the moment the recipient is switched back on (1 and 2)
#: or from the send (4). Loopback-fast in the lab; the margin is for hardware
#: and for a peer-stream redial ladder that may be part-way up.
DELIVERY_AFTER_RETURN_S = 60
HOP_RECEIVED_S = 30
#: How long to wait for the sender's receipt across the hop. It does not come
#: back on an engine without #537 (docs/local-api.md), so by default its
#: absence is reported and does not fail the run.
HOP_RECEIPT_S = 20
#: Session formation after devices first hear each other, and a restarted
#: service answering on its socket.
PAIR_S = 90
READY_S = 60


class ScenarioFailed(Exception):
    pass


@dataclass
class Result:
    scenario: str
    passed: bool
    elapsed_s: float
    detail: str


@dataclass
class Device:
    name: str
    runner: "Runner"
    address: str = ""

    # Every verifier runs with stdin closed. Under --ssh the operator answers
    # prompts on the terminal while an `await` is in flight, and an ssh
    # client that inherits the terminal forwards what it reads to the remote
    # side: the Enter that says B is back on would never reach input().
    def verify(self, *args: str, timeout: float = 600) -> tuple[int, list[dict], str]:
        process = subprocess.run(
            self.runner.command(self.name, ["offline-protocol-verify", "--socket", SOCKET, *args]),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        return process.returncode, _json_lines(process.stdout), process.stderr.strip()

    def verify_in_background(self, *args: str) -> subprocess.Popen[str]:
        return subprocess.Popen(
            self.runner.command(self.name, ["offline-protocol-verify", "--socket", SOCKET, *args]),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )

    def wait_ready(self) -> None:
        deadline = time.monotonic() + READY_S
        while True:
            status, lines, err = self.verify("state", timeout=30)
            if status == 0:
                self.address = lines[-1]["state"]["local_address"]
                return
            if time.monotonic() > deadline:
                raise ScenarioFailed(f"{self.name} did not answer on its socket: {err}")
            time.sleep(1)

    def send(self, recipient: "Device", text: str) -> str:
        status, lines, err = self.verify("send", recipient.address, text)
        if status != 0:
            raise ScenarioFailed(f"{self.name} could not send: {err}")
        return lines[-1]["sent"]["message_id"]


def _json_lines(text: str) -> list[dict]:
    lines = []
    for line in text.splitlines():
        try:
            lines.append(json.loads(line))
        except ValueError:
            continue
    return lines


def _saw(watched: list[dict], tag: str, message_id: str) -> dict | None:
    for line in watched:
        event = line.get("event", {})
        if event.get("type") == tag and event.get("message_id") == message_id:
            return event
    return None


def _finish(process: subprocess.Popen[str], timeout: float) -> tuple[int, list[dict], str]:
    try:
        out, err = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        out, err = process.communicate()
        return 2, _json_lines(out), err.strip()
    return process.returncode, _json_lines(out), err.strip()


class Runner:
    """How to reach a device and how to switch it off and on."""

    def command(self, device: str, argv: list[str]) -> list[str]:
        raise NotImplementedError

    def setup(self, fresh: bool) -> None:
        pass

    def off(self, device: str) -> None:
        raise NotImplementedError

    def on(self, device: str) -> None:
        raise NotImplementedError

    def kill(self, device: str) -> None:
        raise NotImplementedError

    def join_a_and_c(self) -> None:
        raise NotImplementedError

    def part_a_and_c(self) -> None:
        raise NotImplementedError


class DockerRunner(Runner):
    def _compose(self, *args: str) -> None:
        subprocess.run(["docker", "compose", "-p", PROJECT, *args], cwd=HERE, check=True)

    def _docker(self, *args: str) -> None:
        subprocess.run(["docker", *args], check=True, stdout=subprocess.DEVNULL)

    def command(self, device: str, argv: list[str]) -> list[str]:
        return ["docker", "exec", f"{PROJECT}-{device}", *argv]

    def __init__(self, build: bool) -> None:
        self.build = build

    def setup(self, fresh: bool) -> None:
        env = HERE / ".env"
        if fresh:
            self._compose("down", "-v")
        if not env.exists():
            # One key per device, kept: the identity and every queued message
            # are sealed under it.
            env.write_text("".join(f"KEY_{d.upper()}={secrets.token_hex(32)}\n" for d in "abc"))
            env.chmod(0o600)
        self._compose("up", "-d", "--build" if self.build else "--no-build")

    def off(self, device: str) -> None:
        self._docker("stop", f"{PROJECT}-{device}")

    def on(self, device: str) -> None:
        self._docker("start", f"{PROJECT}-{device}")

    def kill(self, device: str) -> None:
        self._docker("kill", "--signal", "KILL", f"{PROJECT}-{device}")

    def join_a_and_c(self) -> None:
        self._docker("network", "connect", f"{PROJECT}_net-ab", f"{PROJECT}-c")

    def part_a_and_c(self) -> None:
        # Stopped while still on net-ab, so A sees the stream close. Taken off
        # the network first, C would leave A a stream that looks open until
        # keepalive ends it (about 30 s), and a message A sends in that window
        # goes down it, is retried over direct carriers only, and never
        # reaches the mesh (docs/local-api.md).
        self._docker("stop", f"{PROJECT}-c")
        self._docker("network", "disconnect", f"{PROJECT}_net-ab", f"{PROJECT}-c")
        self._docker("start", f"{PROJECT}-c")


class SshRunner(Runner):
    """Hosts that run the service already; power and links are the operator's."""

    def __init__(self, hosts: dict[str, str]) -> None:
        self.hosts = hosts

    def command(self, device: str, argv: list[str]) -> list[str]:
        return ["ssh", self.hosts[device], shlex.join(argv)]

    def _ask(self, text: str) -> None:
        input(f"\n>>> {text}, then press Enter: ")

    def off(self, device: str) -> None:
        self._ask(f"switch {device} ({self.hosts[device]}) off")

    def on(self, device: str) -> None:
        self._ask(f"switch {device} ({self.hosts[device]}) on")

    def kill(self, device: str) -> None:
        self._ask(f"cut {device}'s power, or kill -9 its service")

    def join_a_and_c(self) -> None:
        self._ask("put a and c where they hear each other directly")

    def part_a_and_c(self) -> None:
        self._ask("move a and c apart so only b hears both (restart c's service to drop the old link)")


@dataclass
class Lab:
    a: Device
    b: Device
    c: Device
    runner: Runner
    results: list[Result] = field(default_factory=list)

    def pair(self, x: Device, y: Device) -> None:
        status, _, err = x.verify("pair", y.address, "--timeout", str(PAIR_S))
        if status != 0:
            raise ScenarioFailed(f"no session between {x.name} and {y.name}: {err}")

    def store_and_forward(self, restart_sender: bool) -> Result:
        name = "2 sender restarts" if restart_sender else "1 store and forward"
        self.pair(self.a, self.b)
        self.runner.off("b")
        message_id = self.a.send(self.b, f"{name} {time.strftime('%H:%M:%S')}")
        if restart_sender:
            self.runner.kill("a")
            self.runner.on("a")
            self.a.wait_ready()
        # The receipt is not held for a client that is not connected: the
        # wait starts before B can answer.
        receipt = self.a.verify_in_background("await", message_id, "--until", "delivered",
                                              "--timeout", str(DELIVERY_AFTER_RETURN_S + READY_S))
        time.sleep(2)
        started = time.monotonic()
        self.runner.on("b")
        status, lines, err = _finish(receipt, DELIVERY_AFTER_RETURN_S + READY_S + 30)
        elapsed = time.monotonic() - started
        if status != 0 or elapsed > DELIVERY_AFTER_RETURN_S:
            return Result(name, False, elapsed, f"no receipt within {DELIVERY_AFTER_RETURN_S} s: {err}")
        event = lines[-1]["event"]
        self.b.wait_ready()
        status, _, err = self.b.verify("await", message_id, "--until", "received", "--timeout", "30")
        if status != 0:
            return Result(name, False, elapsed, f"receipt came back but B has no message: {err}")
        return Result(name, True, elapsed, f"receipt over {event['transport']}, latency {event['latency_ms']} ms")

    def through_the_middle(self, require_receipt: bool) -> Result:
        name = "4 through the middle"
        self.runner.join_a_and_c()
        self.c.wait_ready()
        self.pair(self.a, self.c)
        self.runner.part_a_and_c()
        self.c.wait_ready()
        self.pair(self.b, self.c)
        self.pair(self.a, self.b)

        # Both watches start before the send: the receipt on A, and B's
        # report of carrying the frame, are not held for a late client.
        window = str(HOP_RECEIVED_S + HOP_RECEIPT_S)
        middle = self.b.verify_in_background("watch", "--duration", window)
        sender = self.a.verify_in_background("watch", "--duration", window)
        time.sleep(2)
        started = time.monotonic()
        message_id = self.a.send(self.c, f"{name} {time.strftime('%H:%M:%S')}")
        status, lines, err = self.c.verify("await", message_id, "--until", "received",
                                           "--timeout", str(HOP_RECEIVED_S))
        elapsed = time.monotonic() - started
        _, watched_b, _ = _finish(middle, float(window) + 30)
        _, watched_a, _ = _finish(sender, float(window) + 30)
        if status != 0:
            return Result(name, False, elapsed, f"C did not receive it within {HOP_RECEIVED_S} s: {err}")
        hops = lines[-1]["event"]["hop_count"]
        relayed = _saw(watched_b, "message_relayed", message_id)
        if hops != 1 or relayed is None:
            return Result(name, False, elapsed, f"received at hop {hops}, B relayed it: {relayed is not None}")
        detail = "C received it at hop 1, B relayed it"
        receipt = _saw(watched_a, "message_delivered", message_id)
        if receipt is not None:
            return Result(name, True, elapsed, detail + f"; A's receipt at hop {receipt['hop_count']}")
        if require_receipt:
            return Result(name, False, elapsed, detail + "; A's receipt never came back")
        return Result(name, True, elapsed, detail + "; A's receipt did not come back (known, #537)")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ssh", action="append", default=[], metavar="NAME=HOST",
                        help="a device as an ssh destination (a, b and c); without it, Docker")
    parser.add_argument("--fresh", action="store_true", help="Docker: remove the devices and their identities first")
    parser.add_argument("--no-build", action="store_true",
                        help="Docker: use the image offline-protocol-service:offline-first as it is")
    parser.add_argument("--scenario", action="append", type=int, choices=(1, 2, 4), help="run only these")
    parser.add_argument("--require-hop-receipt", action="store_true",
                        help="fail scenario 4 when A's receipt does not come back")
    args = parser.parse_args(argv)

    if args.ssh:
        hosts = dict(item.split("=", 1) for item in args.ssh)
        if set(hosts) != {"a", "b", "c"}:
            parser.error("--ssh needs a=..., b=... and c=...")
        runner: Runner = SshRunner(hosts)
    else:
        runner = DockerRunner(build=not args.no_build)
    runner.setup(args.fresh)

    lab = Lab(*(Device(name, runner) for name in "abc"), runner=runner)
    for device in (lab.a, lab.b, lab.c):
        device.wait_ready()
        print(f"{device.name}: {device.address}", file=sys.stderr)

    wanted = args.scenario or [1, 2, 4]
    for number, run in (
        (1, lambda: lab.store_and_forward(restart_sender=False)),
        (2, lambda: lab.store_and_forward(restart_sender=True)),
        (4, lambda: lab.through_the_middle(args.require_hop_receipt)),
    ):
        if number not in wanted:
            continue
        try:
            result = run()
        except (ScenarioFailed, subprocess.SubprocessError) as exc:
            result = Result(str(number), False, 0.0, str(exc))
        lab.results.append(result)
        print(f"{'PASS' if result.passed else 'FAIL'} {result.scenario}: {result.detail}", file=sys.stderr)

    width = max(len(r.scenario) for r in lab.results)
    print(f"\n{'scenario':<{width}}  result  seconds  detail")
    for r in lab.results:
        print(f"{r.scenario:<{width}}  {'pass' if r.passed else 'FAIL':<6}  {r.elapsed_s:7.1f}  {r.detail}")
    return 0 if all(r.passed for r in lab.results) else 1


if __name__ == "__main__":
    sys.exit(main())
