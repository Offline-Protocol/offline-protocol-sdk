"""Makes the generated callback handle map safe to re-enter.

The invariant: freeing a core object must never block, wherever the
interpreter decides to free it.

The generated bindings keep every callback object in one handle map, guarded
by a ``threading.Lock``. Rust takes that lock through the map on every
callback it makes (a lookup), and once more when it drops a callback (a
removal). The collector runs between any two bytecodes, the ones inside that
critical section included. When it finalizes a core object there, the core's
drop releases the callbacks it holds, each release calls back into the map,
and the map's lock is already held by the very thread that is asking for it.
A plain lock does not let its own holder in again, so the thread waits for
itself and the process hangs.

A core object reaches the collector whenever it is part of a reference
cycle: an event handler that is a method of the object owning the manager,
or an exception whose traceback names the manager. None of that is unusual.

A re-entrant lock removes the wait. Re-entry here is removal only (a
finalizer drops callbacks, it never registers one), and each of the map's
critical sections is a single dictionary operation, or a read followed by an
insert under a fresh handle, so an inner removal cannot leave an outer
operation looking at a half-changed map.

The generated file cannot carry this itself: it is regenerated from the UDL,
and the drift gate compares it byte for byte.
"""

from __future__ import annotations

import threading
from types import ModuleType
from typing import Any


def handle_maps(generated: ModuleType) -> list[Any]:
    """Every handle map the generated module holds, at module level or as a
    class attribute."""
    kind = getattr(generated, "_UniffiHandleMap", None)
    if kind is None:
        return []
    found: list[Any] = []
    for value in list(vars(generated).values()):
        if isinstance(value, kind):
            found.append(value)
        elif isinstance(value, type):
            found.extend(
                attribute
                for attribute in list(vars(value).values())
                if isinstance(attribute, kind)
            )
    return found


def make_handle_maps_reentrant(generated: ModuleType) -> int:
    """Gives every handle map a re-entrant lock. Returns how many it found.

    Called once, when the package is imported, before any callback exists.
    """
    maps = handle_maps(generated)
    for handle_map in maps:
        handle_map._lock = threading.RLock()
    return len(maps)
