import XCTest

@MainActor
final class CloneTests: XCTestCase {
    func testALinkNamesTheFolderItClonesIntoAndAPathIsNoLink() {
        XCTAssertEqual(Start.cloneName(of: "git@github.com:atuinsh/atuin.git"), "atuin")
        XCTAssertEqual(Start.cloneName(of: "https://code.pecheny.me/pecheny/sesh"), "sesh")
        XCTAssertEqual(Start.cloneName(of: " ssh://git@host:2222/team/tool.git\n"), "tool")
        XCTAssertNil(Start.cloneName(of: "~/src/sesh"))
        XCTAssertNil(Start.cloneName(of: "/home/u/repo.git"))
        XCTAssertNil(Start.cloneName(of: "git@github.com:"))
    }

    func testNewFoldersGoWhereMostRecentRepositoriesSit() {
        XCTAssertEqual(Start.parent(of: ["~/src/sesh", "~/work/api", "~/src/herdr"]), "~/src")
        XCTAssertEqual(Start.parent(of: []), "~")
    }

    func testGitsProgressLinesGiveAPhaseAndHowFarAlong() {
        let receiving = Start.cloning("Receiving objects:  45% (450/1000), 1.2 MiB | 3 MiB/s")
        XCTAssertEqual(receiving?.phase, "Receiving objects")
        XCTAssertEqual(receiving?.done, 0.45)
        XCTAssertEqual(Start.cloning("remote: Counting objects: 100% (12/12), done.")?.phase, "Counting objects")
        XCTAssertNil(Start.cloning("fatal: repository 'x' not found"))
    }
}
