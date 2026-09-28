"""Offline Protocol SDK — Python bindings for offline-first mesh networking."""

# Re-export all UniFFI-generated types (available after build-desktop.sh runs)
from .offline_protocol import *  # noqa: F401, F403

# Before anything registers a callback: see the module for the hang it prevents.
from . import offline_protocol as _generated
from ._callback_reentrancy import make_handle_maps_reentrant as _make_reentrant

_make_reentrant(_generated)
del _generated, _make_reentrant

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
