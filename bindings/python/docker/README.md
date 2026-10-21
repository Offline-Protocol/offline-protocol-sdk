# The service as a container

One container per host runs one engine: the identity, the queued messages,
the transports the configuration enables, the [local API](../../../docs/local-api.md)
for applications that use the SDK, and the
[HTTP front](../../../docs/spec/http-front.md) for applications that do not.
Applications on the host never embed the SDK; they call the front on
`127.0.0.1:8080`.

```bash
export OFFLINE_PROTOCOL_STORE_KEY="$(openssl rand -hex 32)"   # once per host; keep it
docker compose up -d
```

The image installs the released package from PyPI, which has the HTTP
front and the verifier from 0.29.0; a wheel built from a checkout replaces
it (below). To see the service work before deploying it,
two demos run on one machine:
[examples/offline-first](../../../examples/offline-first) (messages held for
a device that is off, and carried through a third) and
[examples/http-front](../../../examples/http-front) (two applications with
no SDK calling each other over HTTP, each in its own container).

This `compose.yml` uses host networking, which on Docker Desktop (macOS,
Windows) is the namespace of Docker's Linux virtual machine, not the
desktop's: the front is not on the Mac's loopback, and multicast DNS does
not reach the Mac's LAN. Use the demos there; this file is for a Linux host.

## Published image

Each release from 0.29.0 is also published, built from this directory, for
linux/amd64 and linux/arm64:

```bash
docker pull ghcr.io/offline-protocol/offline-protocol-service:0.30.0
```

| Tag | What it names |
|---|---|
| `X.Y.Z`, such as `0.30.0` | That release |
| `X.Y`, such as `0.30` | The newest release in that series |
| `latest` | The newest release, never a prerelease |
| `X.Y.Z-rc.N` | A prerelease, under that tag only |

The image installs `offline-protocol-sdk[lan,http]` at exactly its tag's
version from PyPI (the `SDK_VERSION` build argument), and its build fails if
pip installs any other version. `X.Y` and `latest` move with every release;
`X.Y.Z` is never pushed twice by the release workflow, but a registry tag
can be moved, and a digest cannot. So pin the digest in a deployment, which
`docker buildx imagetools inspect
ghcr.io/offline-protocol/offline-protocol-service:0.30.0` prints:
`image: ghcr.io/offline-protocol/offline-protocol-service@sha256:<digest>`.
Read [docs/UPGRADING.md](../../../docs/UPGRADING.md) before moving a host to
a new one. Each image carries a provenance attestation and an SBOM; check the
attestation with `gh attestation verify
oci://ghcr.io/offline-protocol/offline-protocol-service:0.30.0 --repo
Offline-Protocol/offline-protocol-sdk`.

To run it with this directory's `compose.yml`, replace `build: .` with
`image:` and that reference. The grants below apply to it unchanged.

## What a host's container manifest must grant

Whatever runs the container (Compose, a fleet manager, a unit file), it owes
the service these, and the failure each one prevents:

| Grant | Why |
|---|---|
| The host's network namespace | Multicast DNS (UDP 5353) does not leave a container bridge: on one, `--lan` finds only containers on the same bridge, never another host. It also puts the front on the host's loopback, where the host's applications reach it. Set `OP_LISTEN` to the host's LAN address (`192.168.1.10:7878`): on `0.0.0.0` the multicast DNS record names every interface the host has, and on a Docker host that includes `docker0`'s `172.17.0.1`, which every other Docker host holds as its own. A peer dials a record's entries in order and takes the first that connects, so one that lists `docker0` first reaches the dialling host itself every time: that side never connects, and the pair links only when the other side's dial lands. The address must be static: after a DHCP change the bind fails, the peer stream is the only transport the default configuration enables, so the service refuses to start and `restart: unless-stopped` restarts it in a loop. Reserve the address in DHCP. |
| Inbound TCP 7878 | Other hosts open their peer streams here. Every host also dials, but a pair where neither accepts never connects. |
| A persistent volume at `/var/lib/offline-protocol` | The identity and every queued message. Without it the device has a new address after every recreate, and its peers' sessions and queued messages go to an address nobody holds. |
| `OFFLINE_PROTOCOL_STORE_KEY` from the host's secret store | Everything on the volume is sealed under it. A volume shared with another application is readable by it, so the key must not sit beside the data. Losing the key loses the identity. |
| The system D-Bus socket, for Bluetooth LE | With `"ble_enabled": true` the service advertises to phones through BlueZ. It needs `/run/dbus/system_bus_socket` and a D-Bus policy that lets it register an advertisement and a GATT application; running as root inside the container is the simplest. Another process advertising on the same adapter may hold its only advertising slot: the service then logs the refusal and runs on its other transports. |
| A clock | Freshness checks and request deadlines use it. A provider drops a request more than 60 s past its deadline, so a host whose clock runs ahead after an offline boot refuses valid requests. |

## Who can reach this host

The defaults admit any device, and the application behind the front
decides who it serves:

- The peer stream listens on every interface (`0.0.0.0:7878`) and the
  baked configuration has `auto_key_exchange` on, so any device running this
  SDK that reaches the port forms a session by itself. It can then call
  every service registered with the front and answer discovery. An address
  is a key the device generated: a consistent key, not a known device.
