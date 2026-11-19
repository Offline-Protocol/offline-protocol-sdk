# Python: wheels, PyPI, and fallbacks

This page is for Python developers putting the SDK on a desktop, a server or a
gateway. It tells you which hosts `pip install` covers, and what to run on a
host it does not. For the API itself, read
[bindings/python/README.md](../bindings/python/README.md).

## Install from PyPI

```bash
pip install offline-protocol-sdk
```

A host that runs the service (a gateway, a site computer) also wants the `lan`
and `http` extras:

```bash
pip install 'offline-protocol-sdk[lan,http]'
```

The SDK needs Python 3.10 or later. Each release publishes four wheels, one per
platform, and each carries the native library for its platform, so there is
nothing to compile:

| Host | Wheel tag | Requires |
|------|-----------|----------|
| macOS, Apple Silicon (arm64) | `macosx_14_0_arm64` | macOS 14 or later |
| Linux x86_64 | `manylinux_2_34_x86_64` | glibc 2.34 or later |
| Linux aarch64 | `manylinux_2_34_aarch64` | glibc 2.34 or later |
| Windows x86_64 | `win_amd64` | |

The release reads each Linux wheel's glibc floor from the library it carries
(`bindings/python/scripts/build-wheel.sh`), so the tag always states what the
library needs. Debian 12, Ubuntu 22.04 and RHEL 9 meet the 2.34 floor; if a
release raises it, the wheel's tag on PyPI shows the new number.

After installing, set `state_root` explicitly when you construct
`ProtocolManager`: it is where your application's data lives.

## Hosts without a wheel

There is no wheel for the hosts below, and no source distribution, so on them
`pip` finds nothing to install. The wheel is tagged for the native library it
bundles, and an sdist would ask `pip` to compile the Rust core on install.

| Host | What to run |
|------|-------------|
| macOS on Intel (x86_64) | [Build from source](../bindings/python/README.md#building-from-source). The service image needs a Linux host, so it is not an option on the Mac itself. |
| Alpine or another musl Linux | The [service image](#docker-one-liner): it brings its own glibc, so it runs on any Linux host with Docker, Alpine included. Or build from source on the host. |
| Linux with glibc older than 2.34 | The [service image](#docker-one-liner), or build from source on the host. |
| iOS and Android | The [React Native](react-native-integration.md), [Flutter](flutter-distribution.md), [iOS](ios-native-distribution.md) or [Android](android-integration.md) guides: the Python package is for desktops and servers. |

## Docker one-liner

The service image runs the SDK as a service on a Linux host, for amd64 and
arm64, with `offline-protocol-sdk[lan,http]` installed from PyPI at the image's
version. Your application does not import the SDK in this setup: it calls the
service's [HTTP front](spec/http-front.md) on `127.0.0.1:8080`, or the
[local API](local-api.md) over its socket.

The recipe uses host networking, so it needs a Linux host. On Docker Desktop
(macOS, Windows) host networking is the namespace of Docker's own virtual
machine, and the service cannot reach your LAN.

Once per host, generate a store key and save it outside the data volume. Reuse
it on every start that mounts the same volume: everything on the volume is
sealed under it, and a new key with an existing `offline-protocol-data` volume
fails with `InvalidConfiguration`.

```bash
(
  umask 077
  mkdir -p ~/.offline-protocol
  openssl rand -hex 32 > ~/.offline-protocol/store-key
)
```

Then, on each run, load the saved key and set `OP_LISTEN` to the host's
reserved LAN address. Leave it at the default `0.0.0.0` and the service
advertises `docker0` as well, which breaks LAN peering; the
[container README](../bindings/python/docker/README.md) explains why.

```bash
export OFFLINE_PROTOCOL_STORE_KEY="$(cat ~/.offline-protocol/store-key)"
export OP_LISTEN="192.168.1.10:7878"   # this host's LAN address
docker pull ghcr.io/offline-protocol/offline-protocol-service:0.34.0
docker run --rm -it \
  --network host \
  -e OFFLINE_PROTOCOL_STORE_KEY \
  -e OP_LISTEN \
  -v offline-protocol-data:/var/lib/offline-protocol \
  ghcr.io/offline-protocol/offline-protocol-service:0.34.0
```

In production, pin the image by digest, which
`docker buildx imagetools inspect ghcr.io/offline-protocol/offline-protocol-service:0.34.0`
prints. The [container README](../bindings/python/docker/README.md) covers
Compose, the grants a host must give the container, and LAN networking.

## Upgrading

Move to a new release by bumping the PyPI version to it (`X.Y.Z`, tagged
`vX.Y.Z`). On Linux, check that the new wheel's tag still matches your glibc
before rolling it out. [UPGRADING.md](UPGRADING.md) lists what each release
changes for Python, including storage-split upgrades and service templates.

## Maintainers

The `python-package` job in `release.yml` builds the wheels and
`test-python-wheels` installs and tests them; they are published when
`PYPI_PUBLISH` is on. The platform list is the job's matrix and the cases in
`build-wheel.sh`, and `python_wheel_distribution_path_is_documented` holds this
page to both. Adding a platform is a release-engineering change, and it fails
that test until the page names it.
