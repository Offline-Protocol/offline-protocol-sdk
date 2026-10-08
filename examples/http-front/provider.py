#!/usr/bin/env python3
"""A provider that knows nothing of the SDK: an ordinary HTTP server.

It answers ``/now`` with the time, says which device asked (the front puts
the caller's authenticated address in ``X-Offline-Protocol-Sender``), and
registers itself with the HTTP front on its own host at start. Standard
library only.

    python3 provider.py --front http://127.0.0.1:8080 --port 9000

A front started with ``--http-token-file`` refuses a registration without
its token; pass the same file as ``--token-file``.

Prints one JSON line once registered, ``{"registered": "timeofday", "port": N}``.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
from urllib.parse import parse_qs, urlsplit

SERVICE = "timeofday"


class Handler(BaseHTTPRequestHandler):
    def _answer(self) -> None:
        # Read any body before answering: a request whose body is left
        # unread can be reset by the kernel when the socket closes.
        try:
            length = max(0, int(self.headers.get("Content-Length") or 0))
        except ValueError:
            length = 0
        self.rfile.read(length)
        if urlsplit(self.path).path != "/now":
            self._json(404, {"error": "no such path"})
            return
        self._json(
            200,
            {
                "time": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                "caller": self.headers.get("X-Offline-Protocol-Sender"),
            },
        )

    do_GET = _answer
    do_POST = _answer

    def _json(self, status: int, doc: dict) -> None:
        body = json.dumps(doc).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        sys.stderr.write(f"{self.address_string()} {format % args}\n")


def register(front: str, port: int, deadline: float, token_file: str | None) -> None:
    """``PUT /services/timeofday`` on the front, retried while the front is
    starting: unreachable, or a 5xx while it connects to the service. A 4xx
    is the front refusing this registration, and is final."""
    body = json.dumps({"callback": f"http://127.0.0.1:{port}", "version": "1"}).encode()
    while True:
        headers = {"Content-Type": "application/json"}
        if token_file:
            # Read at every attempt: the front writes a new token each start.
            # A missing file is the front not having started yet; any other
            # failure (a permission, a directory) will not fix itself.
            try:
                with open(token_file, encoding="utf-8") as handle:
                    headers["X-Offline-Protocol-Token"] = handle.read().strip()
            except FileNotFoundError:
                pass
            except OSError as exc:
                raise SystemExit(f"--token-file: {exc}") from None
        request = urllib.request.Request(
            f"{front.rstrip('/')}/services/{SERVICE}", data=body, method="PUT", headers=headers
        )
        try:
            with urllib.request.urlopen(request, timeout=5) as reply:
                reply.read()
                return
        except urllib.error.HTTPError as exc:
            detail = exc.read().decode("utf-8", errors="replace")
            if exc.code < 500:
                raise SystemExit(f"the front at {front} refused the registration: {exc.code} {detail}") from None
            if time.monotonic() > deadline:
                raise SystemExit(f"the front at {front} did not accept the registration: {exc.code} {detail}") from None
            time.sleep(0.5)
        except (urllib.error.URLError, OSError) as exc:
            if time.monotonic() > deadline:
                raise SystemExit(f"the front at {front} did not accept the registration: {exc}") from None
            time.sleep(0.5)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--front", default="http://127.0.0.1:8080", help="the HTTP front on this host")
    parser.add_argument("--port", type=int, default=9000, help="where to serve (0 picks a free port)")
    parser.add_argument("--token-file", help="the front's --http-token-file, when it has one")
    parser.add_argument("--wait", type=float, default=30, help="seconds to wait for the front to start")
    args = parser.parse_args(argv)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    port = server.server_address[1]
    Thread(target=server.serve_forever, daemon=True).start()
    register(args.front, port, time.monotonic() + args.wait, args.token_file)
    print(json.dumps({"registered": SERVICE, "port": port}), flush=True)
    try:
        while True:
            time.sleep(3600)
    except KeyboardInterrupt:
        server.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
