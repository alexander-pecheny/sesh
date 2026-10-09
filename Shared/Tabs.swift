import Foundation

/// One Tab of a Task. Sessions and Documents are records; a Terminal lives only while open.
enum TabItem: Hashable, Identifiable, Codable {
    case journal
    case session(String)
    case document(String)
    /// A shell in a herdr pane, recorded in the Vault so it reopens after a restart.
    case terminal(String)
    /// A Claude subagent's own Transcript, read-only, opened from its parent's Conversation.
    case subagent(session: String, path: String, title: String)

    var id: Self { self }

    /// The Agent session or Document shown, the only Tabs that can move to another Task.
    var record: String? {
        switch self {
        case .session(let id), .document(let id): id
        case .journal, .terminal, .subagent: nil
        }
    }

    var icon: String {
        switch self {
        case .journal: "book"
        case .session: "bubble.left.and.text.bubble.right"
        case .document: "doc.text"
        case .terminal: "terminal"
        case .subagent: "person.2"
        }
    }

    @MainActor
    func title(in vault: some Records) -> String {
        switch self {
        case .journal: "Journal"
        case .session(let id), .document(let id): vault.records[id]?.body.title ?? "Untitled"
        case .terminal: "Terminal"
        case .subagent(_, _, let title): title
        }
    }
}

/// What a Tab's context menu offers, by what the Tab shows.
@MainActor
struct TabMenu {
    /// The Agent session or Document, to rename or move to one of `targets`.
    let record: Record?
    let targets: [Record]
    /// A Document's path, to copy.
    let path: String?
    let closes: Bool
    /// An ended Agent session, to resume.
    let resume: Record?
    /// A running Agent session, to close with its Tab.
    let end: Record?

    init(_ tab: TabItem, in vault: some Records, ended: (Record) -> Bool, resuming: Set<String>) {
        record = tab.record.flatMap { vault.records[$0] }
        targets = record.map { Tree.moveTargets(for: $0, in: vault) } ?? []
        path = record?.kind == .document ? record?.body.path : nil
        closes = tab != .journal
        let session = record?.kind == .session ? record : nil
        resume = session.flatMap { ended($0) ? $0 : nil }
        end = session.flatMap { ended($0) || resuming.contains($0.id) ? nil : $0 }
    }

    var endTitle: String {
        "Close Tab and End \(end?.body.agent.flatMap(Agent.init)?.title ?? "Agent") Session"
    }
}

/// A Task's Add menu: new Tabs first, then every Agent session and Document it has had, to reopen.
@MainActor
struct NewTabMenu {
    enum Action: Hashable {
        case agent(Agent)
        /// A Terminal on the machine at this place in the Task's machines.
        case terminal(Int)
        case document
        case open(TabItem)
    }

    struct Item: Hashable {
        let title: String
        let action: Action
    }

    struct Section: Hashable {
        let title: String?
        let items: [Item]
    }

    let sections: [Section]

    init(task: String, machines: [String], in vault: some Records, ended: (Record) -> Bool) {
        let new = Agent.allCases.map { Item(title: "New \($0.title) session…", action: .agent($0)) }
            + machines.enumerated().map { Item(title: machines.count > 1 ? "New Terminal on \($1)" : "New Terminal", action: .terminal($0)) }
        let sessions = vault.children(.session, task: task).sorted { ($0.body.position ?? 0) < ($1.body.position ?? 0) }
        let reopen = { (title: String, records: [Record]) in
            Section(title: title, items: records.map { record in
                Item(title: record.body.title ?? (record.kind == .session ? "Agent session" : "Untitled"),
                     action: .open(record.kind == .session ? .session(record.id) : .document(record.id)))
            })
        }
        let documents = vault.children(.document, task: task).sorted { ($0.body.title ?? "") < ($1.body.title ?? "") }
        sections = [Section(title: nil, items: new), Section(title: nil, items: [Item(title: "New Document", action: .document)]),
                    reopen("Running Agent sessions", sessions.filter { !ended($0) }),
                    reopen("Ended Agent sessions", sessions.filter(ended)),
                    reopen("Documents", documents)]
            .filter { !$0.items.isEmpty }
    }
}
