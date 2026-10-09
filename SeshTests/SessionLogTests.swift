import XCTest

final class SessionLogTests: XCTestCase {
    private static let root = URL(filePath: #filePath).deletingLastPathComponent().deletingLastPathComponent()

    private func line(_ object: [String: Any]) -> String {
        String(decoding: try! JSONSerialization.data(withJSONObject: object), as: UTF8.self)
    }

    private func item(_ id: String, ord: Int, seq: Int, final: Bool = true, gone: Bool = false,
                      entry: String? = nil, kind: String = "text", text: String = "", extra: [String: Any] = [:]) -> String {
        var body: [String: Any] = ["id": entry ?? id, "kind": kind, "summary": text, "text": text]
        body.merge(extra) { $1 }
        return line(["t": "item", "session": "w1:p1", "id": id, "ord": ord, "seq": seq, "final": final, "gone": gone, "entry": body])
    }

    private let hello = #"{"t":"hello","protocol":5,"version":"test"}"#

    private func texts(_ log: SessionLog) -> [String] {
        log.rows.flatMap(\.entries).map { $0.text ?? $0.summary }
    }

    /// Each recording the Rust replay test holds, as a device watching it was sent, ends with the
    /// rows of the items it expects, never shows a message twice, and keeps the row of every
    /// screen item the Transcript takes over.
    func testRecordingsReplayToTheirExpectedRows() throws {
        let fixtures = Self.root.appending(path: "SeshTests/Fixtures")
        let names = try FileManager.default.contentsOfDirectory(atPath: fixtures.path()).filter { $0.hasSuffix(".jsonl") }
        XCTAssertFalse(names.isEmpty)
        for name in names {
            var log = SessionLog()
            var liveRows: Set<String> = []
            var takenOver = 0
            for text in try String(contentsOf: fixtures.appending(path: name), encoding: .utf8).split(separator: "\n") {
                log.feed(String(text))
                for row in log.rows {
                    let entry = row.entries.first
                    if entry.map({ SessionLog.isLive($0.id) }) == true { liveRows.insert(row.id) }
                    else if liveRows.remove(row.id) != nil { takenOver += 1 }
                }
                let said = log.rows.flatMap(\.entries).filter { ["user", "text"].contains($0.kind) }.map { "\($0.kind): \($0.text ?? "")" }
                XCTAssertEqual(said.count, Set(said).count, "\(name) showed a message twice")
            }
            XCTAssertNil(log.problem)
            XCTAssertTrue(log.loaded)
            XCTAssertGreaterThan(takenOver, 0, "\(name) never kept a screen row")
            let expected = try String(contentsOf: Self.root.appending(path: "core/sesh-transcript/tests/sessions")
                .appending(path: name.replacingOccurrences(of: ".jsonl", with: ".expected")), encoding: .utf8)
                .split(separator: "\n").map { try JSONDecoder().decode(String.self, from: Data($0.utf8)) }
                .filter { !$0.hasPrefix("result:") && !$0.hasPrefix("todo:") }
            let got = log.rows.flatMap(\.entries).map { "\($0.kind): \(($0.text ?? $0.command ?? $0.summary).trimmingCharacters(in: .whitespacesAndNewlines))" }
            XCTAssertEqual(got, expected, name)
        }
    }

    func testRowsFollowTheOrderNotTheArrival() {
        var log = SessionLog()
        [hello, item("c", ord: 3, seq: 5, text: "three"), item("a", ord: 1, seq: 6, text: "one"), item("b", ord: 2, seq: 7, text: "two")].forEach { log.feed($0) }
        XCTAssertEqual(texts(log), ["one", "two", "three"])
        XCTAssertEqual(log.seq, 7)
    }

    func testTheScreensRowsComeAfterTheFinalOnes() {
        var log = SessionLog()
        [item("live.1.9", ord: 1, seq: 1, final: false, entry: "live.1", text: "typing"), item("a", ord: 2, seq: 2, text: "said")].forEach { log.feed($0) }
        XCTAssertEqual(texts(log), ["said", "typing"])
    }

    func testARowTheTranscriptTakesOverKeepsItsId() {
        var log = SessionLog()
        log.feed(item("live.1.9", ord: 1, seq: 1, final: false, entry: "live.1", text: "Hello"))
        let id = log.rows[0].id
        log.feed(item("live.1.9", ord: 1, seq: 2, entry: "e1", text: "Hello there"))
        XCTAssertEqual(log.rows.map(\.id), [id])
        XCTAssertEqual(log.rowKey("e1"), id)
        XCTAssertEqual(log.rows[0].entries.map(\.id), ["e1"])
    }

    func testARewriteOfTheSameLengthBumpsTheVersion() {
        var log = SessionLog()
        log.feed(item("a", ord: 1, seq: 1, text: "cat"))
        let version = log.rows[0].version
        log.feed(item("a", ord: 1, seq: 2, text: "cot"))
        XCTAssertNotEqual(log.rows[0].version, version)
        let again = log.rows[0].version
        log.feed(item("a", ord: 1, seq: 3, text: "cot", extra: ["images": ["/tmp/a.png"]]))
        XCTAssertNotEqual(log.rows[0].version, again)
    }

    func testAResultBumpsItsToolsVersion() {
        var log = SessionLog()
        log.feed(item("t1", ord: 1, seq: 1, kind: "tool", extra: ["tool": "bash", "command": "ls"]))
        let version = log.rows[0].version
        log.feed(item("r1", ord: 2, seq: 2, kind: "result", text: "a b", extra: ["call": "t1"]))
        XCTAssertEqual(log.rows.count, 1)
        XCTAssertEqual(log.results["t1"]?.text, "a b")
        XCTAssertNotEqual(log.rows[0].version, version)
    }

    func testReadsAndSearchesShareOneRow() {
        var log = SessionLog()
        [item("r1", ord: 1, seq: 1, kind: "tool", extra: ["tool": "read"]), item("s1", ord: 2, seq: 2, kind: "tool", extra: ["tool": "search"]),
         item("t", ord: 3, seq: 3, text: "done")].forEach { log.feed($0) }
        XCTAssertEqual(log.rows.map(\.id), ["r1", "t"])
        XCTAssertTrue(log.rows[0].contains("s1"))
    }

    func testAGoneItemVanishes() {
        var log = SessionLog()
        log.feed(item("live.1.9", ord: 1, seq: 1, final: false, entry: "live.1", text: "maybe"))
        log.feed(item("live.1.9", ord: 1, seq: 2, final: false, gone: true, entry: "live.1", text: "maybe"))
        XCTAssertTrue(log.rows.isEmpty)
    }

    /// A follower that sends `opened` has resent its last items, perhaps from a new log; the
    /// items held from before go, while a resume that sends no `opened` keeps them.
    func testOpenedDropsTheItemsHeldFromBefore() {
        let kept = [item("old1", ord: 1, seq: 10, text: "old one"), item("old2", ord: 2, seq: 11, text: "old two")]
            .map { try! JSONDecoder().decode(SessionLog.Item.self, from: Data($0.utf8)) }
        var resumed = SessionLog()
        resumed.restore(kept, seq: 11)
        [hello, item("new", ord: 3, seq: 12, text: "new")].forEach { resumed.feed($0) }
        XCTAssertEqual(texts(resumed), ["old one", "old two", "new"])
        XCTAssertEqual(resumed.seq, 12)

        var reopened = SessionLog()
        reopened.restore(kept, seq: 11)
        [hello, item("x", ord: 1, seq: 5, text: "fresh one"), item("old2", ord: 2, seq: 6, text: "old two"),
         #"{"t":"opened","session":"w1:p1","seq":7}"#].forEach { reopened.feed($0) }
        XCTAssertEqual(texts(reopened), ["fresh one", "old two"])
        XCTAssertEqual(reopened.seq, 7)
        XCTAssertTrue(reopened.earlier)
    }

    func testAnUnknownProtocolIsAProblem() {
        var log = SessionLog()
        log.feed(hello)
        XCTAssertNil(log.problem)
        log.feed(#"{"t":"hello","protocol":9}"#)
        XCTAssertNotNil(log.problem)
    }

    func testTheSessionLineSaysWhatTheAgentDoes() {
        var log = SessionLog()
        log.feed(#"{"t":"session","state":"working","status":"Musing…","agent":"claude","permissions":[{"id":"p","tool":"Bash","summary":"ls"}],"background":[{"call":"c","label":"build"}]}"#)
        XCTAssertEqual(log.state, "working")
        XCTAssertEqual(log.status, "Musing…")
        XCTAssertEqual(log.agent, "claude")
        XCTAssertEqual(log.permissions.map(\.id), ["p"])
        XCTAssertEqual(log.background.map(\.call), ["c"])
    }

    /// A message shows the moment it is sent, and the follower's item of the same id takes its
    /// row; the Transcript's entry then takes it over in place (ADR 0015).
    func testASentMessagesRowIsTheFollowersItemOfTheSameId() {
        var log = SessionLog()
        [hello, item("a", ord: 1, seq: 1, text: "before")].forEach { log.feed($0) }
        log.send("sent.1", text: "hello")
        XCTAssertEqual(log.rows.map(\.id), ["a", "sent.1"])
        XCTAssertEqual(log.rows.last?.entries.first?.state, "sent")
        XCTAssertNotNil(log.rows.last?.entries.first?.at)
        log.feed(item("sent.1", ord: 2, seq: 2, final: false, kind: "user", text: "hello", extra: ["state": "shown"]))
        XCTAssertEqual(log.rows.map(\.id), ["a", "sent.1"])
        XCTAssertEqual(log.rows.last?.entries.first?.state, "shown")
        log.feed(item("sent.1", ord: 2, seq: 3, entry: "u1", kind: "user", text: "hello"))
        XCTAssertEqual(log.rows.map(\.id), ["a", "sent.1"])
        XCTAssertEqual(log.rowKey("u1"), "sent.1")
        XCTAssertFalse(log.isLocal("sent.1"))
        XCTAssertFalse(log.kept(10).isEmpty)
    }

    func testAFailedSendTakesItsRowAway() {
        var log = SessionLog()
        log.send("sent.1", text: "hello")
        XCTAssertEqual(log.kept(10).count, 0, "a message only on the device is not cached")
        XCTAssertEqual(log.drop("sent.1"), "hello")
        XCTAssertTrue(log.rows.isEmpty)
    }

    func testPagingEarlierLeavesASentMessageAlone() {
        var log = SessionLog()
        [hello, item("b", ord: 5, seq: 1, text: "later")].forEach { log.feed($0) }
        log.send("sent.1", text: "hello")
        log.page(item("a", ord: 4, seq: 2, text: "earlier") + "\n" + #"{"t":"page_done","more":false}"#)
        XCTAssertEqual(texts(log), ["earlier", "later", "hello"])
        XCTAssertEqual(log.first?.id, "a")
        XCTAssertTrue(log.isLocal("sent.1"))
        log.feed(#"{"t":"opened","session":"w1:p1","seq":3}"#)
        XCTAssertEqual(texts(log).last, "hello", "a reopened watch keeps it too")
    }

    /// Messages waiting to be taken sit apart from the rows, under the newest one.
    func testWaitingMessagesAreQueuedNotRows() {
        var log = SessionLog()
        log.feed(item("sent.1", ord: 1, seq: 1, final: false, kind: "user", text: "next", extra: ["state": "handed"]))
        log.feed(item("sent.2", ord: 2, seq: 2, final: false, kind: "user", text: "then", extra: ["state": "queued"]))
        log.send("sent.3", text: "later", held: true)
        XCTAssertTrue(log.rows.isEmpty)
        XCTAssertEqual(log.queued.map(\.id), ["sent.1", "sent.2", "sent.3"])
        log.feed(item("sent.1", ord: 3, seq: 3, final: false, kind: "user", text: "next", extra: ["state": "shown"]))
        XCTAssertEqual(log.rows.map(\.id), ["sent.1"])
        XCTAssertEqual(log.queued.map(\.id), ["sent.2", "sent.3"])
    }

    /// A Transcript file's entries, as `follow --file` prints them, go through the same rows.
    func testATranscriptFilesEntriesBuildTheSameRows() {
        var log = SessionLog()
        [#"{"t":"hello","protocol":2,"agent":"claude","transcript":"/t.jsonl"}"#,
         #"{"t":"entry","id":"u1","kind":"user","summary":"hi","text":"hi"}"#,
         #"{"t":"live","status":"Thinking","items":[{"id":"live.1","kind":"text","summary":"Hel","text":"Hel"}]}"#,
         #"{"t":"entry","id":"q1","kind":"question","summary":"Tea?"}"#,
         #"{"t":"entry","id":"t1","kind":"text","summary":"Hello","text":"Hello","replaces":"live.1"}"#,
         #"{"t":"cursor","cursor":"42"}"#].forEach { log.feed($0) }
        XCTAssertEqual(log.rows.map(\.id), ["u1", "live.1", "q1"])
        XCTAssertEqual(log.rowKey("t1"), "live.1")
        XCTAssertEqual(log.cursor, "42")
        XCTAssertEqual(log.agent, "claude")
        XCTAssertTrue(log.loaded)
        XCTAssertTrue(log.earlier)
        log.page(#"{"t":"entry","id":"u0","kind":"user","summary":"before","text":"before"}"# + "\n" + #"{"t":"history","more":false}"#)
        XCTAssertEqual(log.rows.first?.id, "u0")
        XCTAssertFalse(log.earlier)
    }
}
