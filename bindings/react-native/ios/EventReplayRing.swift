import Foundation

/// The last `capacity` events handed to JavaScript, by the number each was
/// handed over with, so the TypeScript layer can fetch one that never arrived
/// (#514). The same contract as the Android module's `EventReplayRing`.
///
/// Numbering alone only reports a loss. An event such as `message_received`
/// or `message_delivered` reports something the core has already settled for
/// good, so nothing will restate it, and a lost one is a lost message unless a
/// copy is kept where JavaScript can ask for it.
///
/// Only events that were handed over are recorded, under the lock that gives
/// out their numbers, so the ring holds a contiguous run. Past the cap the
/// oldest goes, and a gap that reaches back further is reported rather than
/// recovered. Values are event JSON, so the class is Foundation-only and the
/// SwiftPM harness covers it.
final class EventReplayRing {

    struct Entry: Equatable {
        let seq: Int
        let eventJson: String
    }

    static let defaultCapacity = 256

    private let lock = NSLock()
    private var entries: [Entry] = []
    private let capacity: Int

    init(capacity: Int = EventReplayRing.defaultCapacity) {
        precondition(capacity > 0, "capacity must be positive")
        self.capacity = capacity
    }

    /// Keeps `eventJson` as the event handed over with `seq`.
    func record(seq: Int, eventJson: String) {
        lock.lock()
        defer { lock.unlock() }
        entries.append(Entry(seq: seq, eventJson: eventJson))
        if entries.count > capacity {
            entries.removeFirst(entries.count - capacity)
        }
    }

    /// Every kept event numbered `fromSeq` to `toSeq` inclusive, in order. A
    /// number older than the ring is absent; the caller compares what came
    /// back with what it asked for.
    func replay(fromSeq: Int, toSeq: Int) -> [Entry] {
        guard toSeq >= fromSeq else { return [] }
        lock.lock()
        defer { lock.unlock() }
        return entries.filter { $0.seq >= fromSeq && $0.seq <= toSeq }
    }

    var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return entries.count
    }

    /// Forgets everything, for a host that starts numbering from zero again.
    func clear() {
        lock.lock()
        defer { lock.unlock() }
        entries.removeAll(keepingCapacity: false)
    }
}
