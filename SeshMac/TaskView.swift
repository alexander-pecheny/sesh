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
                case .document, .terminal: Text("Not built yet").foregroundStyle(.secondary)
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
            Image(systemName: icon).imageScale(.small)
            Text(title).lineLimit(1)
            if tab != .journal {
                Button { library.close(tab, in: task) } label: { Image(systemName: "xmark").imageScale(.small) }
                    .buttonStyle(.borderless)
            }
        }
        .padding(.horizontal, Metric.pad)
        .padding(.vertical, Metric.tiny)
        .background(selected ? Color.primary.opacity(0.1) : .clear, in: .rect(cornerRadius: 6))
        .contentShape(.rect)
        .onTapGesture { library.open(tab, in: task) }
    }

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

    var body: some View {
        if let session = vault.records[id] {
            ConversationView(conversation: library.conversation(for: session), title: session.body.title ?? "Agent session", fresh: false)
                .frame(minWidth: 420, maxWidth: .infinity, minHeight: 300, maxHeight: .infinity)
        }
    }
}
