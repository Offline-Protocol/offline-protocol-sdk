//
// Development-only ordering hints for hosts that drive ProtocolEngineHost
// directly (the Flutter plugin, a hand-written Swift host). React Native
// turns them off, because its TypeScript layer already gives the same hints
// (lifecycleHints.ts).
//
// A hint that fires on correct code teaches developers to ignore the channel,
// so every hint here must be one a correctly ordered app never sees. That is
// why the data hint reacts to the engine refusing a call for want of storage,
// rather than predicting it from engine state: the engine gates data calls on
// storage, not on running, so any prediction from state is wrong on some
// valid order (after MLS init but before start, while stopped, while paused).
//
// The semantics mirror bindings/react-native/src/lifecycleHints.ts and the
// Kotlin IntegratorLifecycleHints; keep the three in step. One instance per
// host: the dedupe set and the applied interest belong to that host, and are
// read from whatever queue calls it, so both sit behind a lock.
//

import Foundation

final class IntegratorLifecycleHints {
    let enabled: Bool
    private let sink: (String) -> Void
    private let lock = NSLock()
    private var warnedKeys = Set<String>()
    /// The interest last applied, by space; an absent space has the default `["*"]`.
    private var appliedInterest: [String: [String]] = [:]

    private static let doc = "docs/ios-integration.md and docs/integrator-map.md"

    init(enabled: Bool, sink: @escaping (String) -> Void) {
        self.enabled = enabled
        self.sink = sink
    }

    /// Forgets what was warned and applied, for the next engine on this host.
    func reset() {
        lock.lock()
        warnedKeys.removeAll()
        appliedInterest.removeAll()
        lock.unlock()
    }

    private func warnOnce(key: String, message: String) {
        guard enabled else { return }
        lock.lock()
        let first = warnedKeys.insert(key).inserted
        lock.unlock()
        if first { sink(message) }
    }

    func callBeforeEngineStart(context: String, engineRunning: Bool) {
        if engineRunning { return }
        warnOnce(
            key: "engine:\(context)",
            message: "[OfflineProtocol] \(context): call start() before this API. "
                + "Install the event callback and declare data interest before start(). "
                + "See \(Self.doc)."
        )
    }

    func startWithoutMls(encryptionEnabled: Bool, mlsReady: Bool) {
        if !encryptionEnabled || mlsReady { return }
        warnOnce(
            key: "start:mls",
            message: "[OfflineProtocol] start() with encryption enabled before "
                + "initializeMlsWithSecureStorage(). Secure send and handshake fail until it runs, "
                + "or set encryptionEnabled to false for cleartext-only tests. See \(Self.doc)."
        )
    }

    func sendWithoutMls(context: String, encryptionEnabled: Bool, mlsReady: Bool, engineRunning: Bool) {
        if !engineRunning || !encryptionEnabled || mlsReady { return }
        warnOnce(
            key: "mls:\(context)",
            message: "[OfflineProtocol] \(context): encryption is enabled but MLS is not initialized. "
                + "Secure send and handshake may fail until it is. Run "
                + "initializeMlsWithSecureStorage() successfully, or set encryptionEnabled "
                + "to false for cleartext-only tests."
        )
    }

    /// The engine refused `context` with `DataStorageUnavailable`.
    func dataStorageUnavailable(context: String) {
        warnOnce(
            key: "data:\(context)",
            message: "[OfflineProtocol] \(context): the data layer has no storage yet. "
                + "Run initializeMlsWithSecureStorage() (or set a data storage) before data calls. "
                + "See docs/data.md."
        )
    }

    /// Records that the engine accepted `patterns` for `spaceId`, warning first
    /// when a running engine narrowed the space's interest. A widening is the
    /// supported post-start use (it asks peers for what it adds) and never warns.
    func interestApplied(spaceId: String, patterns: [String], engineRunning: Bool) {
        lock.lock()
        let previous = appliedInterest[spaceId] ?? ["*"]
        appliedInterest[spaceId] = patterns
        lock.unlock()
        guard engineRunning, Self.isInterestNarrowing(previous: previous, next: patterns) else { return }
        warnOnce(
            key: "interest:narrowed:\(spaceId)",
            message: "[OfflineProtocol] setInterest(\"\(spaceId)\") after start() narrowed the interest. "
                + "The narrower filter refuses documents from now on, but does not delete what "
                + "this device already holds, including what the start-up exchange pulled in. "
                + "Declare interest before start() when you need a tight filter from the first exchange."
        )
    }

    /// Whether interest pattern `outer` matches every document `inner`
    /// matches. A pattern is a document name, or a prefix ending in `*`.
    private static func patternCovers(_ outer: String, _ inner: String) -> Bool {
        if outer == inner { return true }
        guard outer.hasSuffix("*") else { return false }
        return inner.hasPrefix(String(outer.dropLast()))
    }

    /// Whether replacing `previous` with `next` drops any document `previous`
    /// matched. Conservative, as in lifecycleHints.ts: a pattern counts as
    /// kept only when one pattern of `next` covers it whole.
    static func isInterestNarrowing(previous: [String], next: [String]) -> Bool {
        previous.contains { old in !next.contains { patternCovers($0, old) } }
    }
}
