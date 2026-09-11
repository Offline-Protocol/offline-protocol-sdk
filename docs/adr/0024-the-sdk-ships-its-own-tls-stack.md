# 0024. The SDK ships its own TLS stack for telemetry

**Status:** Accepted

## Context

The telemetry pipe (0.26.0) uploads batches to one HTTPS endpoint. That is the
only network egress, the only background thread and the only TLS stack in the
workspace, and every consumer of the mobile bindings links it whether or not
their application ever calls `enableTelemetry`: the bindings ship one FFI
artifact, and a feature-gated UDL method would be a runtime checksum mismatch
rather than a build error.

The plan for the pipe set a tripwire: **more than 2 MB per architecture and the
host-HTTP design gets reconsidered.** The measurement, release profile, `main`
at 76b2e056 against the pipe branch:

| Target | Before | After | Delta |
|---|---|---|---|
| iOS arm64 dylib | 8,693,992 | 10,651,480 | +1,957,488 |
| iOS arm64 dylib, minisize | 4,465,760 | 5,707,976 | +1,242,216 |
| Android arm64 `.so` | 9,177,936 | 11,203,328 | +2,025,392 |

Android arm64 landed on the line: 2,025,392 bytes, 1.93 MiB, 2.03 MB. Which
side of the tripwire that falls on depends on which megabyte you mean, so it
is recorded here rather than settled by arithmetic.

Almost none of it is the pipe. The classifier, the ring, the rollup, the store
and the uploader are tens of kilobytes between them. The rest is rustls, ring
and the compiled-in Mozilla root set.

## Decision

Keep the TLS stack in the SDK. Do not move the upload to a host-supplied HTTP
callback.

`ureq` is taken with exactly its `rustls` feature: rustls, ring and the bundled
roots, with no gzip, no SOCKS proxy support and no charset handling. That
feature hard-requires rustls's `tls12` and `logging`, so there is no cheaper
trim short of forking the feature set.

HTTP CONNECT proxy support is part of ureq itself rather than a feature, and
the uploader keeps it. The telemetry uploader can use an HTTP CONNECT proxy
selected through the HTTP client's supported environment configuration, read
when telemetry is enabled. TLS remains between the SDK and the destination
server and is validated against the bundled Mozilla root certificates.
Installing an interception certificate only in the device trust store does not
make that certificate trusted by the SDK. Such an interception attempt fails
certificate validation; queued telemetry remains subject to retry, capacity,
and expiry limits.

The agent follows no redirect, and every setting these properties rest on (the
proxy source, the redirect limit, the root store and verification) is named in
the uploader's agent configuration rather than inherited from ureq's defaults,
with tests pinning them, so a ureq upgrade cannot change them silently.

## Consequences

The alternative was a host callback: the pipe hands a binding the bytes and a
URL, and the binding posts them with `URLSession` or `OkHttp`, both already
linked. That is roughly two megabytes cheaper per architecture, and it is the
design this rejects, for three reasons.

- **It is a hidden hook.** A callback the application implements is a callback
  the application can read, rewrite, redirect or drop. The endpoint being a
  build-time constant with an `https://` assertion in `build.rs` is what makes
  "the SDK sends telemetry to exactly one place, and you can read the code that
  proves it" a checkable claim rather than a promise about a callback nobody
  audits. [ADR 0014](0014-dedicated-ffi-entry-points.md) made the same call for
  relay answers.
- **The device trust store is deliberately not consulted.** The compiled-in
  Mozilla roots are why an interception certificate installed on a device does
  not let the stream be redirected or read: an interception certificate trusted
  only by the device is not trusted by the SDK, and a CONNECT proxy tunnels the
  encrypted connection without exposing its payload. A host HTTP client uses
  the device store by construction, so the callback design does not merely cost
  the property, it cannot express it. This is the anchor for the network-egress
  section of the threat model.
- **One implementation, four bindings.** A callback puts the retry schedule,
  the idempotency key, the status matrix and the `Retry-After` handling into
  Swift, Kotlin, TypeScript and Python, four times, each with its own bugs.
  That is precisely the shape the pipe exists to delete: the frozen TypeScript
  client it replaces was one binding's worth of exactly this.

The cost is real and falls on every adopter. Applications that want none of it
opt out at the Rust level with `default-features = false` on
`offline-protocol`, which drops the `telemetry-pipe` feature and with it the
socket, the thread and the TLS stack. `data` is a default feature too, so an
adopter who wants replicated documents asks for it back:

```toml
offline-protocol = { version = "0.26", default-features = false, features = ["data"] }
```

That escape hatch does not exist for a consumer of the prebuilt mobile
artifacts, who pays the bytes regardless.

### What would undo this

A measured demand from adopters that the mobile binary shrink, weighed against
losing the bundled roots. If that arrives, the answer is a second prebuilt
artifact without the `telemetry-pipe` feature rather than a host callback: it
keeps the egress property intact for everyone who does enable telemetry, and
gives the size back to everyone who does not. Splitting the artifact means a
second UDL surface and a second checksum, which is the work that decision buys.
