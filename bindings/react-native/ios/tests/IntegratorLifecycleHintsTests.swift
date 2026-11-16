import XCTest
@testable import OfflineProtocol

/// The hints mirror lifecycleHints.ts: each fires only on a real ordering
/// mistake, once per key, and nothing at all when disabled (how React Native
/// runs the host).
final class IntegratorLifecycleHintsTests: XCTestCase {
    private var messages: [String] = []

    override func setUp() {
        super.setUp()
        messages = []
    }

    private func hints(enabled: Bool = true) -> IntegratorLifecycleHints {
        IntegratorLifecycleHints(enabled: enabled) { [unowned self] in self.messages.append($0) }
    }

    func testDisabledHintsSayNothing() {
        let h = hints(enabled: false)
        h.callBeforeEngineStart(context: "sendMessage", engineRunning: false)
        h.startWithoutMls(encryptionEnabled: true, mlsReady: false)
        h.sendWithoutMls(context: "sendMessage", encryptionEnabled: true, mlsReady: false, engineRunning: true)
        h.dataStorageUnavailable(context: "dataCreateDoc")
        h.interestApplied(spaceId: "notes", patterns: ["a"], engineRunning: true)
        XCTAssertEqual(messages, [])
    }

    func testEngineHintFiresOncePerContextAndNeverWhileRunning() {
        let h = hints()
        h.callBeforeEngineStart(context: "sendMessage", engineRunning: true)
        XCTAssertTrue(messages.isEmpty)
        h.callBeforeEngineStart(context: "sendMessage", engineRunning: false)
        h.callBeforeEngineStart(context: "sendMessage", engineRunning: false)
        h.callBeforeEngineStart(context: "enableTransport(ble)", engineRunning: false)
        XCTAssertEqual(messages.count, 2)
        XCTAssertTrue(messages[0].contains("sendMessage: call start() before this API"))
        XCTAssertTrue(messages[1].contains("enableTransport(ble)"))
    }

    func testMlsHintsNeedEncryptionAndAMissingMls() {
        let h = hints()
        h.startWithoutMls(encryptionEnabled: false, mlsReady: false)
        h.startWithoutMls(encryptionEnabled: true, mlsReady: true)
        h.sendWithoutMls(context: "sendMessage", encryptionEnabled: true, mlsReady: false, engineRunning: false)
        h.sendWithoutMls(context: "sendMessage", encryptionEnabled: true, mlsReady: true, engineRunning: true)
        h.sendWithoutMls(context: "sendMessage", encryptionEnabled: false, mlsReady: false, engineRunning: true)
        XCTAssertTrue(messages.isEmpty)
        h.startWithoutMls(encryptionEnabled: true, mlsReady: false)
        h.sendWithoutMls(context: "sendMessage", encryptionEnabled: true, mlsReady: false, engineRunning: true)
        XCTAssertEqual(messages.count, 2)
        XCTAssertTrue(messages[0].hasPrefix("[OfflineProtocol] start() with encryption enabled"))
        XCTAssertTrue(messages[1].contains("sendMessage: encryption is enabled but MLS is not initialized"))
    }

    func testDataHintNamesTheRefusedCallOnce() {
        let h = hints()
        h.dataStorageUnavailable(context: "dataCreateDoc")
        h.dataStorageUnavailable(context: "dataCreateDoc")
        h.dataStorageUnavailable(context: "dataMapSet")
        XCTAssertEqual(messages.count, 2)
        XCTAssertTrue(messages[0].contains("dataCreateDoc: the data layer has no storage yet"))
        XCTAssertTrue(messages[1].contains("dataMapSet"))
    }

    func testInterestWarnsOnlyOnAPostStartNarrowing() {
        let h = hints()
        h.interestApplied(spaceId: "notes", patterns: ["a*"], engineRunning: false)
        XCTAssertTrue(messages.isEmpty)
        h.interestApplied(spaceId: "notes", patterns: ["a*", "b*"], engineRunning: true)
        h.interestApplied(spaceId: "notes", patterns: ["*"], engineRunning: true)
        XCTAssertTrue(messages.isEmpty)
        h.interestApplied(spaceId: "notes", patterns: ["a*"], engineRunning: true)
        h.interestApplied(spaceId: "notes", patterns: ["*"], engineRunning: true)
        h.interestApplied(spaceId: "notes", patterns: ["a1"], engineRunning: true)
        XCTAssertEqual(messages.count, 1)
        XCTAssertTrue(messages[0].contains("setInterest(\"notes\") after start() narrowed"))
        h.interestApplied(spaceId: "photos", patterns: ["x"], engineRunning: true)
        XCTAssertEqual(messages.count, 2)
    }

    func testResetForgetsWarnedKeysAndAppliedInterest() {
        let h = hints()
        h.callBeforeEngineStart(context: "sendMessage", engineRunning: false)
        h.interestApplied(spaceId: "notes", patterns: ["a*"], engineRunning: false)
        h.reset()
        h.callBeforeEngineStart(context: "sendMessage", engineRunning: false)
        h.interestApplied(spaceId: "notes", patterns: ["a*"], engineRunning: true)
        XCTAssertEqual(messages.count, 3)
    }

    func testNarrowingMatchesTheTypeScriptRule() {
        let n = IntegratorLifecycleHints.isInterestNarrowing
        XCTAssertFalse(n(["*"], ["*"]))
        XCTAssertFalse(n(["a*"], ["*"]))
        XCTAssertFalse(n(["a1"], ["a*"]))
        XCTAssertFalse(n(["ab*"], ["a*"]))
        XCTAssertTrue(n(["*"], ["a*"]))
        XCTAssertTrue(n(["a*"], ["a1"]))
        XCTAssertTrue(n(["a*"], ["ab*", "a"]))
        XCTAssertTrue(n(["a1", "b1"], ["a1"]))
        XCTAssertFalse(n([], ["a"]))
    }

    func testConcurrentCallersWarnOnce() {
        let lock = NSLock()
        var count = 0
        let counted = IntegratorLifecycleHints(enabled: true) { _ in
            lock.lock(); count += 1; lock.unlock()
        }
        DispatchQueue.concurrentPerform(iterations: 200) { i in
            counted.callBeforeEngineStart(context: "sendMessage", engineRunning: false)
            counted.interestApplied(spaceId: "s\(i % 4)", patterns: ["a"], engineRunning: true)
        }
        XCTAssertEqual(count, 5)
    }
}
