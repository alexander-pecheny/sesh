import XCTest

@MainActor
final class MoveTests: XCTestCase {
    private let move = Move(from: "old", to: "new", oldHome: "/home/dev", newHome: "/home/ann")

    private func record(_ id: String, _ kind: Record.Kind, _ body: Record.Body, seq: Int = 1) -> Record {
        Record(id: id, kind: kind, body: body, seq: seq)
    }

    /// One open Task with its own Worktree, a parallel session, a Terminal and a Document on the
    /// old Host; a closed Task there; and a session of the open Task on the Mac.
    private var records: [Record] {
        [
            record("t1", .task, .init(title: "Fix login", machine: "old", path: "/home/dev/.sesh/worktrees/app/ann-fix", branch: "ann/fix",
                                      repo: "/home/dev/app", workspace: "w3")),
            record("t2", .task, .init(title: "Done", machine: "old", path: "/home/dev/.sesh/worktrees/app/ann-done", repo: "/home/dev/app",
                                      archived: true)),
            record("s1", .session, .init(task: "t1", machine: "old", path: "/home/dev/.sesh/worktrees/app/ann-fix", repo: "/home/dev/app",
                                         pane: "p1", name: "fix-claude", agent: "claude",
                                         transcripts: ["/home/dev/.claude-work/projects/a/1.jsonl", "/home/dev/.claude-work/projects/a/2.jsonl"])),
            record("s2", .session, .init(task: "t1", machine: "old", path: "/home/dev/.sesh/worktrees/app/ann-fix-2", repo: "/home/dev/app",
                                         pane: "p2", agent: "codex", transcripts: ["/home/dev/.codex/sessions/2026/11/01/rollout-x.jsonl"])),
            record("s3", .session, .init(task: "t2", machine: "old", path: "/home/dev/.sesh/worktrees/app/ann-done", repo: "/home/dev/app", agent: "pi")),
            record("s4", .session, .init(task: "t1", path: "/Users/ann/app", repo: "/Users/ann/app", agent: "claude")),
            record("x1", .tab, .init(title: "Terminal", task: "t1", machine: "old", path: "/home/dev", pane: "p9", kind: "terminal")),
            record("x2", .tab, .init(task: "t1", kind: "strip", tabs: [.session("s1"), .terminal("x1"), .document("d1")])),
            record("d1", .document, .init(title: "notes.md", task: "t1", machine: "old", path: "/home/dev/app/notes.md")),
        ]
    }

    func testThePlanCarriesOnlyTheOpenTasksWorkOnTheOldHost() {
        let plan = Move.plan(records, from: "old")
        XCTAssertEqual(plan.tasks.map(\.id), ["t1"])
        XCTAssertEqual(plan.sessions.map(\.id), ["s1", "s2"])
        XCTAssertEqual(plan.terminals.map(\.id), ["x1"])
        XCTAssertEqual(plan.repos, ["/home/dev/app"])
        XCTAssertEqual(plan.worktrees.map(\.path), ["/home/dev/.sesh/worktrees/app/ann-fix", "/home/dev/.sesh/worktrees/app/ann-fix-2"])
        XCTAssertEqual(plan.accounts, ["/home/dev/.claude-work"])
        XCTAssertEqual(plan.agents, [.claude, .codex])
    }

    func testPathsUnderTheOldHomeMoveUnderTheNewOneAndOthersStay() {
        XCTAssertEqual(move.rehome("/home/dev"), "/home/ann")
        XCTAssertEqual(move.rehome("/home/dev/app/x"), "/home/ann/app/x")
        XCTAssertEqual(move.rehome("/home/developer/app"), "/home/developer/app")
        XCTAssertEqual(move.rehome("~/app"), "~/app")
        XCTAssertEqual(move.rehome("/srv/app"), "/srv/app")
        XCTAssertEqual(Move.portable("/home/dev/.claude-work"), "~/.claude-work")
        XCTAssertEqual(Move.portable("/Users/ann/.claude"), "~/.claude")
        XCTAssertEqual(Move.portable("~/.claude-work"), "~/.claude-work")
        XCTAssertEqual(Move.portable("/opt/claude"), "/opt/claude")
    }

