import Foundation

/// Holds inbound message events that JavaScript could not take, so a message
/// the core has already acknowledged survives a window where nothing was
/// listening.
///
/// By the time `message_received`, `file_received` or the
/// `message_decryption_failed` that stands in for one is emitted, the core has
/// sent the delivery ACK, dedup-marked the id and dropped its queued copy: the
/// sender will not resend, and nothing in the SDK will restate the event. A
/// drop between the core and the app is therefore a lost message — and the
/// drop is ordinary. The React instance is down while the app is backgrounded,
/// or a push injection delivers the ciphertext before JS has subscribed. The
/// one-shot events on this bridge are re-derived from a latch on foreground
/// (`restateInternetSupersededIfNeeded`); an inbound message has no latch to
/// re-derive from, so it has to be held.
///
/// Unlike the one-shot set these do not collapse per type — every message is
/// its own fact — so entries are keyed `type:message_id`, kept FIFO, and the
/// cap is a real capacity (256, oldest dropped past it) rather than a backstop.
/// Re-holding an existing key replaces it in place, which is what makes a
/// flush that fails and re-holds idempotent.
///
/// **Every entry is stamped with the session generation it was emitted for.**
/// `bumpGeneration()` runs on `create()` and `destroy()`; `hold` and `restore`
/// refuse entries from any other generation, so a message held for the
/// account the app just tore down cannot surface in whatever it constructs
/// next, and a flush already carrying entries across a teardown puts nothing
/// back. Mirrors the Android `StickyEventBuffer` session stamp.
///
/// Lock-protected because writers and readers are different threads: holds
/// arrive from the core's event thread, flushes from the main queue. Values are
/// the event JSON, not a React payload, so the class is Foundation-only and the
/// SwiftPM harness covers it.
final class InboundEventBuffer {

    /// A held event: its `type:message_id` key, the JSON to re-emit, and the
    /// session generation it was emitted for.
    struct Entry: Equatable {
        let key: String
        let eventJson: String
        let generation: Int
    }

    /// The event tags this buffer holds. Must match
    /// `BUFFERED_INBOUND_EVENT_TYPES` in `src/constants.ts` and
    /// `OfflineProtocolModule.BUFFERED_INBOUND_EVENT_TYPES` on Android; pinned
    /// by `react_native_buffered_inbound_event_set_matches_native`.
    static let bufferedEventTypes: Set<String> = [
        "message_received",
        "file_received",
        "message_decryption_failed",
    ]

    /// Most events held; the oldest is dropped past it.
    static let defaultMaxEntries = 256

    private let lock = NSLock()
    private var held: [Entry] = []
    private var generation = 0
    private var sequence = 0
    private let maxEntries: Int

    init(maxEntries: Int = InboundEventBuffer.defaultMaxEntries) {
        self.maxEntries = maxEntries
    }

    /// The session events are currently being held for. Read before the emit
    /// is attempted and passed back to `hold`, so an emit that fails against a
    /// session torn down while it was in flight leaves nothing behind.
    func currentGeneration() -> Int {
        lock.lock(); defer { lock.unlock() }
        return generation
    }

    /// The `type:message_id` key for an event, or nil when the event is not of
    /// a buffered type. An event of a buffered type that carries no id at all
    /// is keyed by arrival order rather than dropped — losing a message to a
    /// missing field would be the failure this buffer exists to prevent.
    func key(forEventJson eventJson: String) -> String? {
        guard let data = eventJson.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let type = json["type"] as? String,
              InboundEventBuffer.bufferedEventTypes.contains(type) else {
            return nil
        }
        if let id = json["message_id"] as? String, !id.isEmpty {
            return "\(type):\(id)"
        }
        if let id = json["file_id"] as? String, !id.isEmpty {
            return "\(type):\(id)"
        }
        lock.lock(); defer { lock.unlock() }
        sequence += 1
        return "\(type):seq-\(sequence)"
    }

    /// Holds `eventJson` under `key`, replacing an entry already held under
    /// it. Returns false — and holds nothing — when `generation` is no longer
    /// current. Past the cap the oldest entry is evicted.
    @discardableResult
    func hold(key: String, eventJson: String, generation: Int) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard generation == self.generation else { return false }
        if let index = held.firstIndex(where: { $0.key == key }) {
            held[index] = Entry(key: key, eventJson: eventJson, generation: generation)
        } else {
            held.append(Entry(key: key, eventJson: eventJson, generation: generation))
            trimToCapLocked()
        }
        return true
    }

    /// Removes and returns everything held, oldest first. The caller emits and
    /// hands back whatever JS refused through `restore`.
    func drain() -> [Entry] {
        lock.lock(); defer { lock.unlock() }
        let drained = held
        held.removeAll()
        return drained
    }

    /// Puts back entries a flush could not deliver, at the head so arrival
    /// order holds, skipping any from a superseded generation or whose key has
    /// been held again in the meantime.
    func restore(_ entries: [Entry]) {
        guard !entries.isEmpty else { return }
        lock.lock(); defer { lock.unlock() }
        let restorable = entries.filter { entry in
            entry.generation == generation && !held.contains(where: { $0.key == entry.key })
        }
        guard !restorable.isEmpty else { return }
        held = restorable + held
        trimToCapLocked()
    }

    /// Starts a new session: refuses everything in flight for the old one and
    /// discards what it held. Called on `create()` and `destroy()`.
    func bumpGeneration() {
        lock.lock(); defer { lock.unlock() }
        generation += 1
        held.removeAll()
    }

    var isEmpty: Bool {
        lock.lock(); defer { lock.unlock() }
        return held.isEmpty
    }

    var count: Int {
        lock.lock(); defer { lock.unlock() }
        return held.count
    }

    /// Caller must hold `lock`.
    private func trimToCapLocked() {
        if held.count > maxEntries {
            held.removeFirst(held.count - maxEntries)
        }
    }
}
