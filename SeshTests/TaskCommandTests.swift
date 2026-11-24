import XCTest

/// A Vault's records with no Host behind them.
@MainActor
final class Memory: Records {
    var records: [String: Record] = [:]
    func write(_ record: Record) { records[record.id] = record }
}

@MainActor
final class TaskCommandTests: XCTestCase {
    private let vault = Memory()

    private func make(_ kind: Record.Kind, _ title: String, parent: String? = nil, task: String? = nil, archived: Bool? = nil,
                      position: Double? = nil, agent: Agent? = nil, path: String? = nil) -> Record {
        vault.create(kind, .init(title: title, task: task, parent: parent, position: position, path: path, agent: agent?.rawValue, archived: archived))
    }

    func testANewTaskIsStampedAndGoesLastInItsFolder() {
        let folder = make(.folder, "Work")
        _ = make(.task, "First", parent: folder.id, position: 4)
        let task = Tree.newTask("Second", parent: folder.id, in: vault)
        XCTAssertEqual(task.body.position, 5)
        XCTAssertNotNil(task.body.edited)
        XCTAssertEqual(Tree.children(of: folder.id, in: vault).map(\.body.title), ["First", "Second"])
    }

    func testReopenKeepsTheFolderWhileItIsThere() {
        let folder = make(.folder, "Work")
        let task = make(.task, "Fix it", parent: folder.id, archived: true)
        Tree.reopen(task, in: vault)
        XCTAssertEqual(vault.records[task.id]?.body.archived, false)
        XCTAssertEqual(vault.records[task.id]?.body.parent, folder.id)
    }

    func testReopenBringsATaskWhoseFolderWentBackAtTheTop() {
        let folder = make(.folder, "Work")
        let task = make(.task, "Fix it", parent: folder.id, archived: true)
        vault.delete(folder)
        Tree.reopen(task, in: vault)
        XCTAssertNil(vault.records[task.id]?.body.parent)
        XCTAssertEqual(Tree.children(of: nil, in: vault).map(\.id), [task.id])
    }

    func testReopenKeepsWhatChangedSinceTheTaskWasRead() {
        let stale = make(.task, "Fix it", archived: true)
        var renamed = stale
        renamed.body.title = "Fix the login test"
        vault.write(renamed)
        Tree.reopen(stale, in: vault)
        XCTAssertEqual(vault.records[stale.id]?.body.title, "Fix the login test")
    }

    func testAFolderHoldingAClosedTaskIsNotDeleted() {
        let folder = make(.folder, "Work")
        _ = make(.task, "Done", parent: folder.id, archived: true)
        XCTAssertTrue(Tree.children(of: folder.id, in: vault).isEmpty)
        XCTAssertFalse(Tree.canDelete(folder, in: vault))
        Tree.deleteFolder(folder, in: vault)
        XCTAssertEqual(vault.records[folder.id]?.deleted, false)
    }

    func testAnEmptyFolderIsDeleted() {
        let folder = make(.folder, "Work")
        XCTAssertTrue(Tree.canDelete(folder, in: vault))
        Tree.deleteFolder(folder, in: vault)
        XCTAssertEqual(vault.records[folder.id]?.deleted, true)
    }

    func testOpenAndClosedTasksAreListedByTitle() {
        _ = make(.task, "b")
        _ = make(.task, "a")
        _ = make(.task, "z", archived: true)
        _ = make(.task, "y", archived: true)
        XCTAssertEqual(Tree.open(in: vault).map(\.body.title), ["a", "b"])
        XCTAssertEqual(Tree.archived(in: vault).map(\.body.title), ["y", "z"])
    }

    func testTheScratchpadStaysOutOfTheTree() {
        _ = make(.task, "a")
        let scratchpad = Record(id: "scratchpad-test", kind: .task, body: .init(title: "Scratchpad", kind: "scratchpad"))
        vault.write(scratchpad)
        XCTAssertEqual(Tree.open(in: vault).map(\.body.title), ["a"])
        XCTAssertEqual(Tree.children(of: nil, in: vault).map(\.body.title), ["a"])
    }

