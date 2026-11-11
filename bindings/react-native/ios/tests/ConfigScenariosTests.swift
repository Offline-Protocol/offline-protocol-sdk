import XCTest

/// Full-module tests; excluded from the Foundation-only SwiftPM harness.
final class ConfigScenariosTests: XCTestCase {
    func testLocalBleParsesFromJson() throws {
        let json = try ConfigScenarios.forScenarioJson(.localBle, appId: "a", profile: "p")
        let (config, _) = try ProtocolConfigParser.parse(json)
        XCTAssertTrue(config.bleEnabled)
        XCTAssertFalse(config.internetEnabled)
        XCTAssertFalse(config.wifiDirectEnabled)
        XCTAssertFalse(config.nostrEnabled)
    }

    func testInternetRelayPreferOnline() throws {
        let config = ConfigScenarios.forScenario(.internetRelay, appId: "a", profile: "p")
        XCTAssertTrue(config.internetEnabled)
        XCTAssertTrue(config.preferOnline)
    }

    func testDevelopmentCleartextDisablesEncryption() {
        let config = ConfigScenarios.forScenario(.developmentCleartext, appId: "a", profile: "p")
        XCTAssertFalse(config.encryptionEnabled)
        XCTAssertFalse(config.requireEncryption)
    }
}
