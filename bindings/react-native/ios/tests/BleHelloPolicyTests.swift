import XCTest
@testable import OfflineProtocol

/// Covers `BleHelloPolicy` only. That `BleManager` writes what it returns, after
/// the subscription and before the handshake reads, is pinned in
/// `react_native_ios_central_writes_its_hello` in the uniffi crate.
final class BleHelloPolicyTests: XCTestCase {

    private let assertion = Data(repeating: 7, count: 115)

    func testWritesTheAssertionOncePerConnection() {
        XCTAssertEqual(
            BleHelloPolicy.helloToWrite(identity: assertion, peerServesHello: true,
                                        alreadyWritten: false, maximumWriteLength: 512),
            assertion
        )
        XCTAssertNil(
            BleHelloPolicy.helloToWrite(identity: assertion, peerServesHello: true,
                                        alreadyWritten: true, maximumWriteLength: 512),
            "a discovery replay on a live link must not write a second hello"
        )
    }

    func testAPeerWithoutHelloGetsNone() {
        XCTAssertNil(BleHelloPolicy.helloToWrite(identity: assertion, peerServesHello: false,
                                                 alreadyWritten: false, maximumWriteLength: 512))
    }

    func testNoAssertionYetWritesNone() {
        XCTAssertNil(BleHelloPolicy.helloToWrite(identity: nil, peerServesHello: true,
                                                 alreadyWritten: false, maximumWriteLength: 512))
    }

    func testAnAssertionThatDoesNotFitOneWriteIsSkipped() {
        XCTAssertNil(BleHelloPolicy.helloToWrite(identity: assertion, peerServesHello: true,
                                                 alreadyWritten: false, maximumWriteLength: 114))
        XCTAssertNotNil(BleHelloPolicy.helloToWrite(identity: assertion, peerServesHello: true,
                                                    alreadyWritten: false, maximumWriteLength: 115))
    }

    func testTheChaptersBoundsAreHeld() {
        XCTAssertEqual(BleHelloPolicy.minLength, 96)
        XCTAssertEqual(BleHelloPolicy.maxLength, 512)
        for (count, expected) in [(95, false), (96, true), (512, true), (513, false)] {
            let value = Data(repeating: 1, count: count)
            let written = BleHelloPolicy.helloToWrite(identity: value, peerServesHello: true,
                                                      alreadyWritten: false, maximumWriteLength: 1024)
            XCTAssertEqual(written != nil, expected, "length \(count)")
        }
    }
}

/// Covers `BlePeripheralHelloPolicy` only. That `BleManager` serves Hello,
/// answers through this verdict and binds the central before announcing it
/// is pinned in `react_native_ios_peripheral_serves_hello` in the uniffi
/// crate.
final class BlePeripheralHelloPolicyTests: XCTestCase {

    func testTakesOneCompleteValueWithinTheBounds() {
        XCTAssertEqual(BlePeripheralHelloPolicy.verdict(length: 96, offset: 0, requestsForHello: 1), .accept)
        XCTAssertEqual(BlePeripheralHelloPolicy.verdict(length: 512, offset: 0, requestsForHello: 1), .accept)
    }

    func testRefusesALongWrite() {
        XCTAssertEqual(BlePeripheralHelloPolicy.verdict(length: 115, offset: 18, requestsForHello: 1), .invalidOffset)
        XCTAssertEqual(
            BlePeripheralHelloPolicy.verdict(length: 115, offset: 0, requestsForHello: 2),
            .invalidOffset,
            "a long write arrives as several requests in one call"
        )
    }

    func testRefusesALengthOutsideTheBounds() {
        XCTAssertEqual(BlePeripheralHelloPolicy.verdict(length: 95, offset: 0, requestsForHello: 1), .invalidLength)
        XCTAssertEqual(BlePeripheralHelloPolicy.verdict(length: 513, offset: 0, requestsForHello: 1), .invalidLength)
    }
}
