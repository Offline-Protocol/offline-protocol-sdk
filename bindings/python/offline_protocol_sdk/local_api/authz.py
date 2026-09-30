"""The three server-side rules that separate the applications behind one
identity: service ownership, space scoping and method groups.

Each is applied before a call reaches the engine, each refuses with the
engine's ``PermissionDenied``, and none has any representation on the mesh
(chapter, "Server-side rules"; bridge rule L3). The engine sees one identity
registering services and syncing spaces, exactly as it would from one
application.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from fnmatch import fnmatchcase
from typing import Any

from .codec import taxonomy_error

#: The named method groups an operator may deny to an application id. A
#: policy entry that is not a group name is a single method's wire name.
METHOD_GROUPS: dict[str, frozenset[str]] = {
    "sign_data": frozenset({"sign_data"}),
    "manual_mls": frozenset(
        {
            "mls_generate_key_package",
            "mls_get_or_create_key_package",
            "mls_import_key_package",
            "mls_get_pending_key_packages",
            "mls_mark_key_package_synced",
            "mls_create_session",
            "mls_join_session",
            "mls_encrypt_for_user",
            "mls_decrypt_from_user",
            "mls_get_pending_welcome",
            "mls_clear_pending_welcome",
            "mls_decrypt",
            "mls_process_welcome",
        }
    ),
    "tuning": frozenset(
        {
            "set_relay_priority",
            "update_relay_config",
            "force_transport",
            "release_transport_lock",
            "update_dors_config",
            "update_ack_config",
            "update_retry_config",
            "update_dedup_config",
        }
    ),
}


@dataclass
class Policy:
    """The operator's configuration of the optional rules.

    ``spaces`` maps an application id to the glob patterns over space ids it
    may open; an application with no entry may open every space. ``denied``
    maps an application id to method group names and single method names it
    may not call; nothing is denied by default. ``applications`` lists ids
    that have no rule of their own but are still admitted once a rule exists;
    on its own it configures nothing.

    An application id is self-declared per connection, so a rule keyed by
    id separates applications from each other's mistakes, never from a
    hostile local process, which is inside the socket boundary already.
    What keeps a rule from being stepped around by a reconnect under
    another name is the chapter's unlisted-id rule: once a space allow-list
    or a method deny is configured, ``hello`` with an id no entry names is
    refused. With neither configured, every id is admitted. Service
    ownership is a runtime shadow, never a configured rule, and never counts.
    """

    spaces: dict[str, list[str]] = field(default_factory=dict)
    denied: dict[str, list[str]] = field(default_factory=dict)
    applications: list[str] = field(default_factory=list)

    @classmethod
    def from_dict(cls, raw: dict[str, Any] | None) -> "Policy":
        raw = raw or {}
        unknown = sorted(set(raw) - {"spaces", "denied", "applications"})
        if unknown:
            raise ValueError(f"policy has no section {unknown[0]!r}")
        spaces = {str(k): [str(p) for p in v] for k, v in (raw.get("spaces") or {}).items()}
        denied = {str(k): [str(m) for m in v] for k, v in (raw.get("denied") or {}).items()}
        applications = [str(a) for a in (raw.get("applications") or [])]
        for app_id, names in denied.items():
            for name in names:
                if name not in METHOD_GROUPS and "." not in name and not name.isidentifier():
                    raise ValueError(f"policy denies {name!r} for {app_id!r}: not a method or group")
        return cls(spaces=spaces, denied=denied, applications=applications)

    def restricts(self) -> bool:
        """Whether a space allow-list or a method deny is configured."""
        return bool(self.spaces or self.denied)

    def admits(self, app_id: str) -> bool:
        """Whether ``hello`` may declare ``app_id``."""
        if not self.restricts():
            return True
        return app_id in self.spaces or app_id in self.denied or app_id in self.applications

    def check_admission(self, app_id: str) -> None:
        if not self.admits(app_id):
            raise taxonomy_error(
                "PermissionDenied",
                f"no rule names application {app_id!r}; rules are configured, so unnamed ids are refused",
            )

    def allows_space(self, app_id: str, space_id: str) -> bool:
        patterns = self.spaces.get(app_id)
        if patterns is None:
            return True
        return any(fnmatchcase(space_id, pattern) for pattern in patterns)

    def denies(self, app_id: str, method: str) -> bool:
        for name in self.denied.get(app_id, ()):
            if name == method or method in METHOD_GROUPS.get(name, ()):
                return True
        return False

    def check_space(self, app_id: str, space_id: str) -> None:
        if not self.allows_space(app_id, space_id):
            raise taxonomy_error(
                "PermissionDenied",
                f"space {space_id!r} is outside what {app_id!r} may open",
            )

    def check_method(self, app_id: str, method: str) -> None:
        if self.denies(app_id, method):
            raise taxonomy_error("PermissionDenied", f"{method} is denied to {app_id!r}")


class ServiceOwnership:
    """A shadow of the engine's service registry, keyed by application id.

    The engine has no owner on a registration and no way to enumerate them,
    so this is rebuilt from the clients' calls and is empty after a restart
    until they register again. An id another application already holds is
    refused, which is the collision the registry exists to catch: without it
    the second registration silently replaces the first in the engine and
    the first application's requests start arriving at the second.
    """

    def __init__(self) -> None:
        self._owner: dict[str, str] = {}

    def owner(self, service_id: str) -> str | None:
        return self._owner.get(service_id)

    def claim(self, service_id: str, app_id: str) -> None:
        holder = self._owner.get(service_id)
        if holder is not None and holder != app_id:
            raise taxonomy_error(
                "PermissionDenied",
                f"service {service_id!r} is registered by another application",
            )
        self._owner[service_id] = app_id

    def release(self, service_id: str) -> None:
        self._owner.pop(service_id, None)

    def check(self, service_id: str, app_id: str) -> None:
        holder = self._owner.get(service_id)
        if holder is not None and holder != app_id:
            raise taxonomy_error(
                "PermissionDenied",
                f"service {service_id!r} belongs to another application",
            )