- A provider that serves only some devices compares
  `X-Offline-Protocol-Sender` with addresses it was given out of band. That
  header is trustworthy only when nothing but the front can reach the
  provider's callback: see [Who may call](../../../examples/http-front/README.md#who-may-call).
- A `504 deadline_exceeded` does not mean the request did not run: the
  request is not withdrawn, and a provider that receives it late still
  serves it ([the HTTP front chapter](../../../docs/spec/http-front.md#the-requester)).

An application in another container reaches the front by sharing the
network namespace the front listens in: host networking with this
`compose.yml`, or `network_mode: service:<the service container>` on a
bridge network, as [the HTTP demo](../../../examples/http-front/README.md#how-an-application-container-reaches-the-front)
does.

## Configuration

`config.json` is the engine's `ProtocolConfig`, baked into the image; mount
another at `/etc/offline-protocol/config.json` or point `OP_CONFIG` at one.
The default enables the peer stream with encryption required, and leaves
Bluetooth LE and the internet relay off. Two more are baked beside it and
differ from it in one field each: `OP_CONFIG=/etc/offline-protocol/config-ble.json`
turns Bluetooth LE on (and needs the D-Bus grant above), and
`config-relay.json` turns the internet transport on for `OP_RELAY`. `profile`
is this host's label and part of its storage namespace: change it before the
first start, never after.

The image also has `offline-protocol-verify`, which drives the service in
the same container and waits for the events that prove a message was held,
carried and delivered: `docker exec <container> offline-protocol-verify
--socket /run/offline-protocol/api.sock state`. The
[offline-first demo](../../../examples/offline-first) runs its scenarios
with it.

`entrypoint.sh` maps environment variables to flags:

| Variable | Default | Flag |
|---|---|---|
| `OP_LISTEN` | `0.0.0.0:7878` | `--listen`; under host networking, the host's LAN address (see the grants) |
| `OP_PEERS` | none | `--peer`, one per space-separated entry, for hosts multicast cannot reach |
| `OP_LAN` | `1` | `--lan` when `1`, none when `0`; anything else is refused |
| `OP_CONFIG` | `/etc/offline-protocol/config.json` | `--config` |
| `OP_DATA` | `/var/lib/offline-protocol` | `--mls-root` and `--state-root` beneath it |
| `OP_HTTP` | `127.0.0.1:8080` | `--http`; empty for no front |
| `OP_HTTP_TOKEN_FILE` | none | `--http-token-file`: the front writes a per-launch token there and requires it; needed off loopback, and on a host where a browser runs (R23 in the threat model). The file is replaced at every start and readable by the container's user only: bind-mount its directory, never the file, and read it as that user |
| `OP_HTTP_ALIASES` | none | `--http-aliases` |
| `OP_RELAY` | none | `--relay`, token in `OFFLINE_PROTOCOL_RELAY_TOKEN`; the config must set `internet_enabled`, which the default leaves off, or the service refuses to start |
| `OP_GATEWAY` | none | `--gateway`, a gateway daemon's `HOST:PORT`; the config must set `reticulum_enabled`, or the service refuses to start |
| `OP_SOCKET` | `/run/offline-protocol/api.sock` | `--socket` |

Arguments after the image name come after every flag the environment sets,
so an explicit flag wins over its variable: `docker run IMAGE --http
0.0.0.0:8080 --http-token-file /run/front/token` serves off loopback even
with `OP_HTTP` unset.

The local API socket is inside the container. To reach it from the host,
bind-mount its directory; the service refuses a socket directory that is
not owned by its own user with mode 0700, so the host directory must
belong to the uid the container runs as (root, unless remapped) with mode
0700.

## Building from a checkout

The image installs the released package from PyPI. A Linux wheel built from
a checkout, put in `wheels/`, is installed instead, even when PyPI has a
release with the same version number. On a host that is not Linux, build the
wheel in a container as
[the Python README](../README.md#a-linux-wheel-from-a-checkout-on-another-host)
says; its last step copies it here. Then:

```bash
cp bindings/python/dist/offline_protocol_sdk-*-manylinux_*.whl bindings/python/docker/wheels/
docker build bindings/python/docker
```

Wheels for several architectures may sit there together; pip takes the one
that matches the image. The build fails if the installed service has no HTTP
front or the package has no `offline-protocol-verify`, which is the case for
every release before 0.29.0.

The build context is this directory only. Never build from the repository
root or mount the checkout: `target/` alone fills the build VM.

`docker build --build-arg SDK_VERSION=0.30.0 bindings/python/docker` installs
that release from PyPI and nothing else, as the published image does; it is
refused while a wheel is in `wheels/`, which it would otherwise replace.

## What has been run

Two containers from this image, on one bridge network (not host networking)
with no `OP_PEERS`, found each other over multicast DNS, announced each
other under their proved addresses, and served a request through the HTTP
front from one to the demo provider in
[examples/http-front](../../../examples/http-front) on the other, end to end
encrypted, in 35 ms. The address and the front's registrations survived a
restart of the container. Host networking, the deployment this README
recommends, has not been run between two hosts, and neither has Bluetooth
LE from a container on hardware.

On 2026-10-12 the image was built from this directory with a wheel made by
the Python README's container recipe from `main` (d43f4aba), and the two
demos above passed on it, each on one arm64 laptop.
