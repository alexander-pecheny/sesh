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
                ForEach(library.starting[id] ?? []) { pending in
                    HStack(spacing: Metric.tiny) {
                        ProgressView().controlSize(.small)
                        Text("Starting \(pending.agent.title)…").lineLimit(1)
                    }
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, Metric.pad)
                    .help("Starting \(pending.agent.title) on \(pending.machine)")
                }
                if let task = vault.records[id] { AddMenu(vault: vault, task: task) }
                Spacer()
                if let task = vault.records[id], task.body.path != nil, let branch = task.body.branch {
                    Label("\(branch) on \(TaskActions.machine(task.body.machine).title)", systemImage: "arrow.triangle.branch")
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        .help(task.body.path ?? "")
                }
            }
            .padding(.horizontal, Metric.gap)
            .frame(height: 34)
            .background(.bar)
            Divider()
            TabContent(vault: vault, task: id, tab: current)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .navigationTitle(vault.records[id]?.body.title ?? "Task")
        .alert("End this Agent session?", isPresented: Binding(get: { library.ending != nil }, set: { if !$0 { library.ending = nil } })) {
            Button("End", role: .destructive) {
                if let session = library.ending { Task { await library.end(session) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The Agent is still at work and stops at once. Its Conversation stays in the Task, and Resume picks it up again.")
        }
        .alert("Sesh could not resume it", isPresented: Binding(get: { library.resumeProblem != nil }, set: { if !$0 { library.resumeProblem = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(library.resumeProblem ?? "") }
        #if DEBUG
        // `-start claude` starts an Agent session in the chosen Task, for tests with no menus.
        .task {
            if UserDefaults.standard.bool(forKey: "close") {
                for _ in 0..<40 where vault.records[id] == nil || !vault.online { try? await Task.sleep(for: .milliseconds(500)) }
                if let task = vault.records[id], let problem = await TaskActions.close(task, in: vault, library: library, discard: false) {
                    Ghostty.logger.error("close failed: \(problem, privacy: .public)")
                }
            }
            if UserDefaults.standard.string(forKey: "start") == "terminal" {
                for _ in 0..<40 where vault.records[id] == nil || !vault.online { try? await Task.sleep(for: .milliseconds(500)) }
                if let task = vault.records[id], let problem = await library.openTerminal(in: task, on: TaskActions.machines(for: task, in: vault)[0]) {
                    Ghostty.logger.error("terminal failed: \(problem, privacy: .public)")
                }
            }
            guard let agent = UserDefaults.standard.string(forKey: "start").flatMap(Agent.init) else { return }
            for _ in 0..<40 where vault.records[id] == nil || !vault.online { try? await Task.sleep(for: .milliseconds(500)) }
            guard let task = vault.records[id] else { return }
            library.start(agent, for: task, on: TaskActions.machines(for: task, in: vault)[0],
                          repo: UserDefaults.standard.string(forKey: "repo")) { Ghostty.logger.error("start failed: \($0, privacy: .public)") }
        }
        #endif
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
                    .foregroundStyle(session.map(library.ended) == true ? .secondary : .primary)
            }
            .buttonStyle(.plain)
            .keyboardShortcut(shortcut)
            if case .session(let id) = tab, let session = vault.records[id] { MarkView(mark: library.mark(of: session)) }
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
            if let session {
                if library.ended(session) {
                    Button("Resume Agent session") { Task { await library.resume(session) } }
                } else if !library.resuming.contains(session.id) {
                    Button("Close Tab and End \(session.body.agent.flatMap(Agent.init)?.title ?? "Agent") Session") {
                        library.close(tab, in: task)
                        library.askToEnd(session)
                    }
                }
                Divider()
            }
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

    private var session: Record? {
        if case .session(let id) = tab { vault.records[id] } else { nil }
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
        case .terminal(let id): id
        case .journal, .subagent: ""
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
        case .subagent: "person.2"
        }
    }

    private var title: String {
        switch tab {
        case .journal: "Journal"
        case .session(let id), .document(let id): vault.records[id]?.body.title ?? "Untitled"
        case .terminal: "Terminal"
        case .subagent(_, _, let title): title
        }
    }
}

/// New Tabs for the Task, and every Agent session it has had, to reopen.
private struct AddMenu: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @State private var problem: String?
    @State private var agent: Agent?

    var body: some View {
        Menu {
            Section {
                ForEach(Agent.allCases) { agent in
                    Button("New \(agent.title) session…") { self.agent = agent }
                }
                let machines = TaskActions.machines(for: task, in: vault)
                ForEach(machines) { machine in
                    Button(machines.count > 1 ? "New Terminal on \(machine.title)" : "New Terminal") {
                        Task { problem = await library.openTerminal(in: task, on: machine) }
                    }
                }
            }
            Section {
                Button("New Document") {
                    let document = vault.create(.document, .init(title: "Untitled", text: "", task: task.id, edited: 0))
                    library.open(.document(document.id), in: task.id)
                }
            }
            let sessions = vault.children(.session, task: task.id).sorted { ($0.body.position ?? 0) < ($1.body.position ?? 0) }
            ForEach([false, true], id: \.self) { ended in
                let group = sessions.filter { library.ended($0) == ended }
                if !group.isEmpty {
                    Section(ended ? "Ended Agent sessions" : "Running Agent sessions") {
                        ForEach(group) { session in
                            Button(session.body.title ?? "Agent session") { library.open(.session(session.id), in: task.id) }
                        }
                    }
                }
            }
        } label: {
            Image(systemName: "plus")
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .padding(.horizontal, Metric.gap)
        .help("New Tab")
        .sheet(item: $agent) { agent in
            StartSessionSheet(vault: vault, task: task, agent: agent) { problem = $0 }
        }
        .alert("Sesh could not start it", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(problem ?? "") }
    }

}

/// What the current Tab shows, apart so the Task view stays simple to type-check.
private struct TabContent: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: String
    let tab: TabItem

    var body: some View {
        switch tab {
        case .journal: JournalView(vault: vault, task: task)
        case .session(let session): SessionTab(vault: vault, id: session).id(session)
        case .document(let document): DocumentTab(vault: vault, id: document).id(document)
        case .subagent(let session, let path, let title):
            if let record = vault.records[session] {
                ConversationView(conversation: library.subagentConversation(path: path, of: record), title: title, fresh: false)
                    .frame(minWidth: 420, maxWidth: .infinity, minHeight: 300, maxHeight: .infinity)
                    .id(path)
            }
        case .terminal(let terminal):
            if let record = vault.records[terminal], let pane = record.body.pane {
                PaneView(key: terminal, pane: pane, machine: TaskActions.machine(record.body.machine)) {
                    library.close(.terminal(terminal), in: task)
                }
                .id(terminal)
            }
        }
    }
}

/// A herdr pane's own screen, attached; it attaches again after its connection drops.
private struct PaneView: View {
    @EnvironmentObject private var library: Library
    let key: String
    let pane: String
    let machine: Machine
    let gone: () -> Void
    @State private var surface: Ghostty.TerminalSurface?
    @State private var attempt = 0

    var body: some View {
        Group {
            if let surface { Ghostty.Terminal(view: surface).id(ObjectIdentifier(surface)) } else { ProgressView() }
        }
        .task(id: attempt) {
            surface = await library.surface(for: key, pane: pane, on: machine, gone: gone)
        }
        .onReceive(library.objectWillChange) { _ in
            // The cached view went when its attach ended; fetch a new one.
            DispatchQueue.main.async { if surface != nil, !library.attached(key) { surface = nil; attempt += 1 } }
        }
    }
}

/// An Agent session of the Task: its Conversation, or its own terminal for menus and
/// pickers the chat cannot show.
private struct SessionTab: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let id: String
    @State private var conversation: Conversation?

    private var terminal: Bool { library.terminalFace.contains(id) }

    /// A quiet switch: the chosen face is only a shade lighter, as the Tabs above are.
    private func face(_ title: String, selected: Bool, choose: @escaping () -> Void) -> some View {
        Button(action: choose) {
            Text(title)
                .font(.ui(Metric.caption))
                .foregroundStyle(selected ? .primary : .secondary)
                .padding(.horizontal, Metric.pad)
                .padding(.vertical, Metric.tiny)
                .background(selected ? Color.primary.opacity(0.12) : .clear, in: .capsule)
                .contentShape(.capsule)
        }
        .buttonStyle(.plain)
    }

    var body: some View {
        // The Conversation stays laid out under the terminal, so coming back costs no layout.
        ZStack {
            if let conversation, let session = vault.records[id] {
                let ended = conversation.pane == nil && library.ended(session)
                ConversationView(conversation: conversation, title: session.body.title ?? "Agent session", fresh: false, hidden: terminal,
                                 resume: ended ? { await library.resume(session) } : nil)
                    .id(ObjectIdentifier(conversation))
                    .opacity(terminal ? 0 : 1)
                    .allowsHitTesting(!terminal)
            } else {
                ProgressView()
            }
            if terminal, let session = vault.records[id], let pane = session.body.pane {
                PaneView(key: "session:" + id, pane: pane, machine: TaskActions.machine(session.body.machine)) {
                    library.terminalFace.remove(id)
                }
            }
        }
        .frame(minWidth: 420, maxWidth: .infinity, minHeight: 300, maxHeight: .infinity)
        .overlay(alignment: .topTrailing) {
            if conversation?.pane != nil { faces }
        }
        // An Agent that exits by itself leaves a bare shell; its Conversation comes from the copy.
        .onChange(of: vault.records[id].map(library.ended) ?? false) { _, ended in
            if ended, conversation?.pane != nil { library.reload(id) }
        }
        .task(id: library.reloads[id, default: 0]) {
            guard let session = vault.records[id] else { return }
            conversation = await library.conversation(for: session)
            #if DEBUG
            // `-reveal ITEM` scrolls to one entry, for tests with no clicking.
            if let item = UserDefaults.standard.string(forKey: "reveal") { await conversation?.reveal(item) }
            #endif
        }
    }

    private var faces: some View {
        HStack(spacing: 2) {
            face("Chat", selected: !terminal) { library.terminalFace.remove(id) }
            face("Terminal", selected: terminal) { library.terminalFace.insert(id) }
        }
        .padding(2)
        .background(.regularMaterial, in: .capsule)
        .padding(Metric.gap)
        .background {
            Button("") { if terminal { library.terminalFace.remove(id) } else { library.terminalFace.insert(id) } }
                .keyboardShortcut("t", modifiers: [.command, .shift])
                .hidden()
        }
    }
}