    func testATabMovesToEveryOpenTaskButItsOwn() {
        let own = make(.task, "Own")
        _ = make(.task, "Other")
        _ = make(.task, "Closed", archived: true)
        let document = make(.document, "Notes", task: own.id)
        XCTAssertEqual(Tree.moveTargets(for: document, in: vault).map(\.body.title), ["Other"])
    }

    private func menu(_ tab: TabItem, life: Panes.Life = .running(.init(status: "idle", done: 0))) -> TabMenu {
        TabMenu(tab, in: vault, life: { _ in life })
    }

    func testTheJournalOffersNothing() {
        let journal = menu(.journal)
        XCTAssertNil(journal.record)
        XCTAssertFalse(journal.closes)
        XCTAssertNil(journal.resume)
        XCTAssertNil(journal.end)
    }

    func testADocumentCanBeRenamedCopiedAndMoved() {
        let task = make(.task, "Own")
        _ = make(.task, "Other")
        let document = make(.document, "Plan", task: task.id, path: "/tmp/plan.md")
        let tab = menu(.document(document.id))
        XCTAssertEqual(tab.record?.id, document.id)
        XCTAssertEqual(tab.path, "/tmp/plan.md")
        XCTAssertEqual(tab.targets.map(\.body.title), ["Other"])
        XCTAssertTrue(tab.closes)
        XCTAssertNil(tab.end)
    }

    func testARunningSessionEndsAndAnEndedOneResumes() {
        let task = make(.task, "Own")
        let session = make(.session, "Codex", task: task.id, agent: .codex)
        let running = menu(.session(session.id))
        XCTAssertEqual(running.end?.id, session.id)
        XCTAssertNil(running.resume)
        XCTAssertEqual(running.endTitle, "Close Tab and End Codex Session")
        XCTAssertEqual(running.endOnlyTitle, "End Codex Session")
        XCTAssertNil(running.path)
        let ended = menu(.session(session.id), life: .ended)
        XCTAssertEqual(ended.resume?.id, session.id)
        XCTAssertNil(ended.end)
        let resuming = menu(.session(session.id), life: .resuming)
        XCTAssertNil(resuming.end)
        XCTAssertNil(resuming.resume)
    }

    func testATerminalOnlyCloses() {
        let terminal = menu(.terminal("pane"))
        XCTAssertTrue(terminal.closes)
        XCTAssertNil(terminal.record)
        XCTAssertTrue(terminal.targets.isEmpty)
    }

    func testTheReopenMenuListsSessionsByStateAndDocumentsByTitle() {
        let task = make(.task, "Own")
        let late = make(.session, "Late", task: task.id, position: 2)
        let early = make(.session, "Early", task: task.id, position: 1)
        let gone = make(.session, "Gone", task: task.id, position: 3)
        let b = make(.document, "b", task: task.id)
        let a = make(.document, "a", task: task.id)
        _ = make(.document, "Elsewhere", task: "other")
        let menu = ReopenMenu(task: task.id, in: vault) { $0.id == gone.id }
        XCTAssertEqual(menu.sections.map(\.title), ["Running Agent sessions", "Ended Agent sessions", "Documents"])
        XCTAssertEqual(menu.sections[0].items.map(\.tab), [.session(early.id), .session(late.id)])
        XCTAssertEqual(menu.sections[1].items.map(\.tab), [.session(gone.id)])
        XCTAssertEqual(menu.sections[2].items.map(\.tab), [.document(a.id), .document(b.id)])
    }

    func testASessionOnAMachineSeshCannotReachIsNeitherRunningNorEnded() {
        let task = make(.task, "Own")
        let away = make(.session, "Away", task: task.id)
        let menu = ReopenMenu(task: task.id, in: vault, ended: { _ in false }, reachable: { $0.id != away.id })
        XCTAssertEqual(menu.sections.map(\.title), ["On machines Sesh cannot reach"])
        XCTAssertNil(TabMenu(.session(away.id), in: vault, life: { _ in .unknown }).end)
    }

    func testClaudeStartsWithRemoteControlUnlessTheTaskSaysNo() {
        XCTAssertEqual(Agent.claude.args("fix").prefix(3), ["--remote-control", "fix", "--dangerously-skip-permissions"])
        XCTAssertEqual(Agent.claude.args("fix", remote: false).prefix(1), ["--dangerously-skip-permissions"])
    }