    func testRewrittenRecordsNameTheNewHostAndSessionsComeBackEndedOnTheirPlacedTranscripts() {
        let placed = ["s1": Move.Spot(path: "/home/ann/.claude-work/projects/b/2.jsonl", cwd: "/home/ann/.sesh/worktrees/app/ann-fix")]
        let changed = Dictionary(uniqueKeysWithValues: move.rewrite(records, placed: placed).map { ($0.id, $0) })
        XCTAssertEqual(Set(changed.keys), ["t1", "s1", "s2", "d1", "x1", "x2"])

        let task = changed["t1"]?.body
        XCTAssertEqual(task?.machine, "new")
        XCTAssertEqual(task?.path, "/home/ann/.sesh/worktrees/app/ann-fix")
        XCTAssertEqual(task?.repo, "/home/ann/app")
        XCTAssertEqual(task?.branch, "ann/fix")
        XCTAssertNil(task?.workspace)

        let session = changed["s1"]?.body
        XCTAssertEqual(session?.machine, "new")
        XCTAssertNil(session?.pane)
        XCTAssertNil(session?.name)
        XCTAssertEqual(session?.transcripts, ["/home/ann/.claude-work/projects/a/1.jsonl", "/home/ann/.claude-work/projects/b/2.jsonl"])
        XCTAssertEqual(changed["s1"]?.seq, 1, "a change builds on the record as the Host holds it")

        XCTAssertEqual(changed["s2"]?.body.path, "/home/ann/.sesh/worktrees/app/ann-fix-2")
        XCTAssertEqual(changed["s2"]?.body.transcripts, ["/home/ann/.codex/sessions/2026/11/01/rollout-x.jsonl"])
        XCTAssertEqual(changed["d1"]?.body.path, "/home/ann/app/notes.md")
        XCTAssertEqual(changed["x1"]?.deleted, true)
        XCTAssertEqual(changed["x2"]?.body.tabs, [.session("s1"), .document("d1")])
    }

    func testAMarkerIsFollowedOnlyToAHostThatReachesTheSameMachine() {
        let marker = Moved.resolved("host new\nhostname 10.0.0.2\nuser ann\nport 22\nidentityfile ~/.ssh/id\n", alias: "new")
        XCTAssertEqual(marker, Moved(alias: "new", hostname: "10.0.0.2", user: "ann", port: 22))
        XCTAssertEqual(marker.place, "new (ann@10.0.0.2)")
        XCTAssertTrue(marker.reached(by: Moved.resolved("hostname 10.0.0.2\nuser ann\nport 22", alias: "new")))
        XCTAssertFalse(marker.reached(by: Moved.resolved("hostname new\nuser ann\nport 22", alias: "new")))
        XCTAssertTrue(Moved(alias: "new").reached(by: marker), "a marker naming no address trusts the alias")

        struct Saved { let id: Int; let name: String; let address: String; let user: String }
        let hosts = [Saved(id: 1, name: "new", address: "10.9.9.9", user: "ann"), Saved(id: 2, name: "Hetzner", address: "10.0.0.2", user: "ann")]
        let pick = { (moved: Moved) in moved.host(among: hosts, name: \.name, address: \.address, user: \.user)?.id }
        XCTAssertEqual(pick(marker), 2, "the address and user count before the name")
        XCTAssertEqual(pick(Moved(alias: "NEW", hostname: "10.1.1.1")), 1)
        XCTAssertNil(pick(Moved(alias: "other", hostname: "10.0.0.2", user: "bob")))
    }

    /// Runs `script` in sh with a home of its own, so the user's git config stays out of it.
    @discardableResult
    private func sh(_ script: String, home: URL) throws -> (status: Int32, out: String, err: String) {
        let process = Process()
        process.executableURL = URL(filePath: "/bin/sh")
        process.arguments = ["-c", script]
        process.currentDirectoryURL = home
        process.environment = ["HOME": home.path, "PATH": "/usr/bin:/bin:/opt/homebrew/bin", "GIT_CONFIG_NOSYSTEM": "1",
                               "GIT_AUTHOR_NAME": "Ann", "GIT_AUTHOR_EMAIL": "ann@example.com",
                               "GIT_COMMITTER_NAME": "Ann", "GIT_COMMITTER_EMAIL": "ann@example.com"]
        let (out, err) = (Pipe(), Pipe())
        (process.standardOutput, process.standardError) = (out, err)
        try process.run()
        let text = { (pipe: Pipe) in String(decoding: pipe.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self) }
        let (printed, said) = (text(out), text(err))
        process.waitUntilExit()
        return (process.terminationStatus, printed, said)
    }

