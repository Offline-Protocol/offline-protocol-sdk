# Java binding contract

Covers `bindings/java`: a JNA wrapper over the C API for the JVM on desktop
and server.

Read [the shared contract](README.md) and [the C binding contract](c.md)
first. This document covers only what the wrapper adds or changes.

## What it is

**The Java binding is the C binding with Java types.** It adds no behaviour of
its own: `OfflineProtocol.call` is `offline_protocol_call`, and the method
names, JSON encodings and error positions are the local API's, as in
[the C contract](c.md#what-it-is). It is not generated from the UniFFI
definition, and it is not the Kotlin binding: the Kotlin bindings are the
Android library, built on the UniFFI ABI and shipped as an AAR. A JVM
application that is not on Android uses this one.

JNA was chosen over the Java 22 foreign function API because the JVMs hosts
actually run are 11 and 17, and the C binding has a dozen functions, so there
is nothing for a generator to earn.

## Which shared rules apply

| Rule | How it reaches the Java binding |
|---|---|
| [C1](README.md#c1-regenerate-every-binding-together) | Does not apply: nothing here is generated from the definition |
| [C2](README.md#c2-the-error-enum-is-append-only) | `OfflineProtocolException.code()` is the variant's position, and `StorageException.Kind` is the storage status. Both are append-only, and `HeaderDriftTest` fails if a value moves |
| [C3](README.md#c3-events-cross-as-opaque-json) | Events reach `EventListener` and `pollEvent` as the engine's JSON, unchanged |
| [C9](README.md#c9-bridge-behaviour-is-not-covered-by-the-rust-test-suite) | `cargo test` proves nothing about this binding. `gradle build` in `bindings/java` runs `ConsumerCheckTest` against the real library, and CI runs it on Linux and macOS |
| [C11](README.md#c11-a-storage-adapter-is-a-supported-extension-point-and-is-verified) | A `StorageProvider` is an adapter like any other: `runStorageConformance` is the bar |
| [C12](README.md#c12-telemetry-is-callback-free-and-the-binding-fills-the-platform) | The same as the C binding's [K3](c.md#k3-the-binding-fills-the-telemetry-platform): the host supplies `os_major` and the lifecycle |

## J1. The declarations are the header, and a test says so

`NativeApi` redeclares every function and structure of
`bindings/c/include/offline_protocol.h` by hand. A function added to the header
and not declared there is unreachable from Java and fails no build, and a
structure whose fields drift reads or writes the wrong memory.
`HeaderDriftTest` reads the committed header and fails if the two lists of
functions differ, or if a `Status`, `Transport` or storage `Kind` value is not
the header's number. A change to the header's structures is not caught that
way: read `NativeApi` when you change them.

`size_t` is declared as `long`, which is correct only on a 64-bit JVM. The
loader refuses any other, because the alternative is corrupted memory.

## J2. A registration is released by `drop`, and only by `drop`

Every listener and storage provider reaches the library through static JNA
callback objects and an id carried in `ctx`. The callback objects are static
because JNA releases a callback that is no longer referenced, and native code
may call it for as long as it holds the registration. The library calls
`drop(ctx)` exactly once when it lets go ([K2](c.md#k2-callbacks-run-on-the-engines-thread-inside-the-engine)),
a refused registration included, and `Callbacks.DROP` is the only code that
removes the entry. Removing it earlier, say when `setEventListener` is called
again, would race a callback still running on another thread. The consumer
check pins this: a replaced listener, a freed engine and a storage pair each
leave the registration count where it was.

Buffers a provider hands back are native memory held until the library returns
them through `release`.

## J3. Listeners and providers do not unwind

An exception out of a listener is swallowed, and one out of a storage method is
turned into a status. Unwinding through a C frame into Rust is undefined
behaviour ([K2](c.md#k2-callbacks-run-on-the-engines-thread-inside-the-engine)).
A storage method that throws anything but `StorageException` is reported as the
operation's own failure, never as `CORRUPTED_DATA`: the engine answers
corruption on the protocol-state record key by minting a new key, which strands
every record sealed under the old one ([K6](c.md#k6-storage-providers)).

## J4. `close()` waits for calls, and a listener cannot close the engine

`OfflineProtocol` takes the read side of a lock for each call and the write
side to free the engine, which is the C contract's "no call in flight when it is
freed". The consequence is that `close()` called from inside a listener, which
runs inside a call, waits for itself. A listener copies the event and returns;
that rule already forbids it.

## What the host owes

What [the C binding](c.md#what-the-host-owes) says: the transports, storage,
the lifecycle, and one engine freed once. This binding adds the native library,
which it does not bundle, and the JVM flag that finds it.
