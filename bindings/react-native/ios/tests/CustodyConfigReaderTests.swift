import XCTest
@testable import OfflineProtocol

/// Mirrors android/ `ProtocolConfigParserTest`'s custody cases, and keeps the
/// two suites in sync.
///
/// The property under test is the one the mesh forwarding reader pins: this
/// reader resolves nothing. Absent must stay absent all the way to the core,
/// whose default for custody is off, and a reader that wrote that default as a
/// literal would keep every app that omitted the section on it forever.
final class CustodyConfigReaderTests: XCTestCase {

    private func read(_ json: String) throws -> CustodyConfigValues? {
        let data = try XCTUnwrap(json.data(using: .utf8))
        let raw = try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        return CustodyConfigReader.read(raw)
    }

    func testSectionIsAbsentWhenOmitted() throws {
        // Nil, not an object of nils: the module passes nil across the FFI and
        // the core keeps every default untouched, off included.
        XCTAssertNil(try read(#"{"appId":"app","userId":"alice"}"#))
    }

    func testSectionReadsItsNestedCamelCaseHome() throws {
        let values = try XCTUnwrap(try read(#"""
        {"appId":"app","custody":{"enabled":true,"holdMs":3600000,"maxEntriesPerDepositor":16,"maxBytesPerDepositor":131072,"maxEntries":128,"maxBytes":4194304,"strangerMaxEntries":2,"strangerMaxBytes":65536,"overflowPolicy":"drop_newest"}}
        """#))

        XCTAssertEqual(values.enabled, true)
        XCTAssertEqual(values.holdMs, 3_600_000)
        XCTAssertEqual(values.maxEntriesPerDepositor, 16)
        XCTAssertEqual(values.maxBytesPerDepositor, 131_072)
        XCTAssertEqual(values.maxEntries, 128)
        XCTAssertEqual(values.maxBytes, 4_194_304)
        XCTAssertEqual(values.strangerMaxEntries, 2)
        XCTAssertEqual(values.strangerMaxBytes, 65_536)
        XCTAssertEqual(values.overflowPolicy, "drop_newest")
    }

    func testSectionReadsNestedSnakeCase() throws {
        let values = try XCTUnwrap(try read(#"""
        {"appId":"app","custody":{"enabled":false,"hold_ms":7200000,"max_entries_per_depositor":8,"max_bytes_per_depositor":65536,"max_entries":64,"max_bytes":1048576,"stranger_max_entries":1,"stranger_max_bytes":65536,"overflow_policy":"drop_oldest"}}
        """#))

        XCTAssertEqual(values.enabled, false)
        XCTAssertEqual(values.holdMs, 7_200_000)
        XCTAssertEqual(values.maxEntriesPerDepositor, 8)
        XCTAssertEqual(values.maxBytesPerDepositor, 65_536)
        XCTAssertEqual(values.maxEntries, 64)
        XCTAssertEqual(values.maxBytes, 1_048_576)
        XCTAssertEqual(values.strangerMaxEntries, 1)
        XCTAssertEqual(values.strangerMaxBytes, 65_536)
        XCTAssertEqual(values.overflowPolicy, "drop_oldest")
    }

    func testLeavesUnnamedFieldsNil() throws {
        // The ordinary case: an app switches custody on and names nothing
        // else. Every other field must arrive nil so the core keeps its own
        // value.
        let values = try XCTUnwrap(try read(#"{"appId":"app","custody":{"enabled":true}}"#))
        XCTAssertEqual(values.enabled, true)
        XCTAssertNil(values.holdMs)
        XCTAssertNil(values.maxEntriesPerDepositor)
        XCTAssertNil(values.maxBytesPerDepositor)
        XCTAssertNil(values.maxEntries)
        XCTAssertNil(values.maxBytes)
        XCTAssertNil(values.strangerMaxEntries)
        XCTAssertNil(values.strangerMaxBytes)
        XCTAssertNil(values.overflowPolicy)
    }

    func testAnEmptySectionIsPresentAndEmpty() throws {
        // Present but naming nothing: still an object, every field nil, so the
        // module sends an all-nil dictionary and the core still keeps every
        // default. Distinguished from absent only in that it proves the app
        // meant to mention custody.
        let values = try XCTUnwrap(try read(#"{"appId":"app","custody":{}}"#))
        XCTAssertEqual(values, CustodyConfigValues())
    }

    func testNegativeNumbersClampToZeroRatherThanTrapping() throws {
        // App-supplied JS. A negative would trap the unsigned initializer;
        // clamped low it reaches the core's own validation.
        let values = try XCTUnwrap(try read(#"{"appId":"app","custody":{"holdMs":-5,"maxEntries":-1}}"#))
        XCTAssertEqual(values.holdMs, 0)
        XCTAssertEqual(values.maxEntries, 0)
    }

    func testABooleanIsNotReadAsANumber() throws {
        let values = try XCTUnwrap(try read(#"{"appId":"app","custody":{"holdMs":true}}"#))
        XCTAssertNil(values.holdMs)
    }

    func testZeroAndOneAreNumbersNotBooleans() throws {
        // Swift bridges the numbers 0 and 1 to Bool, so an `is Bool` exclusion
        // reads both as unset. A stranger tier of one entry, or a hold the core
        // must refuse, has to arrive.
        let values = try XCTUnwrap(try read(#"{"appId":"app","custody":{"strangerMaxEntries":1,"holdMs":0}}"#))
        XCTAssertEqual(values.strangerMaxEntries, 1)
        XCTAssertEqual(values.holdMs, 0)
    }
}
