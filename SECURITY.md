# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.13.x  | :white_check_mark: |
| < 0.13  | :x:                |

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
- TOFU key management bypasses
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

## Security Design

The SDK enforces `#![deny(unsafe_code)]` in all core crates. Only the UniFFI FFI boundary (`offline-protocol-uniffi`) allows unsafe code, limited to generated scaffolding.

Cryptographic operations use audited RustCrypto primitives (Ed25519-dalek, SHA-256, AES-256-GCM) via the OpenMLS (RFC 9420) implementation.