    func testAWorktreeArrivesOnItsBranchWithUnpushedCommitsAndEveryUncommittedChange() throws {
        let home = FileManager.default.temporaryDirectory.appending(path: "sesh-move-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: home) }
        let root = home.path
        try sh("""
            set -e
            git init -q --bare -b main origin.git
            git clone -q origin.git old/app && cd old/app
            echo one > a.txt && git add a.txt && git commit -qm one && git push -q origin main
            git worktree add -q -b ann/fix ../wt && cd ../wt
            echo two > b.txt && git add b.txt && git commit -qm two
            echo changed > a.txt && echo staged > e.txt && git add e.txt
            echo untracked > c.txt && printf '\\000\\001\\002' > d.bin
            git -C ../app worktree add -q -b ann/pushed ../pushed main
            """, home: home)
        let before = try sh("git -C old/wt status --porcelain", home: home).out

        let packed = try sh(Move.pack("\(root)/old/wt", into: "\(root)/out"), home: home)
        XCTAssertEqual(packed.status, 0, packed.err)
        let lines = packed.out.components(separatedBy: "\n")
        XCTAssertEqual(Array(lines[1...2]), ["ann/fix", "bundle"])
        XCTAssertEqual(try sh("git -C old/wt status --porcelain", home: home).out, before, "git's own index is left alone")
        XCTAssertEqual(try sh(Move.pack("\(root)/old/pushed", into: "\(root)/out2"), home: home).out.components(separatedBy: "\n")[2], "none",
                       "a branch every commit of which origin has needs no bundle")

        try sh("git clone -q origin.git new/app", home: home)
        let unpack = { (bundle: String?) in
            try self.sh(Move.unpack(repo: "\(root)/new/app", path: "\(root)/new/wt", head: lines[0], branch: lines[1], bundle: bundle,
                                    patch: "\(root)/out/changes.patch", keep: "\(root)/kept.patch"), home: home)
        }
        let unbundled = try unpack(nil)
        XCTAssertNotEqual(unbundled.status, 0)
        XCTAssertTrue(unbundled.err.contains("came neither from origin nor with the bundle"), unbundled.err)

        let unpacked = try unpack("\(root)/out/commits.bundle")
        XCTAssertEqual(unpacked.status, 0, unpacked.err)
        let arrived = try sh("""
            cd new/wt && git symbolic-ref --short HEAD && git log --format=%s && cat a.txt b.txt c.txt e.txt && xxd -p d.bin
            """, home: home).out
        XCTAssertEqual(arrived.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) },
                       ["ann/fix", "two", "one", "changed", "two", "untracked", "staged", "000102"])
        XCTAssertEqual(try sh("git -C new/app for-each-ref refs/sesh", home: home).out, "", "the bundle's ref is cleaned away")

        try sh("printf 'not a diff\\n' > bad.patch", home: home)
        let detached = try sh(Move.unpack(repo: "\(root)/new/app", path: "\(root)/new/detached", head: lines[0], branch: "", bundle: nil,
                                          patch: "\(root)/bad.patch", keep: "\(root)/kept/1.patch"), home: home)
        XCTAssertTrue(detached.err.contains("wait in \(root)/kept/1.patch"), detached.err)
        XCTAssertTrue(FileManager.default.fileExists(atPath: "\(root)/kept/1.patch"))
        XCTAssertEqual(try sh("git -C new/detached rev-parse HEAD", home: home).out, lines[0] + "\n")

        try sh("git -C new/app worktree remove --force ../wt && git -C new/app branch -qf ann/fix main && git -C new/app commit -q --allow-empty -m other",
               home: home)
        try sh("git -C new/app branch -qf ann/fix HEAD", home: home)
        let diverged = try unpack("\(root)/out/commits.bundle")
        XCTAssertTrue(diverged.err.contains("is there already with other commits"), diverged.err)
    }
}
