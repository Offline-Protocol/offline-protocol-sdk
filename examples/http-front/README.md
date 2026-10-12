# Calling a service on another device with plain HTTP

Two programs that know nothing of the SDK, talking across two hosts through
the [HTTP front](../../docs/spec/http-front.md):

- `provider.py` is an ordinary HTTP server (standard library only). It
  answers `/now` with the time and the caller's address, and registers itself
  with the front on its own host at start.
- `client.py` calls it from another host with the standard library only, and
  `client.sh` makes the same call with `curl`. The container image has
  Python and no `curl`, so use `client.py` there.

## Two boxes on one machine

`compose.yml` runs two boxes, each the service with its front, the provider
on box B and the client on box A, on one Docker bridge network where they
find each other over multicast DNS:

```bash
cd examples/http-front
printf 'BOX_A_STORE_KEY=%s\nBOX_B_STORE_KEY=%s\n' \
    "$(openssl rand -hex 32)" "$(openssl rand -hex 32)" > .env   # once; keep it
docker compose up -d --build
docker compose logs provider
# {"registered": "timeofday", "port": 9000, "address": "off1qy4g..."}
docker compose run --rm client off1qy4g...        # box B's address from that line
# {"time": "2026-10-12T09:34:12+00:00", "caller": "off1q87p..."}
docker compose down             # add -v to forget both identities
```

The image needs a package that has the HTTP front, and no release has one
yet: build it as [the image's README](../../bindings/python/docker/README.md#building-from-a-checkout)
says, or `--build` fails and says so.

### How an application container reaches the front

The front listens on the box's loopback, `127.0.0.1:8080`, and is not
published. An application container reaches it by joining the box's
network namespace, as the provider and the client do here with
`network_mode: service:box-b`. On a host that runs the service with host
networking (the image's own `compose.yml`), the front is on the host's
loopback, and a container reaches it with host networking too.

Two consequences of sharing a namespace:

- Every program in it reaches the front, and so can register a service or
  call another device as this box. One front is one application to the
  engine's rules ([R23](../../docs/security/threat-model.md#r23-the-http-front-trusts-every-process-that-can-reach-it)).
- When the box's container is recreated or restarted, a container that
  joined its namespace is left in the old one: the front keeps the
  registration, and calls answer `callback_failed` (502). Recreate the
  application container too: `docker compose up -d --force-recreate provider`.

## Who may call

Any device running this SDK that can reach a box forms a session with it by
itself (`auto_key_exchange`), and can then call every service registered
there. `caller` is the calling device's address as the engine authenticated
it: a key that device generated, so it proves the same device called again,
not which device it is. To serve only some devices, give the provider their
addresses:

```bash
python3 provider.py --allow off1q87p... --allow off1qx7j...
# any other caller: 403 {"error": "caller not allowed", ...}
```

The provider can trust `X-Offline-Protocol-Sender` only because its callback
listens on loopback, which only programs in the box's namespace reach. A
callback that other programs can reach (on a shared bridge network, or a
published port) receives whatever header they send, and must not authorize
on it.

## A 504 does not mean the call did not run

`deadline_exceeded` (504) means the answer did not come back in time. The
request is not withdrawn: the engine keeps carrying it, and a provider that
receives it within 60 s past its deadline still runs it. A provider that
switches something on, or a client that retries after a 504, should make
the request idempotent with a key of its own in an `x-` header.

## On any two hosts

On each host, run the service with the front, for example from the
[container image](../../bindings/python/docker):

```bash
export OFFLINE_PROTOCOL_STORE_KEY="$(openssl rand -hex 32)"   # once per host; keep it
offline-protocol-service --config config.json --mls-root keys --state-root state \
    --listen 0.0.0.0:7878 --lan --http 127.0.0.1:8080
```

On the providing host:

```bash
python3 provider.py --front http://127.0.0.1:8080 --port 9000
```

On the calling host, with the provider host's address (the provider prints
it, and the front's `GET /health` gives it as `local_address`):

```bash
python3 client.py off1qx7jj4u8w32ptzysnkadwjzmz9w2nukfmc3ts2ap
{"time": "2026-10-07T12:49:47+00:00", "caller": "off1q9kd..."}
```

A front started with `--http-token-file` refuses all three programs without
its token: pass the file to the provider and to `client.py` as
`--token-file`, and to `client.sh` as `OP_TOKEN_FILE`. R23 in the threat
model says when a front on loopback should have one (a host where a browser
runs).

With an alias file (`--http-aliases`, `{"aliases": {"bob": "off1..."}}`),
`python3 client.py bob` works too. The Python suite runs the provider
against two in-process hosts (`tests/http_front/test_demo.py`).

## What has been run

| When | Where | Result |
|---|---|---|
| 2026-10-12 | `compose.yml`, Docker 28.3 on one arm64 laptop, the image with the native library and Python package from `main` (d43f4aba) | the call answered in 0.4 to 0.5 s per `docker compose run`, `caller` was box A; `--allow` refused another address with 403 and served box A's; after a restart of box B, 502 until the provider was recreated, then 200; with box B stopped, 504 `deadline_exceeded` at the requested 5 s. The call passed again on the image built by its Dockerfile from a Linux wheel of the same commit |

Not run: two physical hosts, host networking, a token-protected front from
the compose file.
