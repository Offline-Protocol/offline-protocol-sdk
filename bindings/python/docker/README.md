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

## What a host's container manifest must grant

Whatever runs the container (Compose, a fleet manager, a unit file), it owes
the service these, and the failure each one prevents:

| Grant | Why |
|---|---|
| The host's network namespace | Multicast DNS (UDP 5353) does not leave a container bridge: on one, `--lan` finds only containers on the same bridge, never another host. It also puts the front on the host's loopback, where the host's applications reach it. |
| Inbound TCP 7878 | Other hosts open their peer streams here. Every host also dials, but a pair where neither accepts never connects. |
| A persistent volume at `/var/lib/offline-protocol` | The identity and every queued message. Without it the device has a new address after every recreate, and its peers' sessions and queued messages go to an address nobody holds. |
| `OFFLINE_PROTOCOL_STORE_KEY` from the host's secret store | Everything on the volume is sealed under it. A volume shared with another application is readable by it, so the key must not sit beside the data. Losing the key loses the identity. |
| The system D-Bus socket, for Bluetooth LE | With `"ble_enabled": true` the service advertises to phones through BlueZ. It needs `/run/dbus/system_bus_socket` and a D-Bus policy that lets it register an advertisement and a GATT application; running as root inside the container is the simplest. Another process advertising on the same adapter may hold its only advertising slot: the service then logs the refusal and runs on its other transports. |
| A clock | Freshness checks and request deadlines use it. A provider drops a request more than 60 s past its deadline, so a host whose clock runs ahead after an offline boot refuses valid requests. |

## Configuration

`config.json` is the engine's `ProtocolConfig`, baked into the image; mount
another at `/etc/offline-protocol/config.json` or point `OP_CONFIG` at one.
The default enables the peer stream with encryption required, and leaves
Bluetooth LE and the internet relay off. `profile` is this host's label and
part of its storage namespace: change it before the first start, never after.

`entrypoint.sh` maps environment variables to flags:

| Variable | Default | Flag |
|---|---|---|
| `OP_LISTEN` | `0.0.0.0:7878` | `--listen` |
| `OP_PEERS` | none | `--peer`, one per space-separated entry, for hosts multicast cannot reach |
| `OP_LAN` | `1` | `--lan` when `1`, none when `0`; anything else is refused |
| `OP_HTTP` | `127.0.0.1:8080` | `--http`; empty for no front |
| `OP_HTTP_TOKEN_FILE` | none | `--http-token-file`: the front writes a per-launch token there and requires it; needed off loopback, and on a host where a browser runs (R23 in the threat model) |
| `OP_HTTP_ALIASES` | none | `--http-aliases` |
| `OP_RELAY` | none | `--relay`, token in `OFFLINE_PROTOCOL_RELAY_TOKEN`; the config must set `internet_enabled`, which the default leaves off, or the service refuses to start |
| `OP_SOCKET` | `/run/offline-protocol/api.sock` | `--socket` |

Arguments after the image name are appended to the command.

The local API socket is inside the container. To reach it from the host,
bind-mount its directory; the service refuses a socket directory that is
not owned by its own user with mode 0700, so the host directory must be
root's and 0700.

## Building from a checkout

The image installs the released package from PyPI. A Linux wheel built from
a checkout, put in `wheels/`, is installed instead, even when PyPI has a
release with the same version number:

```bash
cp bindings/python/dist/offline_protocol_sdk-*-manylinux_*.whl bindings/python/docker/wheels/
docker build bindings/python/docker
```

Wheels for several architectures may sit there together; pip takes the one
that matches the image. The build fails if the installed service has no HTTP
front, which is the case for every release before the `http` extra.

The build context is this directory only. Never build from the repository
root or mount the checkout: `target/` alone fills the build VM.

## What has been run

Two containers from this image, on one bridge network (not host networking)
with no `OP_PEERS`, found each other over multicast DNS, announced each
other under their proved addresses, and served a request through the HTTP front from one to the
demo provider in [examples/http-front](../../../examples/http-front) on the
other, end to end encrypted, in 35 ms. The address and the front's
registrations survived a restart of the container. Bluetooth LE from a
container has not been run on hardware.
