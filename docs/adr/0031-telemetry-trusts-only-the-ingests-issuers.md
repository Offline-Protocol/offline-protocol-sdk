# 0031. The telemetry uploader trusts only the ingest's issuers

**Status:** Accepted

## Context

[ADR 0024](0024-the-sdk-ships-its-own-tls-stack.md) put the telemetry
uploader's TLS stack inside the SDK and had it trust the Mozilla root set
compiled in through `webpki-roots`, never the device's trust store. That closes
interception by a certificate installed on the device. It leaves every other
root in the Mozilla set able to vouch for the ingest.

In October 2026 eight eMudhra intermediate certificates were found to have
weak keys. Root stores answered by removing emSign ECC Root CA - G3 and AAA
Certificate Services, neither of which the ingest uses. `webpki-roots` 1.0.9,
the bundle in every SDK from 0.26.0 to 0.29.0, did not carry AAA Certificate
Services but did carry emSign ECC Root CA - G3, so anyone holding one of those
keys and sitting on the network path could have presented a certificate for
the ingest and read the bearer key and the batches. The fix for a bundle is a new
bundle, which only reaches a device when the application ships an SDK update;
the next such incident would repeat the same exposure.

The ingest is `analytics.offlineprotocol.com`, served through Cloudflare.
Cloudflare issues its edge certificates from Let's Encrypt, Google Trust
Services or SSL.com, and keeps a backup certificate, from one of those or from
Sectigo, that it deploys on its own when the primary is revoked or its key is
compromised. Which of the four is serving on a given day is Cloudflare's
choice, not ours.

## Decision

The uploader trusts exactly the server roots of those four authorities,
fourteen certificates committed in
`crates/offline-protocol/src/telemetry/pipe/ingest-roots.pem`:

- Let's Encrypt: ISRG Root X1, ISRG Root X2.
- Google Trust Services: GTS Root R1, R2, R3, R4.
- SSL.com: SSL.com TLS RSA Root CA 2022, SSL.com TLS ECC Root CA 2022,
  SSL.com Root Certification Authority RSA, SSL.com Root Certification
  Authority ECC.
- Sectigo: Sectigo Public Server Authentication Root R46 and E46, USERTrust
  RSA Certification Authority, USERTrust ECC Certification Authority.

Each is byte-identical to the Mozilla root store entry and the Apple system
root of the same name, and a test pins the SHA-256 of every one, in order, so
the set cannot grow or change in a diff that does not also change the test. AAA Certificate Services is not
in it, although Sectigo owns it: it is one of the two roots this incident
removed, and Sectigo's current server roots cover the backup certificate
without it.

`ureq` is taken with `rustls-no-provider` instead of `rustls`, which drops
`webpki-roots` from the dependency graph entirely, and the uploader names the
ring crypto provider in its agent configuration rather than inheriting a
process default another library may have installed. The rustls and ring are
the ones the uploader already linked.

## Consequences

- **A compromise of any other authority does not reach the SDK.** Of the 121
  roots `webpki-roots` 1.0.9 carried, 14 remain able to vouch for the ingest,
  all belonging to authorities the edge actually issues from.
- **A rotation the edge makes on its own stays inside the set.** The set was
  chosen as everything Cloudflare can issue from, including the backup
  authority, not only the authority serving today. Pinning Let's Encrypt
  alone would have broken telemetry on every shipped SDK the first time
  Cloudflare renewed through Google Trust Services.
- **Moving the ingest outside these four authorities is an SDK release
  event.** That was already true of the Mozilla set (R14 in the threat
  model); it is now true of a smaller set. An SDK that does not trust the
  ingest's chain reports a network error in `telemetryStats().lastError` and
  backs off, which is loud and loses nothing beyond the queue's limits.
- **The size of the mobile libraries drops** by the bundle the uploader no
  longer links, and the `CDLA-Permissive-2.0` data license leaves the
  notices with it.
- **A build for another ingest** (a staging server, the live integration test)
  must sit behind one of the same four authorities, or carry its own roots
  file.

### What would undo this

The ingest moving off Cloudflare, or Cloudflare adding an authority. Either
way the roots file is the one place to change, the fingerprint test says so,
and the change ships in an SDK release before the edge moves, never after.
