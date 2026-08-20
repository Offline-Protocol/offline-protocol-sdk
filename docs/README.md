# Documentation

Two kinds of document live here, and they answer different questions.

**Guides** answer "how do I use this". Start with the Quick Start and the
integration guide for your platform.

**Reference** answers "what is the contract" and "why is it like this". Read it
before changing behaviour, not before using the SDK.

## Getting Started

| Guide | Description |
|-------|-------------|
| [Quick Start](../QUICKSTART.md) | Get started in 5 minutes (React Native, iOS, Android) |
| [Upgrading](UPGRADING.md) | **Breaking changes and required app-side work** |
| [React Native Integration](react-native-integration.md) | Full SDK integration guide with complete API reference |
| [iOS Integration](ios-integration.md) | Native iOS (Swift) setup and usage |
| [Android Integration](android-integration.md) | Native Android (Kotlin) setup and usage |

## Core Concepts

| Guide | Description |
|-------|-------------|
| [Architecture](architecture.md) | Design philosophy, crate organization, and system overview |
| [API Reference](api-reference.md) | Core types, configuration structs, main API, and events |
| [Configuration](configuration.md) | All configuration parameters with use-case examples |

## Features

| Guide | Description |
|-------|-------------|
| [DORS Deep Dive](dors.md) | How the transport selection engine works (scoring, switching, escalation) |
| [DORS Configuration](dors-configuration.md) | Tuning DORS for different use cases with parameter reference |
| [Message Delivery](message-delivery.md) | Delivery lifecycle, retry/ACK system, flush triggers, and client-side persistence |
| [Mesh Networking](mesh.md) | Peer discovery, connection management, message delivery, and routing |
| [MLS Encryption](mls-integration.md) | End-to-end encryption with auto-encryption and manual MLS APIs |
| [Service Discovery](service-discovery.md) | Decentralized service registration, discovery, and request/response |
| [Telemetry](telemetry.md) | Wire up a telemetry sink for metrics, routing decisions, and MLS lifecycle |
| [Transport Architecture](transport-architecture.md) | Transport abstraction layer and how to add new transports |
| [Reticulum Transport](reticulum.md) | Reticulum mesh transport setup, architecture, and platform integration |
| [Nostr Transport](nostr.md) | Nostr relay transport, censorship-resistant routing over WebSockets |

## Protocol specification

The wire and behaviour contract, independent of this implementation. A second
implementation written against these documents should interoperate.

| Document | Scope |
|----------|-------|
| [Specification index](spec/README.md) | Layering, conformance language, the two overriding invariants |
| [Identity and addressing](spec/identity.md) | Address derivation, canonical form, session and group identifiers |
| [Message model and wire format](spec/wire-format.md) | The abstract message, the JSON floor, binary v1, the extension TLV registry |
| [Control messages](spec/control-messages.md) | Reserved prefix registry, control-plane signing, the two exemption classes |
| [Document replication](spec/data-sync.md) | Sync frames, anti-entropy, attachment references, blobs over the media path |
| [Encryption envelopes](spec/encryption-envelopes.md) | MLS envelope forms, media chunk envelope, sealed rich payload |
| [Group protocol](spec/group-protocol.md) | Group frames, membership commits, leaf identity binding, relay broadcast |
| [Capability negotiation](spec/capability-negotiation.md) | What peers advertise, what it gates, what absence means |

## Security

| Document | Scope |
|----------|-------|
| [Threat model and trust boundaries](security/threat-model.md) | Assets, adversary classes, controls, and the residual risks stated plainly |
| [Security Policy](../SECURITY.md) | Vulnerability reporting and safe harbor |

## State machines

| Document | Governs |
|----------|---------|
| [Overview](state-machines/README.md) | The invariant that spans all five |
| [Delivery and acknowledgements](state-machines/delivery-and-acks.md) | What happens to an inbound frame, and when a receiver acknowledges |
| [Outbox and retries](state-machines/outbox-and-retries.md) | An outbound message from send to terminal state |
| [Session lifecycle](state-machines/session-lifecycle.md) | 1:1 MLS establishment, confirmation, desync, and heal |
| [Group message lifecycle](state-machines/group-message-lifecycle.md) | A group message through fan-out, buffering, and drain |
| [Transport lifecycle](state-machines/transport-lifecycle.md) | Transport availability, scoring, switching, escalation |

## Decisions

| Document | Scope |
|----------|-------|
| [ADR index](adr/README.md) | Fifteen decisions that are expensive to reverse or easy to undo by accident |

If something in the codebase looks redundant or over-engineered, check here
before simplifying it.

## Bridge contracts

What each language binding owes the core, and what the core owes it. Every rule
in here fails **silently** when violated.

| Document | Scope |
|----------|-------|
| [Shared contract](bridges/README.md) | The ten rules every binding shares |
| [Swift](bridges/swift.md) | iOS native and the React Native iOS bridge |
| [Kotlin](bridges/kotlin.md) | Android native and the React Native Android bridge |
| [Python](bridges/python.md) | Desktop and tooling |
| [TypeScript](bridges/typescript.md) | The React Native JavaScript surface |

## Release history

| Resource | Description |
|----------|-------------|
| [CHANGELOG](../CHANGELOG.md) | Unreleased changes and the current release |
| [Changelog archive](changelog/README.md) | Older releases, one file per minor series |

## Examples

| Resource | Description |
|----------|-------------|
| [React Native Example App](../examples/react-native-app/README.md) | Complete messaging app demonstrating all SDK features |
| [Example App Setup](../examples/react-native-app/SETUP.md) | First-time setup for the example app |
| [Example App Integration Guide](../examples/react-native-app/INTEGRATION_GUIDE.md) | Step-by-step project integration walkthrough |

## Contributing

| Resource | Description |
|----------|-------------|
| [Contributing Guide](../CONTRIBUTING.md) | Development setup, code quality standards, and PR process |

## Licensing

| Resource | Description |
|----------|-------------|
| [Licensing FAQ](licensing-faq.md) | The dual license in practice: app stores, the AGPL's reach, commercial licensing |
| [Export Control Notice](../EXPORT.md) | Encryption export status of the SDK and what app teams must handle themselves |
