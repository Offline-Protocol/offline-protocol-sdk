# Commercial License

Copyright © 2025-2026 Offline Protocol, Inc.

The Offline Protocol SDK is **dual-licensed**. You may use it under **either** of the
following licenses, at your option:

1. **GNU Affero General Public License v3.0 (AGPL-3.0-only)** — the full text is in
   [`LICENSE`](LICENSE). This option is free of charge but carries strong copyleft
   obligations: software that incorporates this SDK is generally a covered work
   under the AGPL-3.0 (per section 5) and must be distributed under the same
   license with corresponding source made available to recipients (per section 6).
   If you operate a modified version that interacts with users over a network, you
   must additionally offer those users the corresponding source — this is the
   network-use clause specific to AGPL (section 13).

2. **Commercial License** — for organizations that cannot or do not wish to comply
   with the AGPL-3.0 (for example, shipping the SDK inside a proprietary mobile
   application, embedding it in closed-source firmware, or operating a SaaS without
   releasing source). A separate commercial license from **Offline Protocol, Inc.**
   grants the right to use, modify, and distribute the SDK without the AGPL-3.0
   obligations.

You only need **one** of the two licenses, not both.

One distribution channel deserves a specific call-out: Apple's standard App
Store terms are widely regarded as incompatible with the AGPL-3.0's
prohibition on further restrictions, so the commercial license is the
supported option for apps distributed through the Apple App Store — the
reasoning is laid out in the
[Licensing FAQ](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/licensing-faq.md).

## Telemetry under the Commercial License

The SDK includes a telemetry pipe that, once an application enables it with a
key issued by Offline Protocol, Inc., uploads accepted events to the Offline
Protocol telemetry service. The service is a hosted, metered product included
in commercial plans, and every commercial license carries the following term.

A commercial licensee **may** leave telemetry off, switch it off at runtime,
and never supply a key; the SDK collects and sends nothing until a key is
supplied. A commercial licensee **may not** modify the SDK so as to change the
service endpoint the pipe uploads to, alter the wire format of what it
uploads, or redirect or duplicate that upload to a service other than Offline
Protocol's. Receiving the SDK's events through its event API and forwarding
them to systems of your own is ordinary use of the SDK and is not restricted.

This term applies to the commercial license only. Under the AGPL-3.0 option
the pipe may be modified, removed or replaced like any other part of the SDK,
and nothing in the SDK verifies a license or a key at runtime under either
option.

## Obtaining a Commercial License

Commercial licenses are offered by **Offline Protocol, Inc.** To request a quote
or discuss commercial terms, contact:

- **Email:** legal@offlineprotocol.com

Please include a brief description of your intended use (product, distribution
model, expected scale) so we can scope the license appropriately.

## Contributions

Contributors grant **Offline Protocol, Inc.** the right to sublicense their
contributions under this Commercial License alongside the AGPL-3.0. The full terms are in
[`CLA.md`](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/CLA.md);
see [`CONTRIBUTING.md`](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/CONTRIBUTING.md)
for the signing flow.
