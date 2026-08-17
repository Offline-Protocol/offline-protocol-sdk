import XCTest
@testable import OfflineProtocol

/// Mirrors android/ `ProtocolConfigParserTest`'s mesh-relay cases — keep the
/// two suites in sync.
///
/// The property under test is unusual and worth stating: this reader resolves
/// nothing. Absent must stay absent all the way to the core, because the core
/// owns every default. A reader that helpfully filled one in would give apps
/// its own idea of the defaults, with nothing comparing the two.
final class MeshRelayConfigReaderTests: XCTestCase {

    private func read(_ json: String) throws -> MeshRelayConfigValues? {
        let data = try XCTUnwrap(json.data(using: .utf8))
        let raw = try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any]
        )
        return MeshRelayConfigReader.read(raw)
    }

    func testSectionIsAbsentWhenOmitted() throws {
        // Nil, not an object of nils: the module passes nil across the FFI and
        // the core keeps every default untouched.
        XCTAssertNil(try read(#"{"appId":"app","userId":"alice"}"#))
    }

    func testSectionReadsItsNestedCamelCaseHome() throws {
        let values = try XCTUnwrap(try read(#"""
        {"appId":"app","meshRelay":{"maxTtl":6,"denseMaxTtl":4,"denseDegree":9,"fanout":2,"jitterMinMs":35,"jitterMaxMs":175,"ratePerSec":12.5,"burst":41,"peerRatePerSec":6.5,"peerBurst":17,"queueCapacity":321,"biasMinScale":0.4,"biasMaxHandicapMs":275,"activityWindowMs":45000,"activityMinForwards":5,"activityIdleWindows":3}}
        """#))

        XCTAssertEqual(values.maxTtl, 6)
        XCTAssertEqual(values.denseMaxTtl, 4)
        XCTAssertEqual(values.denseDegree, 9)
        XCTAssertEqual(values.fanout, 2)
        XCTAssertEqual(values.jitterMinMs, 35)
        XCTAssertEqual(values.jitterMaxMs, 175)
        XCTAssertEqual(values.ratePerSec, 12.5)
        XCTAssertEqual(values.burst, 41)
        XCTAssertEqual(values.peerRatePerSec, 6.5)
        XCTAssertEqual(values.peerBurst, 17)
        XCTAssertEqual(values.queueCapacity, 321)
        XCTAssertEqual(values.biasMinScale, 0.4)
        XCTAssertEqual(values.biasMaxHandicapMs, 275)
        XCTAssertEqual(values.activityWindowMs, 45000)
        XCTAssertEqual(values.activityMinForwards, 5)
        XCTAssertEqual(values.activityIdleWindows, 3)
    }

    func testSectionReadsNestedSnakeCase() throws {
        let values = try XCTUnwrap(try read(#"""
        {"appId":"app","mesh_relay":{"max_ttl":7,"dense_max_ttl":3,"dense_degree":8,"jitter_min_ms":10,"jitter_max_ms":150,"rate_per_sec":9.5,"peer_rate_per_sec":4.5,"peer_burst":12,"queue_capacity":128,"bias_min_scale":0.5,"bias_max_handicap_ms":300,"activity_window_ms":30000,"activity_min_forwards":4,"activity_idle_windows":5}}
        """#))

        XCTAssertEqual(values.maxTtl, 7)
        XCTAssertEqual(values.denseMaxTtl, 3)
        XCTAssertEqual(values.denseDegree, 8)
        XCTAssertEqual(values.jitterMinMs, 10)
        XCTAssertEqual(values.jitterMaxMs, 150)
        XCTAssertEqual(values.ratePerSec, 9.5)
        XCTAssertEqual(values.peerRatePerSec, 4.5)
        XCTAssertEqual(values.peerBurst, 12)
        XCTAssertEqual(values.queueCapacity, 128)
        XCTAssertEqual(values.biasMinScale, 0.5)
        XCTAssertEqual(values.biasMaxHandicapMs, 300)
        XCTAssertEqual(values.activityWindowMs, 30000)
        XCTAssertEqual(values.activityMinForwards, 4)
        XCTAssertEqual(values.activityIdleWindows, 5)
    }

    func testUnnamedFieldsStayNil() throws {
        // A partial section is the ordinary case: an app sets the one dial it
        // cares about. Everything it did not name must arrive nil so the core
        // keeps its own value — this is the DORS silent-reset failure in the
        // shape it would take here.
        let values = try XCTUnwrap(try read(#"{"appId":"app","meshRelay":{"fanout":7}}"#))

        XCTAssertEqual(values.fanout, 7)
        XCTAssertNil(values.maxTtl)
        XCTAssertNil(values.denseMaxTtl)
        XCTAssertNil(values.denseDegree)
        XCTAssertNil(values.jitterMinMs)
        XCTAssertNil(values.jitterMaxMs)
        XCTAssertNil(values.ratePerSec)
        XCTAssertNil(values.burst)
        XCTAssertNil(values.peerRatePerSec)
        XCTAssertNil(values.peerBurst)
        XCTAssertNil(values.queueCapacity)
        XCTAssertNil(values.biasMinScale)
        XCTAssertNil(values.biasMaxHandicapMs)
        XCTAssertNil(values.activityWindowMs)
        XCTAssertNil(values.activityMinForwards)
        XCTAssertNil(values.activityIdleWindows)
    }

    func testNegativesAreClampedRatherThanTrapping() throws {
        // These are unsigned across the FFI. `UInt64(-1)` traps outright in
        // Swift, so an app typo would crash the bridge rather than be refused;
        // clamped to zero it reaches the core's validation instead.
        let values = try XCTUnwrap(try read(
            #"{"appId":"app","meshRelay":{"fanout":-1,"maxTtl":-5,"activityIdleWindows":-2}}"#
        ))

        XCTAssertEqual(values.fanout, 0)
        XCTAssertEqual(values.maxTtl, 0)
        XCTAssertEqual(values.activityIdleWindows, 0)
    }

    func testABooleanIsNotReadAsANumber() throws {
        // Bool bridges as NSNumber through JSONSerialization, so without an
        // explicit exclusion a stray `true` would arrive as a fan-out of 1.
        let values = try XCTUnwrap(try read(#"{"appId":"app","meshRelay":{"fanout":true}}"#))
        XCTAssertNil(values.fanout)
    }
}
