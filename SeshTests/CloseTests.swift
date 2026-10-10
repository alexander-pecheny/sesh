import XCTest

/// A machine holding a few git worktrees and herdr Workspaces, answering the commands Close runs
/// as git and herdr would.
@MainActor
private final class Box: Runner {
    let title: String
    var linked: Set<String>
    var dirty: [String: String] = [:]
    /// Open Workspaces by id, with the Worktree each opened on.
    var workspaces: [String: String] = [:]
    var broken: Set<String> = []
    var branches: Set<String> = []
    private(set) var commands: [String] = []
    private(set) lazy var follower = FollowerLink(self)

    init(_ title: String, linked: Set<String> = []) {
        self.title = title
        self.linked = linked
    }

    func run(_ command: String) async -> Ran {
        commands.append(command)
        let args = command.components(separatedBy: "'").enumerated().filter { $0.offset % 2 == 1 }.map(\.element)
        let force = command.hasSuffix("--force")
        if command.hasPrefix("[ -f ") { return Ran(status: linked.contains(args[0]) ? 0 : 1, out: "", err: "") }
        if command == "printf %s \"$HOME\"" { return Ran(status: 0, out: "/home/me", err: "") }
        if command.contains(" worktree add ") { linked.insert(args[2]) }
        if command.contains(" for-each-ref ") { return Ran(status: 0, out: branches.joined(separator: "\n"), err: "") }
        if command.hasSuffix("status --porcelain") { return Ran(status: 0, out: dirty[args[0]] ?? "", err: "") }
        if command == "herdr workspace list" {
            let list = workspaces.keys.map { #"{"workspace_id":"\#($0)","label":"Fix it"}"# }.joined(separator: ",")
            return Ran(status: 0, out: #"{"result":{"workspaces":[\#(list)]}}"#, err: "")
        }
        if command.hasPrefix("herdr workspace close") { workspaces[args[0]] = nil }
        if command.hasPrefix("herdr worktree remove") {
            let ran = remove(workspaces[args[0]] ?? "", force: force)
            if ran.ok { workspaces[args[0]] = nil }
            return ran
        }
        if command.contains(" worktree remove ") { return remove(args[1], force: force) }
        if command.contains(" stop ") { workspaces[String(args[0].prefix { $0 != ":" })] = nil }
        return Ran(status: 0, out: "", err: "")
    }

    private func remove(_ path: String, force: Bool) -> Ran {
        if broken.contains(path) { return Ran(status: 1, out: "", err: "fatal: '\(path)' is busy") }
        if dirty[path] != nil, !force { return Ran(status: 1, out: "", err: "fatal: '\(path)' contains modified or untracked files") }
        linked.remove(path)
        return Ran(status: 0, out: "", err: "")
    }

    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran { Ran(status: 0, out: "", err: "") }
    func put(_ data: Data, to path: String) async -> Ran { Ran(status: 0, out: "", err: "") }
    func prepare() async -> String? { nil }
}

@MainActor
final class CloseTests: XCTestCase {
    private let mac = Box("This Mac")
    private let box = Box("box")
    private var copied: [[String]] = []

    private func task(path: String?, repo: String? = "/src/app") -> Record {
        var body = Record.Body(title: "Fix it", path: path, branch: "me/fix-it", repo: repo)
        body.workspace = "w1"
        return Record(id: "t1", kind: .task, body: body)
    }

    private func session(_ id: String, on machine: String? = nil, repo: String, path: String) -> Record {
        Record(id: id, kind: .session, body: .init(task: "t1", machine: machine, path: path, repo: repo, pane: "\(id):p1"))
    }

    private func close(_ task: Record, _ sessions: [Record] = []) -> Close {
        Close(task: task, sessions: sessions, runner: { [mac, box] in $0 == "box" ? box : mac },
              copy: { self.copied.append($0.map(\.id)) })
    }

    private var removals: [String] { (mac.commands + box.commands).filter { $0.contains("worktree remove") } }

    func testARepositorysOwnCheckoutIsLeftAndOnlyItsWorkspaceClosed() async {
        mac.workspaces = ["w1": "/src/app"]
        let close = close(task(path: "/src/app"), [session("s1", repo: "/src/app", path: "/src/app")])
        let plan = await close.plan()
        XCTAssertEqual(plan.worktrees.map(\.linked), [false])
        XCTAssertTrue(plan.removed.isEmpty)
        let failures = await close.perform(plan, discard: false)
        XCTAssertEqual(failures, [])
        XCTAssertEqual(removals, [])
        XCTAssertTrue(mac.commands.contains("herdr workspace close 'w1'"))
        XCTAssertEqual(copied, [["s1"]])
        XCTAssertTrue(mac.commands.contains { $0.hasSuffix(" stop 's1:p1'") }, "the Agent is stopped")
    }

    func testACleanLinkedWorktreeIsRemovedWithItsWorkspace() async {
        mac.linked = ["/src/app-fix"]
        mac.workspaces = ["w1": "/src/app-fix"]
        let close = close(task(path: "/src/app-fix"))
        let plan = await close.plan()
        XCTAssertEqual(plan.removed.map(\.path), ["/src/app-fix"])
        XCTAssertEqual(plan.removed.first?.branch, "me/fix-it")
        XCTAssertEqual(plan.workspace, "w1")
        XCTAssertTrue(plan.dirty.isEmpty)
        let failures = await close.perform(plan, discard: false)
        XCTAssertEqual(failures, [])
        XCTAssertEqual(removals, ["herdr worktree remove --workspace 'w1'"])
        XCTAssertFalse(mac.commands.contains("herdr workspace close 'w1'"))
    }

    func testUncommittedWorkIsFlaggedAndKeptUntilTheUserDiscardsIt() async {
        mac.linked = ["/src/app-fix"]
        mac.dirty = ["/src/app-fix": " M Sources/App.swift\n"]
        let close = close(task(path: "/src/app-fix"))
        let plan = await close.plan()
        XCTAssertEqual(plan.dirty.map(\.uncommitted), ["M Sources/App.swift"])
        let kept = await close.perform(plan, discard: false)
        XCTAssertEqual(kept.map(\.worktree.path), ["/src/app-fix"])
        XCTAssertTrue(mac.linked.contains("/src/app-fix"))
        let discarded = await close.perform(plan, discard: true)
        XCTAssertEqual(discarded, [])
        XCTAssertEqual(removals.last, "git -C '/src/app' worktree remove '/src/app-fix' --force")
    }

    func testADirtyWorktreeInAnotherRepositoryIsInThePlan() async {
        mac.linked = ["/src/app-fix"]
        box.linked = ["/src/lib-fix"]
        box.dirty = ["/src/lib-fix": "?? notes.md"]
        let close = close(task(path: "/src/app-fix"), [session("s2", on: "box", repo: "/src/lib", path: "/src/lib-fix")])
        let plan = await close.plan()
        XCTAssertEqual(plan.removed.map(\.path), ["/src/app-fix", "/src/lib-fix"])
        XCTAssertEqual(plan.dirty.map(\.place), ["box"])
        XCTAssertEqual(plan.dirty.map(\.uncommitted), ["?? notes.md"])
    }

    func testASessionsOwnWorktreeLeavesTheTasksAloneAndIsInThePlan() async {
        let vault = Memory()
        box.linked = ["/src/app-fix"]
        var body = task(path: "/src/app-fix").body
        body.machine = "box"
        let main = Record(id: "t1", kind: .task, body: body)
        vault.write(main)
        box.branches = ["main", "me/fix-it"]
        guard case .success(let place) = await Start.worktree("me/fix-it-2", of: "/src/app", after: "me/fix-it", on: box) else { return XCTFail() }
        XCTAssertEqual(box.commands.last,
                       "git -C '/src/app' worktree add -b 'me/fix-it-2' '/home/me/.herdr/worktrees/app/me-fix-it-2' 'me/fix-it'",
                       "a parallel session carries the Task's work on")
        let session = Start.session(.claude, for: "t1", on: "box", at: place, repo: "/src/app", in: vault)
        XCTAssertEqual(vault.records["t1"], main)
        XCTAssertEqual(session.body.path, "/home/me/.herdr/worktrees/app/me-fix-it-2")
        XCTAssertEqual(session.body.branch, "me/fix-it-2")
        XCTAssertEqual(session.body.machine, "box")
        let plan = await close(main, [session]).plan()
        XCTAssertEqual(plan.removed.map(\.path), ["/src/app-fix", "/home/me/.herdr/worktrees/app/me-fix-it-2"])
        XCTAssertEqual(plan.removed.map(\.branch), ["me/fix-it", "me/fix-it-2"])
    }

    func testAFailedRemovalStillTriesTheNextAndSaysWhich() async {
        mac.linked = ["/src/app-fix", "/src/lib-fix"]
        mac.broken = ["/src/app-fix"]
        let close = close(task(path: "/src/app-fix"), [session("s2", repo: "/src/lib", path: "/src/lib-fix")])
        let failures = await close.perform(await close.plan(), discard: false)
        XCTAssertEqual(failures.map(\.worktree.path), ["/src/app-fix"])
        XCTAssertTrue(failures[0].message.contains("is busy"), failures[0].message)
        XCTAssertEqual(mac.linked, ["/src/app-fix"])
    }

    func testRunningAPlanAgainOnlyRetriesWhatIsLeft() async {
        mac.linked = ["/src/app-fix", "/src/lib-fix"]
        mac.workspaces = ["w1": "/src/app-fix"]
        let close = close(task(path: "/src/app-fix"), [session("s2", repo: "/src/lib", path: "/src/lib-fix")])
        let plan = await close.plan()
        let first = await close.perform(plan, discard: false)
        let second = await close.perform(plan, discard: false)
        XCTAssertEqual(first + second, [])
        XCTAssertEqual(removals, ["herdr worktree remove --workspace 'w1'", "git -C '/src/lib' worktree remove '/src/lib-fix'"])
    }

    func testTheWorktreeIsRemovedWhenStoppingItsAgentClosedTheWorkspace() async {
        mac.linked = ["/src/app-fix"]
        mac.workspaces = ["w1": "/src/app-fix"]
        let close = close(task(path: "/src/app-fix"), [session("w1", repo: "/src/app", path: "/src/app-fix")])
        let failures = await close.perform(await close.plan(), discard: false)
        XCTAssertEqual(failures, [])
        XCTAssertEqual(removals, ["git -C '/src/app' worktree remove '/src/app-fix'"])
    }

    func testANewWorktreeOfARepositoryWithoutTheTasksBranchStartsFromItsCheckout() async {
        box.branches = ["main"]
        _ = await Start.worktree("me/fix-it-2", of: "/src/lib", after: "me/fix-it", on: box)
        XCTAssertEqual(box.commands.last, "git -C '/src/lib' worktree add -b 'me/fix-it-2' '/home/me/.herdr/worktrees/lib/me-fix-it-2'")
    }
}
