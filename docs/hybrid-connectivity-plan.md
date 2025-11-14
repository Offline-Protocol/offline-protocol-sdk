# Hybrid Messaging Upgrade Plan

Plan to evolve the existing fully offline messaging experience into a hybrid offline/online product while preserving offline resilience and introducing cloud connectivity where it adds value.

## 1. Goals & Success Criteria
- Preserve existing offline-first guarantees: messages always queue, nearby mesh delivery remains the default when offline.
- Add seamless cloud connectivity when the internet is available: real-time delivery, multi-device sync, cloud backup.
- Keep end-to-end encryption intact across offline and online transports.
- Provide observability into delivery status across both domains for Normal and Dev/Enthusiast modes.
- Design cloud components for horizontal scalability and regional distribution with no single point of failure.
- Achieve gradual rollout without disrupting current offline users.

## 2. Current State Recap
- Offline mesh transports (BLE/Wi-Fi Direct/etc.) support text, media, files, voice notes, polls.
- Local-first data store with offline search, queue management, delivery states, and DORS telemetry for Dev mode.
- Presence, notifications, and backups are scoped to offline-capable features; cloud sync not yet available.

## 3. Target Hybrid Experience
- **Instant connectivity switching:** App detects internet availability, chooses optimal transport (local mesh vs. cloud).
- **Unified conversation state:** Cloud-connected devices receive state updates even when the originating peer was offline.
- **Cross-device continuity:** Optional sign-in to sync history, preferences, and media across devices once online.
- **Presence & availability:** Online presence complements proximity indicators without exposing sensitive location details.
- **Transparent status:** Users see whether a message travelled via mesh, cloud, or both, with delivery receipts unified.

## 4. System Architecture Changes
### 4.1 Backend Services (New)
- **Gateway Service:** Acts as bridge between offline protocol network and cloud; ingests messages from devices when they choose cloud transport.
- **Message Router & Store:** Durable queue (e.g., PostgreSQL + Kafka/NATS) maintaining per-conversation ordering, TTL, retries.
- **Media Storage:** Encrypted object storage (S3-compatible) with presigned URLs; supports resumable uploads.
- **Presence Service:** Tracks online state, quiet hours, DND, using WebSocket/HTTP long polling.
- **Sync Service:** Provides delta sync APIs for conversations, contacts, settings, backups.
- **Auth Service:** Issues tokens for hybrid mode while respecting existing device identity and safety numbers.

### 4.2 Infrastructure Considerations
- Deploy on managed cloud (AWS/GCP/Azure) with regional redundancy and automated failover.
- Use TLS everywhere; consider mTLS between gateway and backend.
- Observability stack: structured logging, metrics (latency, delivery success), distributed tracing for message flow.
- Set up CI/CD pipelines with staged environments (dev, staging, canary, production).
- Adopt infrastructure-as-code (Terraform/Pulumi) to stand up repeatable multi-region stacks.
- Utilize service mesh or API gateway to provide consistent routing, auth, and rate limiting across clusters.

### 4.3 Protocol & Data Model Extensions
- Define cloud-friendly message envelope that preserves offline metadata (priority, hops, DORS path).
- Extend conversation schema with server-generated IDs, version vectors for conflict resolution.
- Add transport field to delivery receipts (mesh, cloud, mixed).
- Introduce sync tokens/checkpoints for incremental data fetch.

### 4.4 Distributed Topology & Federation
- Run gateway, router, presence, and sync services in active-active clusters across regions to minimize latency and provide resilience.
- Use partitioned event streams (Kafka/NATS JetStream/Pulsar) with cross-region replication for message durability.
- Select a consensus store (CockroachDB, YugabyteDB, or PostgreSQL with Patroni) to maintain strongly-consistent metadata (accounts, device registrations) while allowing geo-partitioning for conversations.
- Implement sharding strategy for conversations (hash-based or user affinity) and enable rebalancing with minimal disruption.
- Introduce federation layer to optionally bridge multiple operator domains; define trust boundaries, federation keys, and policy enforcement.
- Provide topology-aware routing so clients connect to nearest region but fail over automatically when unavailable.
- Capture cluster membership and health in a control plane (etcd/Consul) to drive service discovery and configuration.

## 5. Client Application Workstream
### 5.1 Connectivity & Transport Selection
- Implement connectivity service detecting internet availability, VPN/proxy constraints, captive portals.
- Define policy for switching: prefer mesh when peers nearby; fall back to cloud when peers unreachable; support concurrent delivery attempts.
- Surface user override in Dev mode for diagnostics.

### 5.2 Data Layer & Sync
- Augment local store with per-entity sync metadata (logical timestamps, tombstones).
- Implement background sync engine: pulls deltas, pushes pending items, reconciles conflicts.
- Use CRDTs or last-writer-wins with vector clocks for conversations, reactions, file metadata; ensure compatibility with server-side sharding.
- Ensure offline search indexes stay updated after sync merges.

