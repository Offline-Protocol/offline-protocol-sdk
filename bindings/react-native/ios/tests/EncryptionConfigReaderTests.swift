import XCTest
@testable import OfflineProtocol

/// Mirrors android/ `ProtocolConfigParserTest`'s encryption cases — keep the
/// two suites in sync. A silent regression here reverts a flag to its
/// default with no error anywhere, which is exactly how the four encryption
/// flags drifted (nested-only reads while the JS wrapper sent the flat
/// shape).
final class EncryptionConfigReaderTests: XCTestCase {

    private func read(_ json: String) throws -> EncryptionConfigValues {
        let data = try XCTUnwrap(json.data(using: .utf8))
        let raw = try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        return EncryptionConfigReader.read(raw)
    }

    func testEncryptionFlagsDefaultOnWhenOmitted() throws {
        let values = try read(#"{"appId":"app","userId":"alice"}"#)
        XCTAssertTrue(values.enabled)
        XCTAssertTrue(values.autoKeyExchange)
        XCTAssertTrue(values.storePending)
        XCTAssertTrue(values.requireEncryption)
        XCTAssertTrue(values.compactEnvelopeEnabled)
        XCTAssertTrue(values.richPayloadEnabled)
        XCTAssertEqual(values.maxPendingPerPeer, 64)
        XCTAssertEqual(values.maxPendingGlobal, 4096)
        XCTAssertEqual(values.pendingTtlMs, 120_000)
        XCTAssertEqual(values.overflowPolicyRaw, "drop_oldest")
    }

    func testEncryptionFlagsReadTheFlatCamelCaseShapeTheJsWrapperSends() throws {
        let values = try read(
            #"{"encryptionEnabled":false,"autoKeyExchange":false,"storePending":false,"requireEncryption":false}"#
        )
        XCTAssertFalse(values.enabled)
        XCTAssertFalse(values.autoKeyExchange)
        XCTAssertFalse(values.storePending)
        XCTAssertFalse(values.requireEncryption)
    }

    func testEncryptionFlagsReadTheirNestedHome() throws {
        let values = try read(
            #"{"encryption":{"enabled":false,"autoKeyExchange":false,"storePending":false,"requireEncryption":false}}"#
        )
        XCTAssertFalse(values.enabled)
        XCTAssertFalse(values.autoKeyExchange)
        XCTAssertFalse(values.storePending)
        XCTAssertFalse(values.requireEncryption)
    }

    func testEncryptionFlagsReadFlatSnakeCase() throws {
        let values = try read(
            #"{"encryption_enabled":false,"auto_key_exchange":false,"store_pending":false,"require_encryption":false}"#
        )
        XCTAssertFalse(values.enabled)
        XCTAssertFalse(values.autoKeyExchange)
        XCTAssertFalse(values.storePending)
        XCTAssertFalse(values.requireEncryption)
    }

    func testNestedEncryptionFlagsWinOverFlat() throws {
        let values = try read(
            #"{"encryptionEnabled":true,"autoKeyExchange":true,"storePending":true,"requireEncryption":true,"encryption":{"enabled":false,"autoKeyExchange":false,"storePending":false,"requireEncryption":false}}"#
        )
        XCTAssertFalse(values.enabled)
        XCTAssertFalse(values.autoKeyExchange)
        XCTAssertFalse(values.storePending)
        XCTAssertFalse(values.requireEncryption)
    }

    func testEncryptionSectionWithoutTheFlagsFallsThroughToFlatKeys() throws {
        let values = try read(
            #"{"encryptionEnabled":false,"requireEncryption":false,"encryption":{"compactEnvelopeEnabled":true}}"#
        )
        XCTAssertFalse(values.enabled)
        XCTAssertFalse(values.requireEncryption)
        XCTAssertTrue(values.compactEnvelopeEnabled)
        XCTAssertTrue(values.autoKeyExchange)
        XCTAssertTrue(values.storePending)
    }

    func testCompactEnvelopeReadsItsNestedHomeThenTopLevel() throws {
        let nested = try read(
            #"{"compactEnvelopeEnabled":true,"encryption":{"compactEnvelopeEnabled":false}}"#
        )
        XCTAssertFalse(nested.compactEnvelopeEnabled)

        let flat = try read(#"{"compactEnvelopeEnabled":false,"encryption":{"enabled":true}}"#)
        XCTAssertFalse(flat.compactEnvelopeEnabled)
    }

    func testRichPayloadReadsItsNestedHomeThenTopLevel() throws {
        let nested = try read(
            #"{"richPayloadEnabled":true,"encryption":{"richPayloadEnabled":false}}"#
        )
        XCTAssertFalse(nested.richPayloadEnabled)

        let flat = try read(#"{"richPayloadEnabled":false,"encryption":{"enabled":true}}"#)
        XCTAssertFalse(flat.richPayloadEnabled)

        let snake = try read(#"{"encryption":{"rich_payload_enabled":false}}"#)
        XCTAssertFalse(snake.richPayloadEnabled)
    }

    func testPendingQueueNestedHomeWinsOverFlat() throws {
        let values = try read(
            #"{"maxPendingPerPeer":1,"pendingTtlMs":1,"encryption":{"pendingQueue":{"maxPendingPerPeer":32,"pendingTtlMs":60000,"overflowPolicy":"drop_newest"}}}"#
        )
        XCTAssertEqual(values.maxPendingPerPeer, 32)
        XCTAssertEqual(values.pendingTtlMs, 60_000)
        XCTAssertEqual(values.overflowPolicyRaw, "drop_newest")
    }

    func testPendingQueueSnakeCaseAndFlatFallback() throws {
        let values = try read(
            #"{"max_pending_per_peer":16,"max_pending_global":256,"pending_ttl_ms":30000,"overflow_policy":"drop_newest"}"#
        )
        XCTAssertEqual(values.maxPendingPerPeer, 16)
        XCTAssertEqual(values.maxPendingGlobal, 256)
        XCTAssertEqual(values.pendingTtlMs, 30_000)
        XCTAssertEqual(values.overflowPolicyRaw, "drop_newest")
    }
}
