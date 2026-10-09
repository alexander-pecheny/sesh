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

    func testATabMovesToEveryOpenTaskButItsOwn() {
        let own = make(.task, "Own")
        _ = make(.task, "Other")
        _ = make(.task, "Closed", archived: true)
        let document = make(.document, "Notes", task: own.id)
        XCTAssertEqual(Tree.moveTargets(for: document, in: vault).map(\.body.title), ["Other"])
    }

    private func menu(_ tab: TabItem, ended: Bool = false, resuming: Set<String> = []) -> TabMenu {
        TabMenu(tab, in: vault, ended: { _ in ended }, resuming: resuming)
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
        XCTAssertNil(running.path)
        let ended = menu(.session(session.id), ended: true)
        XCTAssertEqual(ended.resume?.id, session.id)
        XCTAssertNil(ended.end)
        let resuming = menu(.session(session.id), resuming: [session.id])
        XCTAssertNil(resuming.end)
        XCTAssertNil(resuming.resume)
    }

    func testATerminalOnlyCloses() {
        let terminal = menu(.terminal("pane"))
        XCTAssertTrue(terminal.closes)
        XCTAssertNil(terminal.record)
        XCTAssertTrue(terminal.targets.isEmpty)
    }

    func testTheAddMenuNamesMachinesOnlyWhenThereAreSeveral() {
        let task = make(.task, "Own")
        let one = NewTabMenu(task: task.id, machines: ["Mac"], in: vault) { _ in false }
        XCTAssertEqual(one.sections.map(\.title), [nil, nil])
        XCTAssertEqual(one.sections[0].items.last, .init(title: "New Terminal", action: .terminal(0)))
        let two = NewTabMenu(task: task.id, machines: ["vps", "Mac"], in: vault) { _ in false }
        XCTAssertEqual(two.sections[0].items.suffix(2).map(\.title), ["New Terminal on vps", "New Terminal on Mac"])
        XCTAssertEqual(two.sections[0].items.prefix(Agent.allCases.count).map(\.action), Agent.allCases.map { .agent($0) })
    }

    func testTheAddMenuReopensSessionsByStateAndDocumentsByTitle() {
        let task = make(.task, "Own")
        let late = make(.session, "Late", task: task.id, position: 2)
        let early = make(.session, "Early", task: task.id, position: 1)
        let gone = make(.session, "Gone", task: task.id, position: 3)
        let b = make(.document, "b", task: task.id)
        let a = make(.document, "a", task: task.id)
        _ = make(.document, "Elsewhere", task: "other")
        let menu = NewTabMenu(task: task.id, machines: ["Mac"], in: vault) { $0.id == gone.id }
        XCTAssertEqual(menu.sections.map(\.title), [nil, nil, "Running Agent sessions", "Ended Agent sessions", "Documents"])
        XCTAssertEqual(menu.sections[2].items.map(\.action), [.open(.session(early.id)), .open(.session(late.id))])
        XCTAssertEqual(menu.sections[3].items.map(\.action), [.open(.session(gone.id))])
        XCTAssertEqual(menu.sections[4].items.map(\.action), [.open(.document(a.id)), .open(.document(b.id))])
    }

    func testANewWorktreesBranchTakesTheNextFreeNumber() {
        XCTAssertEqual(Start.nextBranch(after: "me/fix", taken: ["main", "me/fix"]), "me/fix-2")
        XCTAssertEqual(Start.nextBranch(after: "me/fix", taken: ["me/fix", "me/fix-2", "me/fix-3", "me/fix-5"]), "me/fix-4")
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
