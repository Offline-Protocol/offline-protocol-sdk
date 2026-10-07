"""``<service>.<device>.<domain>``: what a request's host names.

A device label is the address itself or an alias from the operator's file,
never one learned from the LAN or from discovery (invariant 3 of
``docs/spec/http-front.md``): an alias a peer could set would bind a
friendly name to that peer's own, authentic, address.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

DEFAULT_DOMAIN = "offline.protocol.internal"

LABEL = re.compile(r"^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$")
#: ``off1`` and 40 characters of the bech32 alphabet: an address is 44
#: characters of ``[a-z0-9]``, so it is a DNS label as it stands.
ADDRESS = re.compile(r"^off1[qpzry9x8gf2tvdw0s3jn54khce6mua7l]{40}$")


class HostError(ValueError):
    """A host under the domain that is not ``<service>.<device>``."""


@dataclass(frozen=True)
class Target:
    service: str
    device: str


def is_address(label: str) -> bool:
    return ADDRESS.fullmatch(label) is not None


def is_label(label: str) -> bool:
    return LABEL.fullmatch(label) is not None


def _bare_host(host: str) -> str:
    host = host.strip().lower()
    if host.startswith("["):
        return host  # an IPv6 literal: never under the domain
    name, sep, port = host.rpartition(":")
    if sep and port.isdigit():
        host = name
    return host.rstrip(".")


def parse_host(host: str | None, domain: str = DEFAULT_DOMAIN) -> Target | None:
    """The target a host names, or ``None`` when the request is for the
    front's own endpoints (the domain itself, or any host not under it)."""
    if not host:
        return None
    bare = _bare_host(host)
    suffix = "." + domain
    if bare == domain or not bare.endswith(suffix):
        return None
    labels = bare[: -len(suffix)].split(".")
    if len(labels) != 2 or not all(labels):
        raise HostError(f"{bare} is not <service>.<device>.{domain}")
    service, device = labels
    if not is_label(service):
        raise HostError(f"{service!r} is not a service label")
    if not (is_label(device) or is_address(device)):
        raise HostError(f"{device!r} is not a device label")
    return Target(service, device)


class Aliases:
    """The operator's alias file: ``{"aliases": {"bob": "off1…"}}``."""

    def __init__(self, aliases: Mapping[str, str] | None = None) -> None:
        self._by_alias: dict[str, str] = {}
        for alias, address in (aliases or {}).items():
            self._add(alias, address)

    def _add(self, alias: str, address: str) -> None:
        if not isinstance(alias, str) or not is_label(alias):
            raise ValueError(f"alias {alias!r} is not a DNS label")
        if is_address(alias):
            raise ValueError(f"alias {alias!r} looks like an address and would shadow one")
        if not isinstance(address, str) or not is_address(address):
            raise ValueError(f"alias {alias!r} does not map to an address")
        self._by_alias[alias] = address

    @classmethod
    def load(cls, path: str | Path) -> "Aliases":
        doc = json.loads(Path(path).read_text(encoding="utf-8"))
        if not isinstance(doc, dict) or set(doc) - {"aliases"} or not isinstance(doc.get("aliases", {}), dict):
            raise ValueError('the alias file must be {"aliases": {"name": "off1…"}}')
        return cls(doc.get("aliases", {}))

    def resolve(self, device: str) -> str | None:
        if is_address(device):
            return device
        return self._by_alias.get(device)

    def alias_of(self, address: str) -> str | None:
        for alias, value in self._by_alias.items():
            if value == address:
                return alias
        return None
