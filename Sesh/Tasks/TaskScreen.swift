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
                if let task = vault.records[id] {
                    AddMenu(vault: vault, task: task) { opening in
                        Group {
                            if opening { ProgressView() } else { Image.lucide("plus", size: Metric.title) }
                        }
                        .foregroundStyle(flavour(.mauve))
                        .frame(width: Metric.control, height: Metric.control)
                    }
                    .accessibilityLabel("New Tab")
                }
            }
            .frame(height: 40)
            .background(flavour(.mantle))
            TabContent(vault: vault, task: id, tab: current)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(flavour(.base))
        .navigationTitle(vault.records[id]?.body.title ?? "Task")
        .confirmsClosing(library)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            if let task = vault.records[id] {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        if let worktree = current.worktree(of: task, in: vault) {
                            Section {
                                Label("\(worktree.branch) on \(TaskActions.machine(worktree.machine).title)", systemImage: "arrow.triangle.branch")
                            }
                        }
                        Button("Rename") { sheet = .rename(vault, task) }
                        if task.body.archived == true {
                            Button("Reopen") { Tree.reopen(task, in: vault) }
                        } else {
                            Button("Close Task…", role: .destructive) { sheet = .close(vault, task) }
                        }
                    } label: { Label("Task", systemImage: "ellipsis.circle") }
                }
            }
        }
        .sheet(item: $sheet) { $0.view }
        .confirmsEnding(library)
    }
}

private struct TabChip: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: String
    let tab: TabItem
    let selected: Bool
    @State private var sheet: TaskSheet?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        HStack(spacing: Metric.tiny) {
            Image(systemName: tab.icon).imageScale(.small)
            Text(tab.title(in: vault)).lineLimit(1)
            if case .session(let id) = tab, let session = vault.records[id] { MarkView(mark: library.mark(of: session)) }
            if selected, tab != .journal {
                Button { library.close(tab, in: task) } label: {
                    Image(systemName: "xmark").imageScale(.small).foregroundStyle(flavour(.overlay1))
                        .frame(width: Metric.control * 0.75, height: Metric.control * 0.75)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .padding(.vertical, -Metric.gap)
                .padding(.trailing, -Metric.gap)
                .accessibilityLabel("Close \(tab.title(in: vault))")
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
        .sheet(item: $sheet) { $0.view }
        .contextMenu {
            TabMenuItems(vault: vault, task: task, tab: tab) { sheet = .rename(vault, $0) }
        }
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
        case .document(let document): DocumentScreen(document: library.document(document, in: vault)).id(document)
        case .subagent(let session, let path, let title):
            if let record = vault.records[session] {
                ConversationView(conversation: library.subagentConversation(path: path, of: record), title: title, fresh: false)
                    .id(path)
            }
        case .terminal(let terminal):
            if let record = vault.records[terminal], let pane = record.body.pane {
                PaneTab(key: terminal, pane: pane, machine: TaskActions.machine(record.body.machine)) {
                    library.close(.terminal(terminal), in: task, confirmed: true)
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
        .loadsConversation($conversation, of: id, in: vault, library: library)
    }
}
