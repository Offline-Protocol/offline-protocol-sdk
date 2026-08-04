# Documentation

## Getting Started

| Guide | Description |
|-------|-------------|
| [Quick Start](../QUICKSTART.md) | Get started in 5 minutes (React Native, iOS, Android) |
| [Upgrading](UPGRADING.md) | **Breaking changes and required app-side work for the storage-split release** |
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
| [Nostr Transport](nostr.md) | Nostr relay transport — censorship-resistant routing over WebSockets |

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
| [Security Policy](../SECURITY.md) | Vulnerability reporting and security design |

## Licensing

| Resource | Description |
|----------|-------------|
| [Licensing FAQ](licensing-faq.md) | The dual license in practice — app stores, the AGPL's reach, commercial licensing |
| [Export Control Notice](../EXPORT.md) | Encryption export status of the SDK and what app teams must handle themselves |
