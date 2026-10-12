#!/usr/bin/env python3
"""Calls the timeofday service on another device through the HTTP front on
this host, with the standard library only: the same call as ``client.sh``,
for a host or a container that has Python and no ``curl`` (the service
image is one).

    python3 client.py off1qx7jj4u8w32ptzysnkadwjzmz9w2nukfmc3ts2ap
    python3 client.py bob --front http://127.0.0.1:8080 --timeout 10

DEVICE is the device's ``off1...`` address, or an alias from the front's
``--http-aliases`` file. A front started with ``--http-token-file`` requires
its token: pass that file as ``--token-file`` (or set ``OP_TOKEN_FILE``). It
is read at every call, since the front writes a new token each time it
starts.

Prints the response body and exits 0 on a 2xx. Otherwise prints the status,
the front's error token (``X-Offline-Protocol-Error``) when there is one,
and the body to stderr, and exits 1. A ``deadline_exceeded`` (504) does not
mean the provider did not run the request: see the HTTP front chapter.
"""

from __future__ import annotations

import argparse
import os
import sys
import urllib.error
import urllib.request

DOMAIN = "offline.protocol.internal"
SERVICE = "timeofday"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("device", help="the provider's off1... address, or an alias")
    parser.add_argument("--front", default="http://127.0.0.1:8080", help="the HTTP front on this host")
    parser.add_argument("--token-file", default=os.environ.get("OP_TOKEN_FILE"), help="the front's token file")
    parser.add_argument("--timeout", type=float, help="seconds the front waits for the answer (1 to 300)")
    args = parser.parse_args(argv)

    headers = {"Host": f"{SERVICE}.{args.device}.{DOMAIN}"}
    if args.token_file:
        # A missing file stops here rather than sending an empty token.
        with open(args.token_file, encoding="utf-8") as handle:
            headers["X-Offline-Protocol-Token"] = handle.read().strip()
    if args.timeout is not None:
        headers["X-Offline-Protocol-Timeout"] = str(args.timeout)
    request = urllib.request.Request(f"{args.front.rstrip('/')}/now", headers=headers)
    # The front holds the request until its own deadline (30 s by default,
    # at most 300 s): wait a little longer than that, so the answer is the
    # front's 504 and not a local timeout.
    wait = (args.timeout if args.timeout is not None else 30) + 10
    try:
        with urllib.request.urlopen(request, timeout=wait) as reply:
            sys.stdout.write(reply.read().decode("utf-8", errors="replace") + "\n")
            return 0
    except urllib.error.HTTPError as exc:
        token = exc.headers.get("X-Offline-Protocol-Error")
        body = exc.read().decode("utf-8", errors="replace")
        sys.stderr.write(f"{exc.code}{' ' + token if token else ''}: {body}\n")
        return 1
    except (urllib.error.URLError, OSError) as exc:
        sys.stderr.write(f"could not reach the front at {args.front}: {exc}\n")
        return 1


if __name__ == "__main__":
    sys.exit(main())
