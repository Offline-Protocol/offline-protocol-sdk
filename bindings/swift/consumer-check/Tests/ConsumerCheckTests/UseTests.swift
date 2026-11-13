import XCTest
import ConsumerCheck

final class UseTests: XCTestCase {
    /// A call into the Rust library from a module that is not the package,
    /// through the product and nothing else. It links, or this does not run.
    func testTheLibraryIsReachedThroughTheProduct() throws {
        let address = try address(of: [UInt8](repeating: 7, count: 32))

        XCTAssertTrue(address.hasPrefix("off1"), "not an address: \(address)")
    }

    /// The typed conformance report is public, and the suite runs from an
    /// application's module over the store an application opens. The Keychain
    /// half of the wipe fails without a host application; the state store is
    /// still wiped.
    func testTheReferenceStateStoreIsSupported() throws {
        XCTAssertTrue(
            try referenceStateStoreIsSupported(
                appId: "consumer-check", scratchProfile: UUID().uuidString))
    }
}
