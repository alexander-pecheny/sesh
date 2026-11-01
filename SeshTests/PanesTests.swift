import XCTest

final class PanesTests: XCTestCase {
    private func summary(_ fields: String) -> Panes.Summary {
        Panes.summary(#"{"t":"session","session":"w1:p1",\#(fields)}"#)!
    }

    private func session(name: String? = nil, path: String? = "/src/app", pane: String = "w1:p1") -> Record {
        Record(id: "s1", kind: .session, body: .init(machine: "vps", path: path, pane: pane, name: name))
    }

    func testOnlySessionLinesAreSummaries() {
        XCTAssertNil(Panes.summary(#"{"t":"hello","protocol":5}"#))
        XCTAssertNil(Panes.summary("not json"))
        XCTAssertNotNil(Panes.summary(#"{"t":"session","session":"w1:p1"}"#))
    }

    func testAnAgentNoSessionHoldsIsUnfiled() {
        let summaries = ["vps": ["w1:p1": summary(#""state":"idle","agent":"claude","cwd":"/src/app","name":"app""#)]]
        XCTAssertEqual(Panes(summaries, sessions: []).unfiled["vps"]?.map(\.name), ["app"])
        XCTAssertNil(Panes(summaries, sessions: [session(name: "app")]).unfiled["vps"])
    }

    func testALaterAgentInTheSamePaneIsNotTheSessions() {
        let panes = Panes(["vps": ["w1:p1": summary(#""state":"working","name":"other""#)]], sessions: [])
        let mine = session(name: "app")
        XCTAssertFalse(panes.holds(mine))
        XCTAssertNil(panes.mark(of: mine, seen: [:]))
        XCTAssertEqual(panes.mark(of: session(name: "other"), seen: [:]), .working)
    }

    func testASessionWithoutANameIsKnownByItsFolder() {
        let panes = Panes(["vps": ["w1:p1": summary(#""state":"idle","cwd":"/elsewhere/app","name":"app-2""#)]], sessions: [])
        XCTAssertEqual(panes.name(of: session()), "app-2")
        XCTAssertNil(panes.name(of: session(path: "/src/web")))
        XCTAssertNil(panes.name(of: session(name: "app")))
    }

    func testMarksRankWaitingOverWorkingOverFinished() {
        func mark(_ fields: String, seen: UInt64? = nil) -> Panes.Mark? {
            let panes = Panes(["vps": ["w1:p1": summary(fields)]], sessions: [])
            return panes.mark(of: session(), seen: seen.map { ["vps:w1:p1": $0] } ?? [:])
        }
        XCTAssertEqual(mark(#""state":"blocked""#), .waiting)
        XCTAssertEqual(mark(#""state":"working""#), .working)
        XCTAssertEqual(mark(#""state":"background","background":[{"call":"c","label":"tests"}]"#), .background)
        XCTAssertEqual(mark(#""state":"idle","done":3"#, seen: 2), .finished)
        XCTAssertEqual(mark(#""state":"idle","done":3"#, seen: 3), .seen)
        XCTAssertEqual(mark(#""state":"idle","done":3"#), .seen)
        XCTAssertNil(mark(#""state":"ended""#))
    }

    func testTheLastMessageTimeIsRead() {
        let panes = Panes(["vps": ["w1:p1": summary(#""state":"idle","last":"2026-11-01T02:00:00.500Z""#)]], sessions: [])
        XCTAssertEqual(panes.last["vps:w1:p1"], 1_793_498_400_500)
    }
}
