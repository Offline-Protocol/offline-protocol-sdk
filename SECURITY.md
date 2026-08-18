# Security Policy

## Supported Versions

Security fixes land on the most recent minor release line only — there are no
backports to earlier lines. If you are on an older line, upgrading is the fix.

| Version                          | Supported          |
| -------------------------------- | ------------------ |
| Current line (`0.22.x`)          | :white_check_mark: |
| Any earlier line (`≤ 0.21.x`)    | :x:                |

Report against the latest published release
([releases](https://github.com/Offline-Protocol/offline-protocol-sdk/releases),
`npm view @offline-protocol/mesh-sdk version`); a fix ships in the next release
on the current line. One number covers every channel: the git tag, the
`@offline-protocol/mesh-sdk` npm package, and the `offline-protocol*` crates on
crates.io all share it. Releases up to and including `v0.20.1` predate crates.io
publishing and exist as tags and npm packages only.

## Reporting a Vulnerability

If you discover a security vulnerability in the Offline Protocol SDK, please report it responsibly.

**Do not open a public GitHub issue for security vulnerabilities.**

Instead, please email **gm@offlineprotocol.com** with the details. Include:

1. A description of the vulnerability
2. Steps to reproduce the issue
3. Potential impact assessment
4. Suggested fix (if any)

### What to expect

- **Acknowledgment** within 48 hours of your report
- **Status update** within 7 days with an assessment and remediation timeline
- **Credit** in the release notes (unless you prefer to remain anonymous)

## Scope

The following are in scope for security reports:

- MLS (RFC 9420) implementation flaws
- Sender-address derivation bypasses — a control frame accepted for an address
  its signing key does not derive to, or an MLS leaf accepted carrying an
  address other than the one its own signature key derives to
- Transport-layer message injection or spoofing
- Memory exhaustion via crafted payloads (DoS)
- Signature verification bypasses
- Information leakage through timing or error messages
- Unsafe code in FFI boundaries
- Dependency vulnerabilities in shipped code

### Out of Scope

- Denial of service requiring physical proximity (BLE range)
- Social engineering
- Issues in development dependencies only (not shipped)
- Theoretical attacks with no practical exploit path

## Safe Harbor

We want security researchers to look at this code, so we will not use the law as
a deterrent against people who do it in good faith.

If you conduct security research and disclose it in accordance with this policy,
Offline Protocol, Inc. considers that research to be:

- **Authorized** under the Computer Fraud and Abuse Act and equivalent
  anti-hacking statutes in other jurisdictions. We will not initiate or support
  civil or criminal action against you, and we will not report you to law
  enforcement, for accidental, good-faith violations of this policy.
- **Authorized** under the anti-circumvention provisions of the DMCA (17 U.S.C.
  §1201) and equivalent laws. We waive any claim against you for circumventing
  technical measures in our software in the course of your research, and we will
  not send DMCA takedown notices aimed at suppressing your findings.
- **Exempt** from any provision of our terms of service or acceptable-use terms
  that would otherwise prohibit security testing, waived on a limited basis for
  the research this policy covers.
- **Lawful and welcome.** If a third party brings action against you and you
  complied with this policy, we will make it known that your research was
  authorized.

Reverse engineering, decompiling, and disassembling the SDK and its shipped
binaries for the purpose of finding vulnerabilities is expressly permitted,
whatever license you received the SDK under.

### What we ask in return

Safe harbor applies while you:

- Test only against software, devices, and deployments **you own or have
  explicit permission to test**. Do not attack other people's peers, meshes, or
  self-hosted relays.
- Avoid privacy violations, data destruction, and service degradation. Access,
  modify, or retain only the data strictly needed to demonstrate the issue, stop
  as soon as the vulnerability is confirmed, and tell us if you encountered
  someone else's data.
- Keep testing against infrastructure we operate (our hosted relay endpoints)
  limited to your own accounts and traffic. No load, volumetric, or
  denial-of-service testing against shared infrastructure — describe the attack
  instead and we will assess it.
- Give us reasonable time to remediate before disclosing publicly. **90 days
  from our acknowledgment** is the default; tell us if you intend to publish
  sooner and we will work out a timeline rather than argue about one.
- Do not extort. A report conditioned on payment is not a good-faith
  disclosure. We credit researchers in release notes; we do not currently run a
  paid bounty program.

Two limits worth stating plainly. This safe harbor is ours to give and covers
only claims by Offline Protocol, Inc. — it cannot bind third parties, so an
application built on this SDK, an independently operated relay, or a platform
vendor may take its own view. And it is not a license to break the law: you are
still expected to comply with all applicable statutes.

If you are unsure whether a specific test is covered, email us **before** you
run it and ask. We would rather answer that question than litigate it.

## Security Design

The SDK enforces `#![deny(unsafe_code)]` in all core crates. Only the UniFFI FFI boundary (`offline-protocol-uniffi`) allows unsafe code, limited to generated scaffolding.

Cryptographic operations use the OpenMLS (RFC 9420) implementation with its `openmls_rust_crypto` provider. The default ciphersuite is `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`: X25519 key agreement, AES-128-GCM authenticated encryption, SHA-256 hashing, and Ed25519 signatures. The underlying primitives come from `ed25519-dalek`/`x25519-dalek` (dalek-cryptography) and the RustCrypto project's AES-GCM and SHA-2 crates.
