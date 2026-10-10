import XCTest

/// A machine that plays each stream from a script, then holds the last one open until it is
/// cancelled, and answers every command with `answer`.
@MainActor
private final class Scripted: Runner {
    let title = "test"
    var scripts: [(lines: [String], status: Int32)]
    var answer = Ran(status: 0, out: "", err: "")
    private(set) var commands: [String] = []
    private(set) var streams = 0
    private(set) lazy var follower = FollowerLink(self, pause: .milliseconds(1))

    init(_ scripts: [(lines: [String], status: Int32)]) { self.scripts = scripts }

    func run(_ command: String) async -> Ran {
        commands.append(command)
        return answer
    }

    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran {
        commands.append(command)
        streams += 1
        guard !scripts.isEmpty else {
            while !Task.isCancelled { try? await Task.sleep(for: .milliseconds(1)) }
            return Ran(status: -1, out: "", err: "cancelled")
        }
        let script = scripts.removeFirst()
        script.lines.forEach(line)
        return Ran(status: script.status, out: "", err: script.status > 0 ? "it broke" : "")
    }

    func put(_ data: Data, to path: String) async -> Ran { answer }
    func prepare() async -> String? { nil }
}

@MainActor
final class FollowerLinkTests: XCTestCase {
    private let hello = #"{"protocol":5,"t":"hello","version":"test"}"#

    private func item(seq: Int) -> String { #"{"t":"item","session":"w1:p1","id":"a","seq":\#(seq)}"# }

    /// Runs `body` until `machine` has opened `streams` streams, then cancels it.
    private func run(_ machine: Scripted, until streams: Int, _ body: @escaping () async -> Void) async {
        let task = Task { await body() }
        for _ in 0..<2000 where machine.streams < streams { try? await Task.sleep(for: .milliseconds(1)) }
        task.cancel()
        await task.value
        XCTAssertGreaterThanOrEqual(machine.streams, streams)
    }

    func testAWatchComesBackAfterADropFromTheLastItemHeld() async {
        let machine = Scripted([([hello, item(seq: 7)], -1)])
        var seq = 0, lines: [String] = []
        await run(machine, until: 2) {
            await machine.follower.watch("w1:p1", from: { seq }, line: { line in
                lines.append(line)
                if line.contains(#""seq":7"#) { seq = 7 }
            }, problem: { XCTFail($0) })
        }
        XCTAssertEqual(lines, [hello, item(seq: 7)])
        XCTAssertTrue(machine.commands[0].hasSuffix("attach --watch 'w1:p1'"), machine.commands[0])
        XCTAssertTrue(machine.commands[1].hasSuffix("attach --watch 'w1:p1:7'"), machine.commands[1])
    }

    func testTheFollowersErrorsSurfaceAndAStaleHelperIsRefused() async {
        let error = #"{"message":"Session log: locked","op":"watch","t":"error"}"#
        let machine = Scripted([([hello, error], 1), ([#"{"protocol":3,"t":"hello"}"#, item(seq: 1)], -1)])
        var lines: [String] = [], problems: [String] = []
        await run(machine, until: 2) {
            await machine.follower.watch("w1:p1", from: { 0 }, line: { lines.append($0) }, problem: { problems.append($0) })
        }
        XCTAssertEqual(lines, [hello])
        XCTAssertEqual(problems, ["Session log: locked", "it broke", SessionLog.unknown(3)])
        XCTAssertEqual(machine.streams, 2, "a refused helper is not asked again")
    }

    func testTheSessionsSayWhenTheFollowerAnswers() async {
        let machine = Scripted([([hello, #"{"t":"session","session":"w1:p1"}"#], -1)])
        var online: [Bool] = [], lines = 0
        await run(machine, until: 2) {
            await machine.follower.sessions { online.append($0) } line: { _ in lines += 1 }
        }
        XCTAssertEqual(online, [true, false])
        XCTAssertEqual(lines, 2)
    }

    func testAFileFollowEndsWithItsAgentSession() async {
        let machine = Scripted([([], -1), ([#"{"t":"cursor","cursor":"9:/t.jsonl"}"#], 0)])
        var problems: [String] = []
        await machine.follower.follow(file: "/t.jsonl", agent: .claude, from: { machine.streams > 0 ? "9:/t.jsonl" : nil },
                                      line: { _ in }, problem: { problems.append($0) })
        XCTAssertEqual(problems, ["This Agent session has ended."])
        XCTAssertTrue(machine.commands[1].hasSuffix("follow --file '/t.jsonl' --agent claude --since '9:/t.jsonl'"), machine.commands[1])
    }

    func testSendsGoToTheFollowerQuotedAndSayWhyTheyFailed() async {
        let machine = Scripted([])
        let sent = await machine.follower.send("it's done", id: "sent.1", to: "w1:p1")
        XCTAssertNil(sent)
        XCTAssertTrue(machine.commands[0].hasSuffix(#"send 'w1:p1' 'sent.1' 'it'\''s done'"#), machine.commands[0])
        machine.answer = Ran(status: 1, out: "", err: "no menu is open in pane w1:p1")
        let failed = await machine.follower.permit(true, in: "w1:p1")
        XCTAssertEqual(failed, "no menu is open in pane w1:p1")
        XCTAssertTrue(machine.commands[1].hasSuffix("permit 'w1:p1' 'allow'"), machine.commands[1])
    }
}

final class HelperInstallerTests: XCTestCase {
    private let published = Helper.Published(version: "sesh-transcript 2", url: "https://example.com/", sha256: ["linux-x86_64": "abc"])

    @MainActor
    private func plan(_ out: String, status: Int32 = 0, local: Bool = false) -> HelperInstaller.Plan {
        HelperInstaller.plan(Ran(status: status, out: out, err: "no route"), on: "box", version: "sesh-transcript 2", local: local, published: published)
    }

    @MainActor
    func testTheProbeDecidesWhatTheMachineNeeds() {
        XCTAssertEqual(plan("Linux x86_64\nsesh-transcript 2\n"), .ready)
        XCTAssertEqual(plan("Linux x86_64\nsesh-transcript 1\n"), .download("linux-x86_64"))
        XCTAssertEqual(plan("Linux x86_64\n\n"), .download("linux-x86_64"))
        XCTAssertEqual(plan("Linux x86_64\n\n", local: true), .local(platform: "Linux x86_64"))
        XCTAssertEqual(plan("Darwin arm64\n\n"), .failed("Sesh has no helper for Darwin arm64 on box."))
        XCTAssertEqual(plan("missing\nLinux x86_64\n\n"), .failed("box has no herdr, which Tasks run their Agents in."))
        XCTAssertEqual(plan("", status: 255), .failed("no route"))
    }

    func testBuildsOfOneFollowerShareItsCache() {
        XCTAssertEqual(Helper.follower(of: "sesh-transcript 0.1.0+932cb2c6076a8838.p4-s1"), "p4-s1")
        XCTAssertEqual(Helper.follower(of: "sesh-transcript 0.1.0+25b5aeb667961443.p4-s1"), "p4-s1")
        XCTAssertEqual(Helper.follower(of: "sesh-transcript 0.1.0+bd98cab2566d1d05"), "sesh-transcript 0.1.0+bd98cab2566d1d05")
    }
}
