# Python: wheels, PyPI, and fallbacks

This guide answers `pip install` on common hosts and what to do when no wheel
matches your platform.

Libraries and gateways: start here. Container deploys: [Docker service](#docker-one-liner)
below. API details: [bindings/python/README.md](../bindings/python/README.md).

## Blessed path: PyPI wheels

```bash
pip install offline-protocol-sdk
```

Gateway / LAN / HTTP extras:

```bash
pip install 'offline-protocol-sdk[lan,http]'
```

Python **3.10+**. Each release publishes **four** platform wheels (no source
distribution). Each wheel bundles the native library for that platform only.

| Host | Wheel ships? | Tag shape (example) |
|------|--------------|---------------------|
| macOS, Apple Silicon | Yes | `macosx_14_0_arm64` |
| Linux x86_64 (glibc) | Yes | `manylinux_2_<N>_x86_64` (N = glibc floor read from the release library) |
| Linux aarch64 (glibc) | Yes | `manylinux_2_<N>_aarch64` |
| Windows x86_64 | Yes | `win_amd64` |

The Linux `manylinux_2_*` number is **not** fixed in documentation: the release
train measures the linked glibc floor from each library
(`bindings/python/scripts/build-wheel.sh`). If `pip` rejects a wheel as
incompatible, your glibc is older than that release built against: use
[Docker](#docker-one-liner) or [build from source](../bindings/python/README.md#building-from-source).

### Explicitly unsupported on `pip install`

There is **no** wheel for:

| Host | Why | What to do |
|------|-----|------------|
| macOS Intel (x86_64) | CI builds arm64 macOS only | Apple Silicon Mac with wheels; Intel: [build from source](../bindings/python/README.md#building-from-source) or run the [service image](#docker-one-liner) on Linux |
| Alpine / musl Linux | No musllinux wheel | [Docker one-liner](#docker-one-liner) on glibc Linux, or build from source on Alpine |
| Older glibc than the wheel tag | Tag promises a minimum glibc | Same as above |
| iOS / Android | Desktop binding only | React Native, Flutter, or native mobile guides |

There is **no sdist** (`pip` cannot compile the Rust core on install). That is
intentional: the tagged wheel must match the bundled `.so` / `.dylib` / `.dll`.

## Docker one-liner

When `pip` has no matching wheel, run the **published service image** (glibc
Linux, amd64 and arm64). It installs `offline-protocol-sdk[lan,http]` from PyPI
at the image tag:

```bash
export OFFLINE_PROTOCOL_STORE_KEY="$(openssl rand -hex 32)"
docker pull ghcr.io/offline-protocol/offline-protocol-service:0.33.0
docker run --rm -it \
  --network host \
  -e OFFLINE_PROTOCOL_STORE_KEY \
  -v offline-protocol-data:/var/lib/offline-protocol \
  ghcr.io/offline-protocol/offline-protocol-service:0.33.0
```

Pin the digest in production (`docker buildx imagetools inspect …`). Full
compose, grants, and LAN networking:
[bindings/python/docker/README.md](../bindings/python/docker/README.md).

## Upgrading

Bump the PyPI version to the SDK release you adopt (`X.Y.Z` with tag `vX.Y.Z`).
On Linux, confirm the new wheel tag still matches your glibc before rolling out
hosts.

`ProtocolManager` requires an explicit `state_root` (application-owned data).
See [UPGRADING.md](UPGRADING.md) for storage-split upgrades and service templates.

## Maintainers

Wheels are built in `release.yml` (`python-package`, `test-python-wheels`),
published when `PYPI_PUBLISH` is on. Platform list is fixed in
`build-wheel.sh` (`macos-arm64`, `linux-x86_64`, `linux-aarch64`,
`windows-x86_64`). Expanding that list is a separate release-engineering change.
