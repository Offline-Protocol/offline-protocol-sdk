"""Pins the root package's ``__all__`` to the attributes it exposes.

The package ships a PEP 561 marker, and under it a type checker treats a
bare ``from .x import Y`` in ``__init__.py`` as private: pyright (and so
Pylance) refuses ``from offline_protocol_sdk import ProtocolManager``, the
README's own quick start, and ``mypy --strict`` reports that the module
"does not explicitly export" it. Only a name in ``__all__`` is public. A
manager added to the root without a line in ``__all__`` imports fine at
runtime and fails only in a user's editor, so the list is checked here
against what the package really exposes.
"""

from __future__ import annotations

import inspect

import offline_protocol_sdk as sdk

# Written out, not derived: the names the README and the example import from
# the root, which the generated module's own ``__all__`` does not cover.
HAND_WRITTEN_EXPORTS = (
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
)


def _public_attributes() -> set[str]:
    # The imports in __init__.py also bind each submodule as an attribute.
    # Those stay reachable as ``offline_protocol_sdk.secure_storage`` whatever
    # ``__all__`` says, and are not part of the star-import surface.
    return {
        name
        for name, value in vars(sdk).items()
        if not name.startswith("_") and not inspect.ismodule(value)
    }


def test_all_names_every_public_attribute_and_nothing_else() -> None:
    listed = set(sdk.__all__)
    exposed = _public_attributes()
    assert listed == exposed, (
        f"missing from __all__: {sorted(exposed - listed)}; "
        f"listed but not exposed: {sorted(listed - exposed)}"
    )


def test_all_has_no_duplicates() -> None:
    assert len(sdk.__all__) == len(set(sdk.__all__))


def test_the_hand_written_managers_are_listed() -> None:
    for name in HAND_WRITTEN_EXPORTS:
        assert name in sdk.__all__, name


def test_the_generated_surface_is_listed() -> None:
    from offline_protocol_sdk import offline_protocol as generated

    assert set(generated.__all__) <= set(sdk.__all__)
