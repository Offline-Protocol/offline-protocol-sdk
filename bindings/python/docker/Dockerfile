# One Offline Protocol service per host: the engine, the local API, the
# transports its configuration enables, and the HTTP front for applications
# that do not use the SDK. See README.md beside this file.
#
# The base must be glibc 2.34 or newer (the wheel is manylinux_2_34) and
# never Alpine (musl has no wheel). python:3.12-slim is Debian.
FROM python:3.12-slim

# What to install. With no wheel in wheels/, the released package from PyPI.
# A wheel built from a checkout, put in wheels/, is installed instead: by
# itself first, so a release with the same version number on PyPI can never
# take its place, and then the extras' dependencies. The build fails unless
# the installed service has the HTTP front, which a release older than the
# `http` extra lacks: that image would otherwise build and then crash-loop on
# `unrecognized arguments: --lan --http`.
# SDK_SPEC must keep both extras, and must carry no version a local wheel
# fails, or the second install replaces the wheel; the check below also
# imports both extras' libraries, since the flag exists without them.
# Never COPY the checkout itself: the build context is this directory only.
ARG SDK_SPEC="offline-protocol-sdk[lan,http]"
# SDK_VERSION installs exactly that release from PyPI (its Python spelling:
# 0.30.0rc1, not 0.30.0-rc.1), and the build fails unless that is the version
# installed. The published image passes the release's version, so it holds the
# package PyPI has under that number and no later one. Empty, the default,
# installs a wheel from wheels/ or else PyPI's newest release. Set together
# with a wheel in wheels/ it is refused: PyPI's package would silently replace
# the checkout's.
ARG SDK_VERSION=""

COPY wheels/ /wheels/
RUN set -eu; \
    spec="${SDK_SPEC}"; \
    if [ -n "${SDK_VERSION}" ]; then \
        if ls /wheels/*.whl >/dev/null 2>&1; then \
            echo "SDK_VERSION installs a release from PyPI: empty wheels/, or leave SDK_VERSION unset to install the wheel" >&2; exit 1; \
        fi; \
        spec="${SDK_SPEC}==${SDK_VERSION}"; \
    fi; \
    if ls /wheels/*.whl >/dev/null 2>&1; then \
        pip install --no-cache-dir --no-deps --no-index --find-links /wheels offline-protocol-sdk; \
    fi; \
    pip install --no-cache-dir --find-links /wheels "${spec}"; \
    rm -rf /wheels; \
    if [ -n "${SDK_VERSION}" ]; then \
        installed="$(python -c 'from importlib.metadata import version; print(version("offline-protocol-sdk"))')"; \
        [ "${installed}" = "${SDK_VERSION}" ] \
            || { echo "SDK_VERSION is ${SDK_VERSION}, and pip installed ${installed}" >&2; exit 1; }; \
    fi; \
    { offline-protocol-service --help | grep -q -- '--http ' && python -c 'import aiohttp, zeroconf'; } \
        || { echo "the installed offline-protocol-service has no HTTP front: put a wheel built from a checkout in wheels/" >&2; exit 1; }; \
    command -v offline-protocol-verify >/dev/null \
        || { echo "the installed package has no offline-protocol-verify: put a wheel built from a checkout in wheels/" >&2; exit 1; }

# The baked configuration, and two to mount or name with OP_CONFIG: one
# with Bluetooth LE on, one with the internet relay on.
COPY config.json config-ble.json config-relay.json /etc/offline-protocol/
COPY entrypoint.sh /usr/local/bin/offline-protocol-entrypoint
RUN chmod 0755 /usr/local/bin/offline-protocol-entrypoint \
    && mkdir -p /var/lib/offline-protocol

# The identity and every queued message live here: mount a volume, or the
# device gets a new address every time the container is recreated.
VOLUME ["/var/lib/offline-protocol"]

# Peer streams from other hosts. The HTTP front listens on loopback inside
# the host's network namespace and is not exposed.
EXPOSE 7878/tcp

ENTRYPOINT ["/usr/local/bin/offline-protocol-entrypoint"]
