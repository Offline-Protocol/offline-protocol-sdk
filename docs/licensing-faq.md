# Licensing FAQ

How the SDK's dual license plays out in practice, especially for mobile apps
and app-store distribution. This page is informational — it is not legal
advice, and nothing here modifies either license. The licenses themselves are
[LICENSE](../LICENSE) (AGPL-3.0-only) and
[LICENSE-COMMERCIAL.md](../LICENSE-COMMERCIAL.md); the trademark policy is
[TRADEMARKS.md](../TRADEMARKS.md), and the SDK's export-control status is
covered in [EXPORT.md](../EXPORT.md).

## Which license am I using?

Either one, at your option — AGPL-3.0-only free of charge, or a commercial
license from Offline Protocol, Inc. You need one of the two, never both. If
you have not obtained a commercial license, the AGPL is the only permission
you have.

## My app links the SDK. Does the AGPL cover my whole app?

Under the AGPL option, yes. An application that incorporates the SDK is a
covered work (AGPL-3.0 §5), so distributing the app means distributing the
whole of it under the AGPL-3.0 and offering every recipient its corresponding
source (§6). If that does not fit your product, that is exactly what the
[commercial license](../LICENSE-COMMERCIAL.md) is for.

## Does the network clause (§13) matter for a mesh app?

§13 adds an obligation the plain GPL does not have: if you **modify** the SDK
and users interact with your modified version "remotely through a computer
network", you must offer those users the corresponding source too — even if
you never ship them a binary. In a peer-to-peer mesh messaging app, the
remote peers your modified app exchanges messages with are plausibly such
users. If you modify the SDK under the AGPL option, plan on offering source
to everyone your app talks to, not only to the people you distribute the app
to.

## Can I ship an AGPL-licensed app on the Apple App Store?

We do not consider that a supported combination, for two independent reasons —
neither of which an app developer can cure on their own:

1. **Apple's terms add restrictions the AGPL forbids.** Apps distributed
   through the App Store are subject to Apple's standard license terms and
   usage rules. The Free Software Foundation's position — established in the
   GNU Go and VLC App Store takedowns — is that those terms impose "further
   restrictions" that GPL-family licenses prohibit (AGPL-3.0 §10), making App
   Store distribution of a covered work itself a license violation.
2. **iOS code signing collides with §6.** For software conveyed in or for a
   consumer device, AGPL-3.0 §6 requires "Installation Information" —
   whatever a user needs to install and run a modified version on their own
   device. App Store distribution cannot provide that: users cannot install
   modified builds of your app on their iPhones.

The supported path for App Store distribution is the
[commercial license](../LICENSE-COMMERCIAL.md), which carries neither
obligation.

## Is there an "App Store exception"?

Not at present: Offline Protocol, Inc. has not granted any additional
permission under AGPL-3.0 §7 for app-store distribution. Because contributors
license their work through the [CLA](../CLA.md), Offline Protocol, Inc. does
hold the rights needed to grant one in the future. If an app-store exception
would matter to your open-source project, contact legal@offlineprotocol.com.

## What about Google Play?

Google Play's terms have not drawn the same incompatibility objections, and
GPL-family apps are distributed there. But everything above about combined
works still applies in full: a Play app using the SDK under the AGPL option
must itself be AGPL-3.0, with source offered to every recipient. A
proprietary Play app needs the commercial license, the same as on iOS.

## Does either license handle export compliance for me?

No. The licenses govern copyright; export control is separate law. The SDK's
own status is described in [EXPORT.md](../EXPORT.md); the export-compliance
declarations an app store asks for when you submit your app are yours to
make.

## How do I get a commercial license?

See [LICENSE-COMMERCIAL.md](../LICENSE-COMMERCIAL.md) — in short, email
legal@offlineprotocol.com with a brief description of your product, its
distribution model, and expected scale.
