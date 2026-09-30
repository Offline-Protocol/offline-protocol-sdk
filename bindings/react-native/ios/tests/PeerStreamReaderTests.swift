//
// PeerStreamReaderTests.swift
//
// Pins how the iOS peer-stream manager cuts a TCP stream into frames
// (docs/spec/stream-framing.md, "What a receiver owes", steps one and two):
// whatever the chunking, each frame comes out whole, and a prefix outside the
// bounds is refused on its four bytes, before any body is buffered. Android
// does the same in PeerStreamFraming.readBody.
//

import XCTest
@testable import OfflineProtocol

final class PeerStreamReaderTests: XCTestCase {

    private func frame(_ body: Data) -> Data { PeerStreamFraming.frame(body)! }

    private func frames(_ results: [Result<Data, PeerStreamRefusal>]) -> [Data] {
        results.compactMap { try? $0.get() }
    }

    func testAFrameSplitAtEveryByteComesOutWholeOnce() {
        let whole = frame(Data("hello".utf8))
        let reader = PeerStreamReader()
        var out: [Data] = []
        for byte in whole {
            out += frames(reader.append(Data([byte])))
        }
        XCTAssertEqual(out, [whole])
    }

    func testSeveralFramesInOneChunkComeOutInOrder() {
        let one = frame(Data("one".utf8))
        let two = frame(Data(repeating: 7, count: 300))
        let three = frame(Data("three".utf8))
        let reader = PeerStreamReader()
        var chunk = one + two + three
        let tail = chunk.suffix(2)
        chunk.removeLast(2)
        XCTAssertEqual(frames(reader.append(chunk)), [one, two])
        XCTAssertEqual(frames(reader.append(Data(tail))), [three])
    }

    func testAFrameOfExactlyTheCeilingIsRead() {
        let whole = frame(Data(count: PeerStreamFraming.maxBodyBytes))
        let out = PeerStreamReader().append(whole)
        XCTAssertEqual(frames(out), [whole])
    }

    func testAPrefixOverTheCeilingIsRefusedBeforeAnyBody() {
        let reader = PeerStreamReader()
        // 1 MiB + 1, and not a byte of body sent.
        let out = reader.append(Data([0x00, 0x10, 0x00, 0x01]))
        XCTAssertEqual(out, [.failure(PeerStreamRefusal(reason: "length over the ceiling"))])
        // The stream is garbage from here: nothing more comes out.
        XCTAssertTrue(reader.append(frame(Data("after".utf8))).isEmpty)
    }

    func testAZeroLengthPrefixIsRefused() {
        let out = PeerStreamReader().append(Data([0, 0, 0, 0, 1, 2, 3]))
        XCTAssertEqual(out, [.failure(PeerStreamRefusal(reason: "zero length"))])
    }

    func testFramesBeforeARefusalAreStillReturned() {
        let good = frame(Data("good".utf8))
        let out = PeerStreamReader().append(good + Data([0, 0, 0, 0]))
        XCTAssertEqual(out, [.success(good), .failure(PeerStreamRefusal(reason: "zero length"))])
    }

    func testTheFloorIsLeftToUnframe() {
        // A short first body is only wrong in the preamble position, which the
        // reader does not know; `unframe` refuses it there.
        let short = frame(Data("hi".utf8))
        XCTAssertEqual(frames(PeerStreamReader().append(short)), [short])
        XCTAssertEqual(
            PeerStreamFraming.unframe(short, preamble: true),
            .failure(PeerStreamRefusal(reason: "preamble under the floor")))
    }
}