    func testANewClaudeGetsItsSessionIdFromSesh() {
        let args = Agent.claude.args("fix", remote: false)
        XCTAssertEqual(args.dropLast(), ["--dangerously-skip-permissions", "--session-id"])
        XCTAssertNotNil(UUID(uuidString: args.last ?? ""))
        XCTAssertEqual(Agent.claude.args("fix", resuming: "~/.claude/projects/-w/abc.jsonl", remote: false).suffix(2), ["--resume", "abc"])
    }

    func testAnAgentAsksPermissionOnlyWhenTheSessionSaysSo() {
        XCTAssertFalse(Agent.claude.args("fix", remote: false, bypass: false).contains("--dangerously-skip-permissions"))
        XCTAssertEqual(Agent.codex.args("fix", bypass: false), [])
        XCTAssertEqual(Agent.codex.args("fix"), ["--dangerously-bypass-approvals-and-sandbox"])
        let task = make(.task, "Own")
        let session = make(.session, "Claude", task: task.id, agent: .claude)
        XCTAssertEqual(menu(.session(session.id)).permissions?.id, session.id)
        XCTAssertNil(menu(.session(session.id), life: .resuming).permissions)
    }

    func testEachAgentResumesTheWayItsFileSays() {
        let id = "0199a213-81c0-7800-8aa1-bbab2a035a53"
        XCTAssertEqual(Agent.codex.args("fix", resuming: "/s/2026/11/02/rollout-2026-11-02T10-00-00-\(id).jsonl", bypass: false), ["resume", id])
        XCTAssertEqual(Agent.pi.args("fix", resuming: "/s/x.jsonl"), ["--session", "/s/x.jsonl"])
        XCTAssertEqual(Agent(rawValue: "pi"), .pi)
        XCTAssertNil(Agent(rawValue: "aider"))
    }

    func testANewWorktreesBranchTakesTheNextFreeNumber() {
        XCTAssertEqual(Start.nextBranch(after: "me/fix", taken: ["main", "me/fix"]), "me/fix-2")
        XCTAssertEqual(Start.nextBranch(after: "me/fix", taken: ["me/fix", "me/fix-2", "me/fix-3", "me/fix-5"]), "me/fix-4")
        XCTAssertEqual(Names.folder("pecheny/test-vs-other-harnesses-3"), "pecheny-test-vs-other-harnesses-3",
                       "a long branch keeps its number in its Worktree's folder")
    }

    func testTheHeaderNamesTheWorktreeOfTheTabShown() {
        let task = vault.create(.task, .init(title: "Own", machine: "vps", path: "/w/own", branch: "me/own", repo: "/src/app"))
        let parallel = vault.create(.session, .init(task: task.id, path: "/w/own-2", branch: "me/own-2", repo: "/src/app"))
        let old = vault.create(.session, .init(task: task.id, machine: "vps", path: "/w/own", repo: "/src/app"))
        let home = vault.create(.tab, .init(task: task.id, machine: "vps", path: "/home/me", kind: "terminal"))
        let inMain = vault.create(.tab, .init(task: task.id, machine: "vps", path: "/w/own", kind: "terminal"))
        let document = make(.document, "Plan", task: task.id)
        let main = TabItem.Worktree(branch: "me/own", machine: "vps", path: "/w/own")
        XCTAssertEqual(TabItem.journal.worktree(of: task, in: vault), main)
        XCTAssertEqual(TabItem.document(document.id).worktree(of: task, in: vault), main)
        XCTAssertEqual(TabItem.session(parallel.id).worktree(of: task, in: vault), .init(branch: "me/own-2", machine: nil, path: "/w/own-2"))
        XCTAssertEqual(TabItem.subagent(session: parallel.id, path: "/t.jsonl", title: "Explore").worktree(of: task, in: vault)?.branch, "me/own-2")
        XCTAssertEqual(TabItem.session(old.id).worktree(of: task, in: vault), main)
        XCTAssertEqual(TabItem.terminal(inMain.id).worktree(of: task, in: vault), main)
        XCTAssertNil(TabItem.terminal(home.id).worktree(of: task, in: vault))
    }

