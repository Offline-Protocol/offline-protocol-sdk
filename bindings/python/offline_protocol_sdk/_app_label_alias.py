"""``app_id`` as a deprecated keyword for the app label.

Before 0.35 the mesh value was called ``app_id`` and developers mixed it up
with the App ID (``app_…``) the developer portal issues for telemetry. The
UDL now names it ``app_label``, and the generated classes take only that
keyword, so code written for 0.34 would fail at the constructor. This keeps
``app_id=`` working on the three generated classes that carried it, with a
``DeprecationWarning``, and refuses the two names set to different values
rather than letting one silently win.

Only the name changed. The value still selects the storage namespace and is
stamped on every frame, so an app that shipped must keep passing the same
string.
"""

from __future__ import annotations

import functools
import warnings
from types import ModuleType
from typing import Any

MESSAGE = (
    "app_id on the mesh is now app_label. Your App ID (app_…) from the "
    "dashboard is what turns on telemetry."
)

_MISSING = object()

# The generated classes whose ``app_id`` field is now ``app_label``.
CLASSES = ("ProtocolConfig", "SendMessageOptions", "MediaSendOptions")


def _warn(stacklevel: int) -> None:
    warnings.warn(MESSAGE, DeprecationWarning, stacklevel=stacklevel + 1)


def _alias(cls: type) -> None:
    original = cls.__init__

    @functools.wraps(original)
    def __init__(self: Any, *args: Any, app_id: Any = _MISSING, **kwargs: Any) -> None:
        if app_id is not _MISSING:
            _warn(2)
            label = kwargs.get("app_label", _MISSING)
            if label is not _MISSING and label != app_id:
                raise TypeError(
                    f"app_label {label!r} and app_id {app_id!r} disagree; "
                    "app_id is the old name of app_label, pass only app_label"
                )
            kwargs["app_label"] = app_id
        original(self, *args, **kwargs)

    def _get(self: Any) -> Any:
        _warn(2)
        return self.app_label

    def _set(self: Any, value: Any) -> None:
        _warn(2)
        self.app_label = value

    cls.__init__ = __init__  # type: ignore[method-assign]
    cls.app_id = property(_get, _set, doc="Deprecated: the old name of app_label.")  # type: ignore[attr-defined]


def install(generated: ModuleType) -> None:
    """Adds the alias to the generated classes, once."""
    for name in CLASSES:
        cls = getattr(generated, name)
        if not getattr(cls, "_app_label_alias", False):
            _alias(cls)
            cls._app_label_alias = True  # type: ignore[attr-defined]
