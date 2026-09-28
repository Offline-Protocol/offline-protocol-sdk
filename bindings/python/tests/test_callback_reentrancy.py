"""Freeing a core object must never block, wherever the interpreter frees it.

The generated handle map guards every callback lookup and removal with one
lock. The collector can run a finalizer inside that critical section, a
core's drop releases its callbacks, and each release asks for the same lock
on the same thread. With a plain lock that thread waits for itself.
"""

from __future__ import annotations

import os
import subprocess
import sys
import threading
from pathlib import Path

import offline_protocol_sdk
from offline_protocol_sdk import offline_protocol as generated
from offline_protocol_sdk._callback_reentrancy import handle_maps

#: One process per case: a thread that waits for itself never returns, so a
#: failure here must not be able to take the rest of the suite with it.
_FREED_INSIDE_THE_MAP = r"""
import faulthandler
faulthandler.dump_traceback_later(30, exit=True)

from offline_protocol_sdk._callback_reentrancy import handle_maps
from offline_protocol_sdk import offline_protocol as generated
from offline_protocol_sdk.offline_protocol import (
    EventCallback, OfflineProtocol, OverflowPolicy, ProtocolConfig,
)

class Handler(EventCallback):
    def on_event(self, event_json):
        pass

core = OfflineProtocol(ProtocolConfig(
    app_id="test-app", profile="reentrancy-user", ble_enabled=True,
    wifi_direct_enabled=True, internet_enabled=True, reticulum_enabled=False,
    nostr_enabled=False, prefer_online=True, initial_ttl=3,
    encryption_enabled=True, auto_key_exchange=False, store_pending=True,
    require_encryption=False, max_pending_per_peer=100,
    max_pending_global=1000, pending_ttl_ms=60000,
    overflow_policy=OverflowPolicy.DROP_OLDEST,
))
core.set_event_callback(Handler())
(handle_map,) = handle_maps(generated)
registered = len(handle_map)
assert registered > 0

# What a callback lookup holds at the moment the collector finalizes a core.
with handle_map._lock:
    del core
assert len(handle_map) < registered, "the core released its callback"
print("freed")
"""


def _run(script: str) -> subprocess.CompletedProcess:
    package_parent = Path(offline_protocol_sdk.__file__).resolve().parent.parent
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join(
        [str(package_parent), environment.get("PYTHONPATH", "")]
    ).rstrip(os.pathsep)
    return subprocess.run(
        [sys.executable, "-c", script],
        env=environment,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )


def test_a_core_freed_inside_a_callback_lookup_does_not_wait_for_itself():
    outcome = _run(_FREED_INSIDE_THE_MAP)
    assert outcome.returncode == 0, outcome.stderr[-2000:]
    assert outcome.stdout.strip() == "freed"


def test_every_handle_map_the_bindings_hold_is_reentrant():
    """Found by looking, not by name: a regenerated binding that moves the
    map, or adds a second one, has to leave this green or be dealt with."""
    maps = handle_maps(generated)
    assert maps, "the generated bindings hold their callbacks in a handle map"
    reentrant = type(threading.RLock())
    for handle_map in maps:
        assert isinstance(handle_map._lock, reentrant)
        # And it is a working lock, entered twice by one thread.
        with handle_map._lock:
            with handle_map._lock:
                pass
