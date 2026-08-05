# Export Control Notice

This distribution includes cryptographic software. The country in which you
currently reside may have restrictions on the import, possession, use, and/or
re-export to another country, of encryption software. Before using any
encryption software, please check your country's laws, regulations, and
policies concerning the import, possession, use, and re-export of encryption
software, to see if this is permitted.

## U.S. export status of this repository

The Offline Protocol SDK implements end-to-end encryption (MLS, RFC 9420) and
uses cryptographic primitives including ChaCha20-Poly1305, AES-GCM, HPKE, and
Ed25519. Encryption software of this kind falls under Export Control
Classification Number (ECCN) 5D002 of the U.S. Export Administration
Regulations (EAR, 15 CFR parts 730–774).

The source code of the SDK is publicly available without restriction, and
Offline Protocol, Inc. has notified both recipients named in
[15 CFR §742.15(b)](https://www.ecfr.gov/current/title-15/subtitle-B/chapter-VII/subchapter-C/part-742/section-742.15)
of its Internet location: the U.S. Bureau of Industry and Security (BIS), at
crypt@bis.doc.gov, and the ENC Encryption Request Coordinator at the National
Security Agency (NSA), at enc@nsa.gov.
Publicly available encryption source code for which that notification has been
made is not subject to the EAR under
[15 CFR §734.7(b)](https://www.ecfr.gov/current/title-15/subtitle-B/chapter-VII/subchapter-C/part-734/section-734.7),
and publicly available object code compiled from it — the npm packages, Python
wheels, and GitHub release artifacts built from this source and published
without charge — receives the same treatment.

## If you ship an application that embeds this SDK

The treatment above covers the SDK's public source code and the public builds
of it — not your application. You, not Offline Protocol, Inc., are the
exporter of your application, and an application embedding this SDK contains
encryption functionality. Expect at least the following, and confirm the
specifics with your own counsel:

- **Apple App Store.** App Store Connect asks export-compliance questions on
  every submission (the `ITSAppUsesNonExemptEncryption` Info.plist key). An
  app whose messaging is end-to-end encrypted by this SDK generally does not
  qualify for the "exempt" answers. Proprietary apps in this position
  typically self-classify as mass-market encryption under License Exception
  ENC
  ([15 CFR §740.17(b)(1)](https://www.ecfr.gov/current/title-15/subtitle-B/chapter-VII/subchapter-C/part-740/section-740.17))
  and file the associated annual self-classification report with BIS, due
  each February 1.
- **Google Play** asks for a comparable export-compliance declaration.
- **France** requires a declaration to ANSSI for supplying or importing means
  of cryptology.
- Other jurisdictions impose their own import, use, or supply restrictions on
  encryption software.

If your application is itself open source, the publicly-available treatment
described above may extend to it as well — in which case the §742.15(b)
notification duty for your own source location is yours, and runs to both
addresses above.

## Not legal advice

This notice reflects Offline Protocol, Inc.'s good-faith understanding of the
rules that apply to this repository and its published artifacts. It is
informational only, is not legal advice, and does not modify the SDK's
licenses (see [LICENSE](LICENSE) and
[LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md)). Questions:
legal@offlineprotocol.com.
