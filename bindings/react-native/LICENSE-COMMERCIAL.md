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
   obligations, subject to the applicable commercial agreement, including the
   telemetry term below.

You only need **one** of the two licenses, not both.

One distribution channel deserves a specific call-out: Apple's standard App
Store terms are widely regarded as incompatible with the AGPL-3.0's
prohibition on further restrictions, so the commercial license is the
supported option for apps distributed through the Apple App Store — the
reasoning is laid out in the
[Licensing FAQ](https://github.com/Offline-Protocol/offline-protocol-sdk/blob/main/docs/licensing-faq.md).

## Telemetry under the Commercial License

The SDK includes an optional telemetry client that, once an application
enables it with a telemetry API key and application identifier issued by
Offline Protocol, Inc., collects, batches, and uploads supported operational
events to Offline Protocol's hosted telemetry service. Service access, usage
allowances, and metered charges are set out in the applicable commercial
agreement. The following telemetry term applies to every commercial license,
as set out in that agreement.

A commercial licensee may leave hosted telemetry off, never supply telemetry
credentials, or switch it off using the SDK's documented controls. Until it is
explicitly enabled, the hosted telemetry client does not collect, buffer,
persist, or upload telemetry. The controls stop new collection; handling of
previously queued records, final flushes, and in-flight uploads is described
in the telemetry documentation.

Except as expressly authorized in writing by Offline Protocol, Inc., a
commercial licensee may not modify, configure, or build the SDK so that its
bundled telemetry client uploads to an unauthorized endpoint, redirects or
duplicates its uploads to another telemetry service, or alters the event
schema or serialized payload format it uploads. Security updates that preserve
the authorized destination and payload format are permitted. Network proxies
and gateways that forward uploads unaltered to an authorized endpoint, and
local test captures in builds that are not shipped, are permitted.

These telemetry restrictions do not limit the licensee's use of information
received through the SDK's public APIs. Receiving events and diagnostics,
storing or analyzing them, and forwarding them to the licensee's own systems
or third-party observability providers are ordinary uses of the SDK.
Independently implementing telemetry collection and export using those APIs is
also permitted.

This term applies to the Commercial License only. It does not modify or impose
additional restrictions on the AGPL-3.0 option, under which the telemetry
client may be modified, removed, or replaced in accordance with that license.
Under either option, the SDK performs no commercial-license entitlement
checks, and core SDK functionality does not require hosted telemetry.

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
