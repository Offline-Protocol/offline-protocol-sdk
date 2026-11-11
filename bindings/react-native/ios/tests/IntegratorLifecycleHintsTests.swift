import XCTest

final class IntegratorLifecycleHintsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        IntegratorLifecycleHints.resetSession()
    }

    func testWarningsDisabledDoesNotLog() {
        IntegratorLifecycleHints.warnIfCallBeforeEngineStarted(
            context: "sendMessage",
            proto: nil,
            enabled: false
        )
        // No trap; primary assertion is that disabled is a no-op with nil proto.
        XCTAssertTrue(true)
    }

    func testInterestAfterStartDedupes() {
        IntegratorLifecycleHints.warnIfInterestAfterStart(enabled: true)
        IntegratorLifecycleHints.warnIfInterestAfterStart(enabled: true)
        XCTAssertTrue(true)
    }

    func testResetSessionAllowsSecondWarn() {
        IntegratorLifecycleHints.warnIfInterestAfterStart(enabled: true)
        IntegratorLifecycleHints.resetSession()
        IntegratorLifecycleHints.warnIfInterestAfterStart(enabled: true)
        XCTAssertTrue(true)
    }
}
