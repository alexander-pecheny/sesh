import SwiftUI

/// One Task: its Tabs in a strip, the Journal always first.
struct TaskView: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let id: String

    private var tabs: [TabItem] { [.journal] + (library.tabs[id] ?? []) }
    private var current: TabItem { library.current[id] ?? .journal }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ForEach(tabs) { tab in
                    TabButton(vault: vault, task: id, tab: tab, selected: tab == current)
                }
                if let task = vault.records[id] { AddMenu(vault: vault, task: task) }
                Spacer()
                if let task = vault.records[id], let branch = task.body.branch {
                    Label("\(branch) on \(TaskActions.machine(task.body.machine).title)", systemImage: "arrow.triangle.branch")
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        .help(task.body.path ?? "")
                }
            }
            .padding(.horizontal, Metric.gap)
            .frame(height: 34)
            .background(.bar)
            Divider()
            Group {
                switch current {
                case .journal: JournalView(vault: vault, task: id)
                case .session(let session): SessionTab(vault: vault, id: session).id(session)
                case .document(let document): DocumentTab(vault: vault, id: document).id(document)
                case .terminal(let terminal):
                    if let surface = library.terminal(terminal) { Ghostty.Terminal(view: surface).id(terminal) }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle(vault.records[id]?.body.title ?? "Task")
    }
}

private struct TabButton: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: String
    let tab: TabItem
    let selected: Bool

    var body: some View {
        HStack(spacing: Metric.tiny) {
            Button { library.open(tab, in: task) } label: {
                Label(title, systemImage: icon).lineLimit(1).contentShape(.rect)
            }
            .buttonStyle(.plain)
            .keyboardShortcut(shortcut)
            if tab != .journal {
                Button { library.close(tab, in: task) } label: { Image(systemName: "xmark").imageScale(.small) }
                    .buttonStyle(.borderless)
            }
        }
        .padding(.horizontal, Metric.pad)
        .padding(.vertical, Metric.tiny)
        .background(selected ? Color.primary.opacity(0.1) : .clear, in: .rect(cornerRadius: 6))
        .draggable(dragged)
        .contextMenu {
            if !recordID.isEmpty {
                Menu("Move to Task") {
                    ForEach(vault.all(.task).filter { $0.id != task && $0.body.archived != true }
                        .sorted { ($0.body.title ?? "") < ($1.body.title ?? "") }) { other in
                        Button(other.body.title ?? "Untitled") { library.move(recordID, to: other.id) }
                    }
                }
            }
        }
    }

    /// Command-1 is always the Journal; the others follow in order up to nine.
    private var shortcut: KeyboardShortcut? {
        let index = [TabItem.journal] + (library.tabs[task] ?? [])
        guard let position = index.firstIndex(of: tab), position < 9 else { return nil }
        return KeyboardShortcut(KeyEquivalent(Character("\(position + 1)")))
    }

    private var recordID: String {
        switch tab {
        case .session(let id), .document(let id): id
        case .journal, .terminal: ""
        }
    }

    /// A session or Document Tab can be dropped on another Task in the sidebar.
    private var dragged: String { recordID }

    private var icon: String {
        switch tab {
        case .journal: "book"
        case .session: "bubble.left.and.text.bubble.right"
        case .document: "doc.text"
        case .terminal: "terminal"
        }
    }

    private var title: String {
        switch tab {
        case .journal: "Journal"
        case .session(let id), .document(let id): vault.records[id]?.body.title ?? "Untitled"
        case .terminal: "Terminal"
        }
    }
}

/// New Tabs for the Task, and every Agent session it has had, to reopen.
private struct AddMenu: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @State private var starting: String?
    @State private var problem: String?

    var body: some View {
        Menu {
            ForEach(TaskActions.machines(for: task, in: vault)) { machine in
                Section(machine.title) {
                    ForEach(Agent.allCases) { agent in
                        Button("New \(agent.title) session") { start(agent, on: machine) }
                    }
                    Button("New Terminal") { library.openTerminal(in: task, on: machine) }
                }
            }
            Section {
                Button("New Document") {
                    let document = vault.create(.document, .init(title: "Untitled", text: "", task: task.id, edited: 0))
                    library.open(.document(document.id), in: task.id)
                }
            }
            let sessions = vault.children(.session, task: task.id).sorted { ($0.body.position ?? 0) < ($1.body.position ?? 0) }
            if !sessions.isEmpty {
                Section("Agent sessions") {
                    ForEach(sessions) { session in
                        Button(session.body.title ?? "Agent session") { library.open(.session(session.id), in: task.id) }
                    }
                }
            }
        } label: {
            if starting != nil { ProgressView().controlSize(.small) } else { Image(systemName: "plus") }
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .padding(.horizontal, Metric.gap)
        .help(starting.map { "Starting \($0)" } ?? "New Tab")
        .disabled(starting != nil)
        .alert("Sesh could not start it", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(problem ?? "") }
    }

    private func start(_ agent: Agent, on machine: Machine) {
        starting = agent.title
        Task {
            defer { starting = nil }
            switch await TaskActions.startSession(agent, for: task, on: machine, in: vault) {
            case .success(let session): library.open(.session(session.id), in: task.id)
            case .failure(let failure): problem = failure.message
            }
        }
    }
}

/// An Agent session of the Task, as its Conversation.
private struct SessionTab: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let id: String
    @State private var conversation: Conversation?

    var body: some View {
        Group {
            if let conversation, let session = vault.records[id] {
                ConversationView(conversation: conversation, title: session.body.title ?? "Agent session", fresh: false)
            } else {
                ProgressView()
            }
        }
        .frame(minWidth: 420, maxWidth: .infinity, minHeight: 300, maxHeight: .infinity)
        .task {
            guard let session = vault.records[id] else { return }
            conversation = await library.conversation(for: session)
        }
    }
}