### 5.3 Messaging Flow Updates
- On send: choose transport, create unified envelope, encrypt payload, queue to chosen channel(s).
- On receive (cloud): decrypt, apply to local store, ack to server.
- Handle retries and deduplication across transports via message IDs + hash.
- Update delivery status UI to combine mesh and cloud acknowledgements.

### 5.4 Presence & Notifications
- Integrate with presence service to publish/subscribe to status updates.
- Extend notification pipeline to use push notifications/APNs/FCM when online, fallback to local alerts offline.
- Respect user privacy controls and quiet hours; sync settings to cloud.

### 5.5 Media & File Handling
- Support resumable uploads/downloads to cloud storage while keeping peer-to-peer transfers for nearby sharing.
- Cache media locally with eviction policies; ensure encrypted at rest.
- Provide manual retry UI for failed cloud transfers.

### 5.6 Account & Identity
- Introduce optional account system (email/phone or passkey) without breaking anonymous offline mode.
- Bind device keys to account for multi-device sync; provide device management UI.
- Update onboarding to explain hybrid benefits and opt-in to cloud features.

### 5.7 Dev/Enthusiast Mode Enhancements
- Display transport selection rationale, cloud routing path, server latencies.
- Provide toggles for forcing mesh vs. cloud for troubleshooting.
- Surface sync checkpoints, pending cloud operations, and gateway metrics.

## 6. Security & Privacy
- Maintain end-to-end encryption; only encrypted payloads leave device.
- Use Double Ratchet/MLS for group conversations; ensure server stores ciphertext only.
- Implement forward secrecy and key rotation across transports.
- Protect metadata: minimize server knowledge (use opaque IDs, batching, padding where feasible).
- Comply with privacy regulations; update policy to cover cloud storage, backups.

## 7. Reliability, Monitoring & Tooling
- Define SLOs: message delivery latency (<2s online), data loss rate, availability.
- Instrument client SDK to emit telemetry (connectivity changes, sync durations) respecting user consent.
- Build automated chaos tests: simulate network flaps, conflicting edits, partial sync failures.
- Add runbooks for on-call: incident triage, message backlog clearing, transport outages.
- Add distributed playbooks: region failover, partition healing, cross-region replay/backfill, federation trust anchor rotation.
- Aggregate metrics centrally (Prometheus + Thanos/Mimir) and enable per-region dashboards with synthetic probes from edge locations.

## 8. Testing Strategy
- **Unit & Integration:** Cover sync engine, conflict resolution, transport chooser.
- **End-to-End:** Simulate offline→online transitions, multi-device sync, media uploads across regions and shards.
- **Security Testing:** Penetration tests on gateway, auth; verify no plaintext leakage.
- **Performance:** Load tests for gateway throughput, database scaling; measure battery impact on client. Exercise horizontal scaling scenarios and chaos drills for node loss.
- **Field Trials:** Controlled beta with instrumentation to capture edge cases before GA.
- **Disaster Recovery:** Regularly validate backup/restore, region evacuation, and federation key rotation drills.

## 9. Rollout & Migration Plan
1. **Prototype Phase:** Build cloud gateway, basic sync for text messages in internal builds.
2. **Feature Flagging:** Wrap hybrid features in remotely controllable flags; default off for public users.
3. **Distributed Alpha:** Deploy to a single region plus shadow region; validate replication, failover, and migration tooling with internal/enthusiast cohort.
4. **Media & Presence Expansion:** Add attachments, presence once core messaging stable; expand to additional regions gradually.
5. **Public Beta:** Offer opt-in to broader user base with in-app education, support resources; ramp per-region quotas.
6. **General Availability:** Enable for all users once KPIs met; keep offline-only mode accessible in settings; continuously monitor inter-region health.
7. **Post-GA Iteration:** Monitor, optimize transport selection heuristics, expand cloud integrations (backups, exports), and evaluate federation partnerships.

## 10. Open Questions & Follow-Ups
- Decide on account model: optional accounts vs. mandatory for cloud features, authentication method.
- Determine hosting strategy: self-managed vs. managed services, regional data residency needs, federation governance.
- Clarify legal/compliance requirements for storing encrypted data in cloud (GDPR, HIPAA where relevant).
- Finalize conflict resolution approach (CRDT choice, server arbitration policy).
- Evaluate third-party integrations (push providers, analytics) for privacy compatibility.
- Define cross-region consistency model tolerances (strong vs. eventual) for different data classes.
- Decide on federation interoperability requirements (other operators, public matrix-like federation, etc.).

## 11. Next Steps
- Run architecture review with backend, security, and client teams.
- Spike on gateway prototype using existing SDK networking primitives.
- Draft product brief for hybrid UX, including messaging, presence, backup flows, and distributed topology communication.
- Create detailed engineering tickets per workstream with effort estimates and dependencies.
- Produce capacity plan and cost model for multi-region deployment, including replication factors and data residency constraints.


