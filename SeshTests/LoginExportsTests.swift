import XCTest

final class LoginExportsTests: XCTestCase {
    func testAPlainShellGetsTheLoginShellsVariablesButNotTheShellsOwn() throws {
        let exported = try XCTUnwrap(exports(fromEnv: "PATH=/usr/bin:/bin\0GREETING=it's a \"walrus\"\nhere\0SHLVL=7\0BAD-NAME=x\0"))
        XCTAssertEqual(exported, "export PATH='/usr/bin:/bin'; export GREETING='it'\\''s a \"walrus\"\nhere'; ")
    }

    func testAnEnvironmentWithoutAPathLeavesTheLoginShellInPlace() {
        XCTAssertNil(exports(fromEnv: "HOME=/home/u\0"))
    }
}
