# One Offline Protocol service per host: the engine, the local API, the
# transports its configuration enables, and the HTTP front for applications
# that do not use the SDK. See README.md beside this file.
#
# The base must be glibc 2.34 or newer (the wheel is manylinux_2_34) and
# never Alpine (musl has no wheel). python:3.12-slim is Debian.
FROM python:3.12-slim

# What to install. The default installs the released package from PyPI; to
# install a wheel built from a checkout instead, put it in wheels/ beside
# this file and pass --build-arg SDK_SPEC='offline-protocol-sdk[lan,http]==<version>'.
# Never COPY the checkout itself: the build context is this directory only.
ARG SDK_SPEC="offline-protocol-sdk[lan,http]"

COPY wheels/ /wheels/
RUN pip install --no-cache-dir --find-links /wheels "${SDK_SPEC}" \
    && rm -rf /wheels

COPY config.json /etc/offline-protocol/config.json
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
