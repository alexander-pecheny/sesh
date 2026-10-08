import SwiftUI

/// One Task: a strip of its Tabs, the Journal always first, over the Tab that is open.
struct TaskScreen: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let id: String
    @State private var sheet: TaskSheet?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var tabs: [TabItem] { [.journal] + (library.tabs[id] ?? []) }
    private var current: TabItem { library.current[id] ?? .journal }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 0) {
                ScrollViewReader { scroller in
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: Metric.tiny) {
                            ForEach(tabs) { tab in
                                TabChip(vault: vault, task: id, tab: tab, selected: tab == current).id(tab)
                            }
                            ForEach(library.starting[id] ?? []) { pending in
                                HStack(spacing: Metric.tiny) {
                                    ProgressView().controlSize(.mini)
                                    Text("Starting \(pending.agent.title)…").lineLimit(1)
                                }
                                .font(.ui(Metric.note))
                                .foregroundStyle(flavour(.subtext0))
                                .padding(.horizontal, Metric.gap)
                            }
                        }
                        .padding(.horizontal, Metric.gap)
                    }
                    .onChange(of: current) { _, tab in withAnimation { scroller.scrollTo(tab) } }
                    .onAppear { scroller.scrollTo(current) }
                }
                if let task = vault.records[id] { AddMenu(vault: vault, task: task) }
            }
            .frame(height: 40)
            .background(flavour(.mantle))
            TabContent(vault: vault, task: id, tab: current)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(flavour(.base))
        .navigationTitle(vault.records[id]?.body.title ?? "Task")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let task = vault.records[id] {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        if task.body.path != nil, let branch = task.body.branch {
                            Section("\(branch) on \(TaskActions.machine(task.body.machine).title)") {}
                        }
                        Button("Rename") { sheet = .rename(vault, task) }
                        if task.body.archived == true {
                            Button("Reopen") {
                                var record = task
                                record.body.archived = false
                                vault.write(record)
                            }
                        } else {
                            Button("Close Task…", role: .destructive) { sheet = .close(vault, task) }
                        }
                    } label: { Label("Task", systemImage: "ellipsis.circle") }
                }
            }
        }
        .sheet(item: $sheet) { $0.view }
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
    }
}

private struct TabChip: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: String
    let tab: TabItem
    let selected: Bool

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        HStack(spacing: Metric.tiny) {
            Image(systemName: icon).imageScale(.small)
            Text(title).lineLimit(1)
            if case .session(let id) = tab, let session = vault.records[id] { MarkView(mark: library.mark(of: session)) }
            if selected, tab != .journal {
                Button { library.close(tab, in: task) } label: {
                    Image(systemName: "xmark").imageScale(.small).foregroundStyle(flavour(.overlay1))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Close \(title)")
            }
        }
        .font(.ui(Metric.note))
        .foregroundStyle(selected ? flavour(.text) : flavour(.subtext0))
        .padding(.horizontal, Metric.pad)
        .padding(.vertical, 6)
        .background(selected ? flavour(.surface0) : .clear, in: .capsule)
        .contentShape(.capsule)
        .onTapGesture { library.open(tab, in: task) }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .contextMenu {
            if tab != .journal {
                Button("Close Tab") { library.close(tab, in: task) }
            }
            if case .session(let id) = tab, let session = vault.records[id] {
                if library.ended(session) {
                    Button("Resume Agent session") { Task { await library.resume(session) } }
                } else if !library.resuming.contains(id) {
                    Button("End Agent session") { library.askToEnd(session) }
                }
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

    /// Sessions and Documents can move to another Task; a Terminal ends with its Tab.
    private var recordID: String {
        switch tab {
        case .session(let id), .document(let id): id
        case .journal, .subagent, .terminal: ""
        }
    }

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

/// New Tabs for the Task, and every Agent session and Document it has had, to reopen.
private struct AddMenu: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @State private var problem: String?
    @State private var agent: Agent?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

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
            let documents = vault.children(.document, task: task.id).sorted { ($0.body.title ?? "") < ($1.body.title ?? "") }
            if !documents.isEmpty {
                Section("Documents") {
                    ForEach(documents) { document in
                        Button(document.body.title ?? "Untitled") { library.open(.document(document.id), in: task.id) }
                    }
                }
            }
        } label: {
            Image.lucide("plus", size: Metric.title)
                .foregroundStyle(flavour(.mauve))
                .frame(width: Metric.control, height: Metric.control)
        }
        .accessibilityLabel("New Tab")
        .sheet(item: $agent) { agent in
            StartSessionSheet(vault: vault, task: task, agent: agent) { problem = $0 }
        }
        .alert("Sesh could not start it", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(problem ?? "") }
    }
}

/// What the current Tab shows, apart so the Task screen stays simple to type-check.
private struct TabContent: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: String
    let tab: TabItem

    var body: some View {
        switch tab {
        case .journal: JournalScreen(vault: vault, task: task)
        case .session(let session): SessionScreen(vault: vault, id: session).id(session)
        case .document(let document): DocumentScreen(vault: vault, id: document).id(document)
        case .subagent(let session, let path, let title):
            if let record = vault.records[session] {
                ConversationView(conversation: library.subagentConversation(path: path, of: record), title: title, fresh: false)
                    .id(path)
            }
        case .terminal(let terminal):
            if let record = vault.records[terminal], let pane = record.body.pane {
                PaneTab(key: terminal, pane: pane, machine: TaskActions.machine(record.body.machine)) {
                    library.close(.terminal(terminal), in: task)
                }
                .id(terminal)
            }
        }
    }
}

/// An Agent session of the Task: its Conversation, or its own terminal for menus and pickers
/// the chat cannot show.
private struct SessionScreen: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let id: String
    @State private var conversation: Conversation?

    private var terminal: Bool { library.terminalFace.contains(id) }

    var body: some View {
        Group {
            if terminal, let session = vault.records[id], let pane = session.body.pane {
                PaneTab(key: "session:" + id, pane: pane, machine: TaskActions.machine(session.body.machine)) {
                    library.terminalFace.remove(id)
                }
            } else if let conversation, let session = vault.records[id] {
                ConversationView(conversation: conversation, title: session.body.title ?? "Agent session", fresh: false,
                                 end: conversation.pane == nil ? nil : { library.askToEnd(session) },
                                 resume: conversation.pane == nil && library.ended(session) ? { await library.resume(session) } : nil)
                    .id(ObjectIdentifier(conversation))
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .toolbar {
            if conversation?.pane != nil {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        if terminal { library.terminalFace.remove(id) } else { library.terminalFace.insert(id) }
                    } label: {
                        Label(terminal ? "Chat" : "Terminal", image: terminal ? "bot" : "terminal")
                    }
                }
            }
        }
        // An Agent that exits by itself leaves a bare shell; its Conversation comes from the copy.
        .onChange(of: vault.records[id].map(library.ended) ?? false) { _, ended in
            if ended, conversation?.pane != nil { library.reload(id) }
        }
        .task(id: library.reloads[id, default: 0]) {
            guard let session = vault.records[id] else { return }
            conversation = await library.conversation(for: session)
        }
    }
}
