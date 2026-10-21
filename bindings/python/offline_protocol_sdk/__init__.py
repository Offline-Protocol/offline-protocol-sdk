"""Offline Protocol SDK — Python bindings for offline-first mesh networking."""

# Re-export all UniFFI-generated types (available after build-desktop.sh runs)
from .offline_protocol import *  # noqa: F401, F403

# Before anything registers a callback: see the module for the hang it prevents.
from . import offline_protocol as _generated
from ._callback_reentrancy import make_handle_maps_reentrant as _make_reentrant

_make_reentrant(_generated)

# Platform managers
from .secure_storage import SecureStorage  # noqa: F401
from .state_storage import AppStateStorage  # noqa: F401
from .storage_namespace import account_storage_namespace  # noqa: F401
from .transport_manager import TransportManager, TransportState  # noqa: F401
from .internet_manager import InternetManager  # noqa: F401
from .peer_stream_manager import PeerEntry, PeerStreamManager  # noqa: F401
from .ble_manager import BleManager  # noqa: F401
from .ble_peripheral import BlePeripheral  # noqa: F401
from .protocol_manager import ProtocolManager  # noqa: F401

# The public surface, by name. The package ships a PEP 561 marker, and under
# it a type checker treats a bare `from .x import Y` above as private: pyright
# (and so Pylance) refuses `from offline_protocol_sdk import ProtocolManager`,
# the README's own quick start, and `mypy --strict` reports that the module
# "does not explicitly export" it. Only the `+=` forms below are ones every
# checker is required to read, so no unpacking or `list()` here.
# tests/test_public_surface.py pins this list to the attributes the package
# exposes, so a manager added above without a line here fails in CI rather
# than in a user's editor.
__all__ = []
__all__ += _generated.__all__
__all__ += [
    "SecureStorage",
    "AppStateStorage",
    "account_storage_namespace",
    "TransportManager",
    "TransportState",
    "InternetManager",
    "PeerEntry",
    "PeerStreamManager",
    "BleManager",
    "BlePeripheral",
    "ProtocolManager",
]

del _generated, _make_reentrant
