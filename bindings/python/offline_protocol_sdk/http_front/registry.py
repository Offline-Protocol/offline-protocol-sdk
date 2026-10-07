"""The services this front provides, kept in a file across restarts.

The engine forgets its registrations when it stops, and the local API forgets
who owns them. The front registers everything in this file again on every
connection, so a restart of either process leaves the services reachable.
"""

from __future__ import annotations

import json
import os
import tempfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from urllib.parse import urlsplit

from .hostname import is_label


@dataclass(frozen=True)
class Registration:
    service: str
    callback: str
    version: str = ""
    capabilities: dict[str, str] = field(default_factory=dict)

    @classmethod
    def parse(cls, service: str, doc: object) -> "Registration":
        if not is_label(service):
            raise ValueError(f"{service!r} is not a service label: lower-case letters, digits and hyphens")
        if not isinstance(doc, dict):
            raise ValueError("the body must be a JSON object")
        unknown = set(doc) - {"callback", "version", "capabilities"}
        if unknown:
            raise ValueError(f"unknown fields: {sorted(unknown)}")
        callback = doc.get("callback")
        if not isinstance(callback, str):
            raise ValueError("callback must be a URL")
        parts = urlsplit(callback)
        if parts.scheme not in ("http", "https") or not parts.netloc:
            raise ValueError("callback must be an http or https URL")
        version = doc.get("version", "")
        if not isinstance(version, str):
            raise ValueError("version must be a string")
        capabilities = doc.get("capabilities", {})
        if not isinstance(capabilities, dict) or not all(
            isinstance(k, str) and isinstance(v, str) for k, v in capabilities.items()
        ):
            raise ValueError("capabilities must be an object of strings")
        return cls(service, callback, version, dict(capabilities))

    def to_json(self) -> dict[str, object]:
        return asdict(self)


class Registry:
    """The registrations, written through to ``path`` when one is given."""

    def __init__(self, path: str | Path | None = None) -> None:
        self._path = Path(path) if path is not None else None
        self._entries: dict[str, Registration] = {}
        if self._path is not None and self._path.exists():
            doc = json.loads(self._path.read_text(encoding="utf-8"))
            for service, entry in (doc.get("services") or {}).items():
                self._entries[service] = Registration.parse(service, entry)

    def get(self, service: str) -> Registration | None:
        return self._entries.get(service)

    def all(self) -> list[Registration]:
        return [self._entries[k] for k in sorted(self._entries)]

    def put(self, registration: Registration) -> None:
        self._entries[registration.service] = registration
        self._save()

    def remove(self, service: str) -> bool:
        removed = self._entries.pop(service, None) is not None
        if removed:
            self._save()
        return removed

    def _save(self) -> None:
        if self._path is None:
            return
        self._path.parent.mkdir(parents=True, exist_ok=True)
        doc = {
            "services": {
                r.service: {"callback": r.callback, "version": r.version, "capabilities": r.capabilities}
                for r in self.all()
            }
        }
        fd, tmp = tempfile.mkstemp(prefix=".services-", dir=self._path.parent)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                json.dump(doc, handle, indent=2, sort_keys=True)
                handle.flush()
                os.fsync(handle.fileno())
            os.chmod(tmp, 0o600)
            os.replace(tmp, self._path)
        except BaseException:
            try:
                os.unlink(tmp)
            except FileNotFoundError:
                pass
            raise
