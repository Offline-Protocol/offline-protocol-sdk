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
