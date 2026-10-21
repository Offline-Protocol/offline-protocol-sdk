# 0031. The telemetry uploader trusts only the ingest's issuers

**Status:** Accepted

## Context

[ADR 0024](0024-the-sdk-ships-its-own-tls-stack.md) put the telemetry
uploader's TLS stack inside the SDK and had it trust the Mozilla root set
compiled in through `webpki-roots`, never the device's trust store. That closes
interception by a certificate installed on the device. It leaves every other
root in the Mozilla set able to vouch for the ingest.

In October 2026 eight eMudhra intermediate certificates were found to have
weak keys. They chain to two roots, neither of which the ingest uses: emSign
ECC Root CA - G3, which Mozilla and Chrome removed in answer, and Sectigo's AAA
Certificate Services through cross-signs. Mozilla and Chrome had already
removed AAA Certificate Services before the incident; Debian removed it in
answer, and Apple and Microsoft still trust it. `webpki-roots` 1.0.9, the
bundle in every SDK from 0.26.0 to 0.29.0, did not carry AAA Certificate
Services but did carry emSign ECC Root CA - G3, so anyone holding one of those
keys and sitting on the network path could have presented a certificate for
the ingest and read the bearer key and the batches. The fix for a bundle is a
new bundle, which only reaches a device when the application ships an SDK
update; the next such incident would repeat the same exposure.

The ingest is `analytics.offlineprotocol.com`, served through Cloudflare.
Cloudflare issues its edge certificates from Let's Encrypt, Google Trust
Services or SSL.com, and keeps a backup certificate, from one of those or from
Sectigo, that it deploys on its own when the primary is revoked or its key is
compromised. Which of the four is serving on a given day is Cloudflare's
choice, not ours.

## Decision

The uploader trusts exactly the server roots of those four authorities,
sixteen certificates committed in
`crates/offline-protocol/src/telemetry/pipe/ingest-roots.pem`:

- Let's Encrypt: ISRG Root X1, ISRG Root X2, ISRG Root YE, ISRG Root YR.
- Google Trust Services: GTS Root R1, R2, R3, R4.
- SSL.com: SSL.com TLS RSA Root CA 2022, SSL.com TLS ECC Root CA 2022,
  SSL.com Root Certification Authority RSA, SSL.com Root Certification
  Authority ECC.
- Sectigo: Sectigo Public Server Authentication Root R46 and E46, USERTrust
  RSA Certification Authority, USERTrust ECC Certification Authority.

Not every one comes from the same place, and the roots file says which:

- Thirteen are byte-identical to the Mozilla root store entry (as carried by
  `webpki-roots` 1.0.9) and the Apple system root of the same name.
- GTS Root R2 is byte-identical to the Apple system root and is one of the
  four server roots Google Trust Services publishes, but the Mozilla set does
  not carry it.
- ISRG Root YE and ISRG Root YR are Let's Encrypt's Generation Y roots, taken
  self-signed from letsencrypt.org; no root program carries them yet. Let's
  Encrypt has issued from them (intermediates YE1 and YR1) since 2026, and the
  ingest's chain runs through Root YE today. It validates against X1 and X2
  only because the edge also sends the cross-signs from Root YE to X2 and from
  X2 to X1, which expire in 2032. Pinning YE and YR means a chain that stops
  at them, which Let's Encrypt also publishes, still validates.

A test pins the SHA-256 of every root, in order, so the set cannot grow or
change in a diff that does not also change the test, and a second test
verifies the ingest's captured chain against the set both as served and cut
at Root YE. AAA Certificate Services is not in the set, although Sectigo owns
it: it is the other root the compromised intermediates chain to, and Sectigo's
current server roots cover the backup certificate without it.

`ureq` is taken with `rustls-no-provider` instead of `rustls`, which drops
`webpki-roots` from the dependency graph entirely, and the uploader names the
ring crypto provider in its agent configuration rather than inheriting a
process default another library may have installed. The rustls and ring are
the ones the uploader already linked.

`rustls-webpki` 0.103.16 rejects any path through the compromised eMudhra
intermediates' keys, whatever root it ends at. It closes the AAA Certificate
Services cross-sign path independently of the root set, and it is the only
part of this fix that also protects a host whose own TLS stack still trusts
those roots.

## Consequences

- **A compromise of any other authority does not reach the SDK.** Of the 121
  roots `webpki-roots` 1.0.9 carried, 13 remain able to vouch for the ingest,
  joined by GTS Root R2 and the two Let's Encrypt Generation Y roots, all
  belonging to authorities the edge actually issues from.
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

The ingest moving off Cloudflare, Cloudflare adding an authority, or one of
the four moving its issuance to a root outside the file, as Let's Encrypt did
with Generation Y. The likeliest is Cloudflare's own: in September 2026 it
announced that it will become a public certificate authority, and a Universal
SSL certificate issued from its own root would not validate against this set.
Whichever it is, the roots file is the one place to change, the fingerprint
test says so, and the change ships in an SDK release before the edge moves,
never after. The ignored live test,
`cargo test -p offline-protocol --lib -- --ignored the_ingest_chain`, is the
check to run before and after any of these.
