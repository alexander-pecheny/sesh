import XCTest

/// The Mac or a Host as Sesh reaches it, through a login shell, for closing real Tasks.
@MainActor
private final class Shell: Runner {
    let alias: String?
    var title: String { alias ?? "This Mac" }
    private(set) lazy var follower = FollowerLink(self)

    private static let mark = "--sesh-output--"

    init(_ alias: String?) { self.alias = alias }

    func run(_ command: String) async -> Ran {
        let command = command.replacingOccurrences(of: Helper.path, with: "~/.sesh/bin/sesh-transcript")
        let process = Process()
        if let alias {
            process.executableURL = URL(filePath: "/usr/bin/ssh")
            process.arguments = ["-o", "BatchMode=yes", alias, "exec \"$SHELL\" -lic \(quote("echo \(Self.mark); " + command))"]
        } else {
            process.executableURL = URL(filePath: "/bin/zsh")
            process.arguments = ["-lic", "echo \(Self.mark); " + command]
        }
        let out = Pipe(), err = Pipe()
        process.standardOutput = out
        process.standardError = err
        process.standardInput = FileHandle.nullDevice
        try? process.run()
        let output = out.fileHandleForReading.readDataToEndOfFile(), error = err.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        let text = String(decoding: output, as: UTF8.self)
        return Ran(status: process.terminationStatus, out: text.range(of: Self.mark + "\n").map { String(text[$0.upperBound...]) } ?? text,
                   err: String(decoding: error, as: UTF8.self))
    }

    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran { await run(command) }
    func put(_ data: Data, to path: String) async -> Ran { Ran(status: 1, out: "", err: "") }
    func prepare() async -> String? { nil }
}

/// Opens real Tasks with herdr and git on the Mac and on a Host, then closes them as the app does.
/// Runs only when SESH_LIVE_HOSTS names the machines, as in `mac,vps-he`.
@MainActor
final class CloseLiveTests: XCTestCase {
    private var machines: [String?] {
        (ProcessInfo.processInfo.environment["SESH_LIVE_HOSTS"] ?? "").split(separator: ",").map { $0 == "mac" ? nil : String($0) }
    }

    private struct Failed: Error { let message: String }

    private struct Opened {
        let task: Record
        let sessions: [Record]
    }

    /// A Task on a fresh herdr Worktree, with an Agent session in each of its first `agents` panes.
    private func open(on shell: Shell, name: String, agents: Int, submodule: Bool, claude: Bool) async throws -> Opened {
        let repo = "/tmp/sesh-close-live/\(name)"
        let repoMade = await shell.run("""
            mkdir -p \(repo)-sub \(repo) && cd \(repo)-sub && git init -q && git commit -q --allow-empty -m sub \
            && cd \(repo) && git init -q && echo ignored > .gitignore && git add . && git commit -q -m init \
            \(submodule ? "&& git -c protocol.file.allow=always submodule add -q \(repo)-sub vendor/sub && git commit -q -m sub" : "")
            """)
        XCTAssertTrue(repoMade.ok, repoMade.problem)
        let label = "sesh-close-live \(name)"
        let made = await Herdr.worktree(repo, branch: "live/\(name)", label: label, on: shell)
        guard case .success(let opened) = made, let path = opened.path else { throw Failed(message: "herdr made no worktree: \(made)") }
        if submodule {
            let sub = await shell.run("cd \(quote(path)) && git -c protocol.file.allow=always submodule update --init -q")
            XCTAssertTrue(sub.ok, sub.problem)
        }
        _ = await shell.run("touch \(quote(path))/ignored")
        var panes = [opened.pane]
        for n in 1..<max(agents, 1) {
            guard case .success(let tab) = await Herdr.tab(in: opened.workspace, folder: path, label: "agent \(n)", on: shell) else { continue }
            panes.append(tab.pane)
        }
        if claude, let problem = await Herdr.launch(.claude, name: name, pane: panes[0], on: shell) {
            throw Failed(message: "Claude did not start: \(problem)")
        }
        var body = Record.Body(title: label, machine: shell.alias, path: path, branch: "live/\(name)", repo: repo)
        body.workspace = opened.workspace
        let task = Record(id: name, kind: .task, body: body)
        let sessions = panes.prefix(agents).enumerated().map { n, pane in
            Record(id: "\(name)-\(n)", kind: .session, body: .init(task: name, machine: shell.alias, path: path, repo: repo, pane: pane))
        }
        return Opened(task: task, sessions: sessions)
    }

    private func close(_ opened: Opened, on shell: Shell) async -> [Close.Failure] {
        let close = Close(task: opened.task, sessions: opened.sessions, runner: { _ in shell }, copy: { _ in })
        let plan = await close.plan()
        XCTAssertEqual(plan.removed.map(\.path), [opened.task.body.path!])
        XCTAssertEqual(plan.dirty, [])
        return await close.perform(plan, discard: false)
    }

    private func check(agents: Int, submodule: Bool, claude: Bool = false) async throws {
        guard !machines.isEmpty else { throw XCTSkip("SESH_LIVE_HOSTS is not set") }
        for alias in machines {
            let shell = Shell(alias)
            let name = "t\(agents)\(submodule ? "s" : "")\(claude ? "c" : "")-\(Int(Date().timeIntervalSince1970))"
            let opened = try await open(on: shell, name: name, agents: agents, submodule: submodule, claude: claude)
            let failures = await close(opened, on: shell)
            XCTAssertEqual(failures.map(\.message), [], "on \(shell.title)")
            let left = await shell.run("[ -e \(quote(opened.task.body.path!)) ] && echo left")
            XCTAssertEqual(left.out, "", "the Worktree is gone on \(shell.title)")
            let workspaces = await Herdr.workspaces(on: shell)
            XCTAssertNil(workspaces[opened.task.body.workspace!], "the Workspace is closed on \(shell.title)")
        }
    }

    func testAWorktreeWhoseAgentClosedTheWorkspaceIsRemoved() async throws { try await check(agents: 1, submodule: false) }
    func testAWorktreeIsRemovedWithItsStillOpenWorkspace() async throws { try await check(agents: 2, submodule: false) }
    func testAWorktreeWithNoAgentIsRemoved() async throws { try await check(agents: 0, submodule: false) }
    func testAWorktreeWithASubmoduleIsRemoved() async throws { try await check(agents: 1, submodule: true) }
    func testAWorktreeWithASubmoduleIsRemovedWithItsWorkspace() async throws { try await check(agents: 2, submodule: true) }
    func testAWorktreeWhereClaudeRanIsRemoved() async throws { try await check(agents: 2, submodule: true, claude: true) }
}
