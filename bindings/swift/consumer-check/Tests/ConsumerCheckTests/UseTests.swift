import XCTest
import ConsumerCheck

final class UseTests: XCTestCase {
    /// A call into the Rust library from a module that is not the package,
    /// through the product and nothing else. It links, or this does not run.
    func testTheLibraryIsReachedThroughTheProduct() throws {
        let address = try address(of: [UInt8](repeating: 7, count: 32))

        XCTAssertTrue(address.hasPrefix("off1"), "not an address: \(address)")
    }
}
