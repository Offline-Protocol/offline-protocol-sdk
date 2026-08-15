# Python bridge contract

Covers the generated Python bindings and the desktop package.

Read [the shared contract](README.md) first. This document covers what is
specific to Python.

## The generated layer

UniFFI produces Python from the interface definition. It is one third of the
artifact set described in [C1](README.md#c1-regenerate-every-binding-together)
and is never regenerated alone.

The desktop build script delegates to the same generation script, and CI builds
the desktop package through it.

## P1. This is the thinnest binding, and that makes it the ABI canary

Python has no platform integration to speak of: no Keychain, no Keystore, no
Bluetooth stack, no React Native lifecycle. It is close to a direct view of the
interface definition.

That makes it the cheapest place to detect an ABI break. If a change makes the
Python package fail to import or a call fail on a checksum, the Swift and Kotlin
bindings have the same problem and will surface it later, on a device, in a
harder-to-diagnose form.

Run the Python tests before the mobile ones when changing the interface.

## P2. Nothing here is a reference implementation of platform concerns

The Python package does not implement secure storage against a platform keystore.
Do not copy its storage handling into a mobile binding, and do not treat its
behaviour as the contract for one.

It does carry one shared constant, which is easy to miss precisely because the
rest of the binding is platform-free: `state_storage.py` holds one of the four
copies of the protocol-state record ceiling, and it must spell
`8 * 1024 * 1024` exactly. A Rust guard reads this file, so editing the ceiling
in the mobile bindings and not here fails the **Rust** suite, not `pytest`. See
[C5](README.md#c5-hand-mirrored-constants-must-be-pinned-in-every-language).

## P3. Packaging

The package is versioned in lockstep with the workspace. A release cut touches
the Python project metadata along with the Cargo manifests, the lockfile, and the
third-party notices.

## P4. The error enum is positional here too

The same append-only rule applies. See
[C2](README.md#c2-the-error-enum-is-append-only).

## P5. Events

Events arrive as JSON strings, exactly as in the other bindings. Python's
dynamism makes it tempting to consume them ad hoc, and that is fine for tooling,
but it means the Python surface offers no drift protection at all. It will not
catch a renamed event field for you.

## Testing

```bash
cd bindings/python
# build the desktop library first, then
pytest
```

Tests live in `bindings/python/tests/`. The build script under
`bindings/python/scripts/` produces the library the tests load.

## Note on packaging tests

Guard tests that assert on repository layout **panic in a packaged tarball**,
because the layout is not there. Keep such assertions out of the packaged test
set.
