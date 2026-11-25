"""The daemon's configuration, read from TOML.

A gateway is provisioned (ADR 0016): a person installs it and writes this
file. Every bound has a default, and every default is chosen for a scarce
backbone, because that is the kind this reference daemon ships with.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass, field, fields
from pathlib import Path
from typing import Any

from . import wire

#: The reference clients' attach timeout. The verdict-only grace may not be
#: shorter, or a client whose declaration is merely slow is announced out
#: from under it, and the contract lets it close on that.
CLIENT_ATTACH_TIMEOUT = 10.0
#: The reference clients' verdict timeout. A backbone verdict that takes
#: longer arrives after the client has already failed the frame.
CLIENT_VERDICT_TIMEOUT = 60.0


class ConfigError(ValueError):
    pass


@dataclass
class ReticulumConfig:
    """The Reticulum backbone. Absent from the file means no backbone."""

    #: Reticulum's own configuration directory (its interfaces, an RNode's
    #: port and radio parameters). ``None`` is Reticulum's default.
    configdir: str | None = None
    #: Where this gateway's Reticulum identity is kept. Created on first run.
    #: The destination its peers provision is derived from it, so losing it
    #: means re-provisioning every peer.
    identity_path: str = "gateway.identity"
    #: Peer gateways' destination hashes, hex, as each one prints at start.
    peers: list[str] = field(default_factory=list)
    #: Seconds between announces of this gateway's destination.
    announce_interval: float = 1800.0
    #: Seconds between attempts to (re)open a link to a peer.
    reconnect_interval: float = 30.0
    #: Reticulum log level, 0 (critical) to 7 (extreme).
    loglevel: int = 2


@dataclass
class GatewayConfig:
    listen_host: str = "127.0.0.1"
    listen_port: int = 4242
    #: Seconds an undeclared session waits before it is announced
    #: verdict-only. ``0`` never announces one.
    attach_grace: float = 15.0
    #: Concurrent device connections.
    max_sessions: int = 256
    #: Of those, sessions that have not proved an address. A newcomer evicts
    #: the oldest of them when this or ``max_sessions`` is full, so sockets
    #: that never attach cannot lock devices out.
    max_unbound_sessions: int = 32
    #: Sessions from one source address. Each device on a hotspot or venue
    #: network has its own; a device reconnecting while its old socket is
    #: still open needs two.
    max_sessions_per_host: int = 8
    #: Frames waiting to be written to one device before it is dropped as
    #: too slow to serve.
    max_queued_frames: int = 512
    #: Largest frame a device may submit, in bytes, before base64.
    max_frame_bytes: int = 256 * 1024
    #: Largest frame carried over the backbone. Smaller, because the backbone
    #: is scarce: a frame past this is refused ``frame_too_large``, which is
    #: how media stays off a LoRa-class link.
    max_backbone_frame_bytes: int = 16 * 1024
    #: Submissions per minute per device, and the burst it may spend at once.
    device_per_minute: float = 120.0
    device_burst: float = 30.0
    #: Backbone frames per minute per device, on a scarce backbone only. The
    #: one that matters: it is what stops one device from spending the whole
    #: link. The burst has to cover a first contact: two SDK engines pairing
    #: over MLS and exchanging one message cost 18 to 20 frames each
    #: (measured by ``test_end_to_end.py``), and a burst smaller than that
    #: refuses the pairing's own frames and stalls it on the retry ladder.
    backbone_per_minute: float = 6.0
    backbone_burst: float = 24.0
    #: Frames per minute each peer gateway may hand this one.
    peer_per_minute: float = 60.0
    peer_burst: float = 20.0
    #: Seconds a backbone forward may take before the device is told
    #: ``backbone_timeout``. Must leave the client's 60 s room for the answer.
    backbone_verdict_timeout: float = 45.0
    reticulum: ReticulumConfig | None = None

    def validate(self) -> None:
        if not 0 <= self.listen_port < 65536:
            raise ConfigError("listen_port must be 0 (any free port) to 65535")
        if self.attach_grace and self.attach_grace < CLIENT_ATTACH_TIMEOUT:
            raise ConfigError(
                f"attach_grace must be 0 or at least {CLIENT_ATTACH_TIMEOUT:g} s, the clients' attach timeout"
            )
        if not 1 <= self.max_unbound_sessions <= self.max_sessions:
            raise ConfigError("max_unbound_sessions must be 1 to max_sessions")
        if self.max_sessions_per_host < 1:
            raise ConfigError("max_sessions_per_host must be at least 1")
        if self.max_frame_bytes <= 0 or self.max_backbone_frame_bytes <= 0:
            raise ConfigError("frame limits must be positive")
        # A MessageReceived carries the frame base64-encoded inside JSON, and
        # the whole line must fit the clients' 1 MiB bound.
        if self.max_frame_bytes * 4 // 3 + 4096 > wire.MAX_LINE_BYTES:
            raise ConfigError("max_frame_bytes is too large to deliver inside a 1 MiB line")
        if not 0 < self.backbone_verdict_timeout < CLIENT_VERDICT_TIMEOUT - 5:
            raise ConfigError(
                f"backbone_verdict_timeout must be under {CLIENT_VERDICT_TIMEOUT - 5:g} s, "
                "or the client fails the frame before the verdict arrives"
            )
        for name in ("device_burst", "backbone_burst", "peer_burst"):
            if getattr(self, name) <= 0:
                raise ConfigError(f"{name} must be positive")
        if self.reticulum is not None:
            for peer in self.reticulum.peers:
                try:
                    if len(bytes.fromhex(peer)) != 16:
                        raise ValueError
                except ValueError:
                    raise ConfigError(f"peer {peer!r} is not a 16-byte destination hash in hex") from None


def load(path: str | Path) -> GatewayConfig:
    """Reads a config file. Relative paths in it are relative to the file, so
    a gateway's directory can be moved whole."""
    with open(path, "rb") as f:
        data = tomllib.load(f)
    config = from_dict(data)
    base = Path(path).resolve().parent
    if config.reticulum is not None:
        r = config.reticulum
        r.identity_path = str(base / r.identity_path)
        if r.configdir is not None:
            r.configdir = str(base / r.configdir)
    return config


def from_dict(data: dict[str, Any]) -> GatewayConfig:
    data = dict(data)
    reticulum = data.pop("reticulum", None)
    config = GatewayConfig(**_known(GatewayConfig, data, "gateway"))
    if reticulum is not None:
        if not isinstance(reticulum, dict):
            raise ConfigError("[reticulum] must be a table")
        config.reticulum = ReticulumConfig(**_known(ReticulumConfig, reticulum, "reticulum"))
    config.validate()
    return config


def _known(cls: type, data: dict[str, Any], section: str) -> dict[str, Any]:
    names = {f.name for f in fields(cls)} - {"reticulum"}
    unknown = set(data) - names
    if unknown:
        # A misspelled bound silently falling back to its default is a
        # budget nobody applies, so a key this daemon does not know refuses
        # the file.
        raise ConfigError(f"unknown key(s) in [{section}]: {', '.join(sorted(unknown))}")
    return data