    func testANewSessionTakesAfterItsTaskThenItsFolderThenAnywhere() {
        let work = make(.folder, "Work")
        let inner = make(.folder, "Inner", parent: work.id)
        let task = make(.task, "New", parent: work.id)
        let sibling = make(.task, "Sibling", parent: inner.id)
        let elsewhere = make(.task, "Elsewhere")
        let newest = make(.session, "newest", task: elsewhere.id, position: 30)
        let folder = make(.session, "folder", task: sibling.id, position: 20)
        let own = make(.session, "own", task: task.id, position: 10)
        let order = Tree.nearest([newest, folder, own], to: task, in: vault).map(\.id)
        XCTAssertEqual(order, [own.id, folder.id, newest.id])
        XCTAssertEqual(Tree.nearest([newest, folder], to: sibling, in: vault).map(\.id), [folder.id, newest.id])
        let otherVault = vault.create(.session, .init(title: "other vault", task: "elsewhere-entirely", position: 40))
        XCTAssertEqual(Tree.nearest([otherVault, newest], to: task, in: vault).map(\.id), [newest.id, otherVault.id])
    }

    func testANewTabTakesAfterTheNearestSessionOrTerminal() {
        let task = make(.task, "New")
        let other = make(.task, "Other")
        XCTAssertTrue(NewTab.last(of: task, among: [], in: vault) == (.session, .claude))
        let codex = make(.session, "Codex", task: other.id, position: 30, agent: .codex)
        let terminal = vault.create(.tab, .init(title: "Terminal", task: task.id, position: 10, kind: "terminal"))
        XCTAssertTrue(NewTab.last(of: task, among: [codex, terminal], in: vault) == (.terminal, .codex))
        let pi = make(.session, "pi", task: task.id, position: 20, agent: .pi)
        XCTAssertTrue(NewTab.last(of: task, among: [codex, terminal, pi], in: vault) == (.session, .pi))
    }

    func testTheStripKeepsTheTabsEveryDeviceShowsAndOnlyChanges() {
        let task = make(.task, "Fix it")
        let subagent = TabItem.subagent(session: "s", path: "/t.jsonl", title: "Explore")
        Strip.keep([], of: task.id, in: vault)
        XCTAssertNil(vault.records[Strip.id(of: task.id)])
        Strip.keep([.session("s"), subagent, .terminal("t")], of: task.id, in: vault)
        XCTAssertEqual(Strip.tabs(of: task.id, in: vault), [.session("s"), .terminal("t")])
        XCTAssertEqual(Strip.merge([.terminal("t")], into: [.session("s"), subagent]), [.terminal("t"), subagent])
        Strip.keep([], of: task.id, in: vault)
        XCTAssertEqual(Strip.tabs(of: task.id, in: vault), [])
    }

    func testThePhoneLeavesTheMacsTerminalsAndFilesOnItsStrip() {
        let mac = vault.create(.tab, .init(path: "/Users/me", kind: "terminal"))
        let vps = vault.create(.tab, .init(machine: "vps", path: "/home/me", kind: "terminal"))
        let file = make(.document, "Notes", path: "/Users/me/notes.md")
        let note = make(.document, "Plan")
        XCTAssertEqual([TabItem.terminal(mac.id), .terminal(vps.id), .document(file.id), .document(note.id), .session("s")]
            .map { $0.onMac(in: vault) }, [true, false, true, false, false])
    }

    func testRecentRepositoriesShowHomeAsATildeAndEachOnce() {
        let repos = ["/Users/me/src/app", "~/src/app", "/opt/lib", "/Users/me", "/Users/meg/x"]
        XCTAssertEqual(Start.recent(repos, home: "/Users/me"), ["~/src/app", "/opt/lib", "~", "/Users/meg/x"])
        XCTAssertEqual(Start.recent(repos, home: nil), repos)
    }

    func testASecondAgentOfTheTaskTakesTheNextFreeName() {
        XCTAssertEqual(Names.free("fix-claude", taken: ["other"]), "fix-claude")
        XCTAssertEqual(Names.free("fix-claude", taken: ["fix-claude", "fix-claude-2"]), "fix-claude-3")
        let long = String(repeating: "a", count: 32)
        XCTAssertEqual(Names.free(long, taken: [long]), String(repeating: "a", count: 30) + "-2")
    }
}
