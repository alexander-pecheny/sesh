import SwiftUI

/// New Tabs for the Task, and every Agent session and Document it has had, to reopen.
struct AddMenu<Label: View>: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @ViewBuilder let label: (_ opening: Bool) -> Label
    @State private var problem: String?
    @State private var agent: Agent?
    @State private var opening = false

    var body: some View {
        let machines = TaskActions.machines(for: task, in: vault)
        Menu {
            let menu = NewTabMenu(task: task.id, machines: machines.map(\.title), in: vault) { library.ended($0) }
            ForEach(menu.sections, id: \.self) { section in
                Section {
                    ForEach(section.items, id: \.self) { item in
                        Button(item.title) { perform(item.action, machines) }
                            .disabled(opening && isTerminal(item.action))
                    }
                } header: {
                    if let title = section.title { Text(title) }
                }
            }
        } label: { label(opening) }
        .sheet(item: $agent) { agent in
            StartSessionSheet(vault: vault, task: task, agent: agent) { problem = $0 }
        }
        .alert("Sesh could not start it", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(problem ?? "") }
    }

    private func isTerminal(_ action: NewTabMenu.Action) -> Bool {
        if case .terminal = action { true } else { false }
    }

    private func perform(_ action: NewTabMenu.Action, _ machines: [Machine]) {
        switch action {
        case .agent(let agent): self.agent = agent
        case .terminal(let index):
            opening = true
            Task {
                problem = await library.openTerminal(in: task, on: machines[index])
                opening = false
            }
        case .document:
            let document = vault.create(.document, .init(title: "Untitled", text: "", task: task.id, edited: 0))
            library.open(.document(document.id), in: task.id)
        case .open(let tab): library.open(tab, in: task.id)
        }
    }
}

/// A Tab's context menu.
struct TabMenuItems: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: String
    let tab: TabItem
    let rename: (Record) -> Void

    var body: some View {
        let menu = TabMenu(tab, in: vault, ended: { library.ended($0) }, resuming: library.resuming)
        if let record = menu.record { Button("Rename…") { rename(record) } }
        if let path = menu.path { Button("Copy Path") { Pasteboard.copy(path) } }
        if menu.closes { Button("Close Tab") { library.close(tab, in: task) } }
        if let session = menu.resume { Button("Resume Agent session") { Task { await library.resume(session) } } }
        if let session = menu.end {
            Button(menu.endTitle) {
                library.close(tab, in: task)
                library.askToEnd(session)
            }
        }
        if let record = menu.record {
            Divider()
            Menu("Move to Task") {
                ForEach(menu.targets) { other in
                    Button(other.body.title ?? "Untitled") { library.move(record.id, to: other.id) }
                }
            }
        }
    }
}

/// An Unfiled Agent session's menu: adopt it into a Task, or stop it.
struct UnfiledMenuItems: View {
    @EnvironmentObject private var library: Library
    let item: Library.Unfiled
    @Binding var stopping: Bool

    var body: some View {
        Menu("Adopt into") {
            ForEach(library.vaults) { vault in
                Section(vault.name) {
                    ForEach(Tree.open(in: vault)) { task in
                        Button(task.body.title ?? "Untitled") { library.adopt(item, into: task.id) }
                    }
                }
            }
        }
        Menu("Adopt into a new Task") {
            ForEach(library.vaults) { vault in
                Button(vault.name) { library.adoptIntoNewTask(item, in: vault) }
            }
        }
        Divider()
        Button("Stop and Close…", role: .destructive) { stopping = true }
    }
}

extension View {
    /// Loads an Agent session's Conversation, again whenever the Library asks. An Agent that
    /// exits by itself leaves a bare shell; its Conversation then comes from the copy.
    func loadsConversation(_ conversation: Binding<Conversation?>, of id: String, in vault: Vault, library: Library) -> some View {
        onChange(of: vault.records[id].map(library.ended) ?? false) { _, ended in
            if ended, conversation.wrappedValue?.pane != nil { library.reload(id) }
        }
        .task(id: library.reloads[id, default: 0]) {
            guard let session = vault.records[id] else { return }
            conversation.wrappedValue = await library.conversation(for: session)
            #if DEBUG
            // `-reveal ITEM` scrolls to one entry, for tests with no clicking.
            if let item = UserDefaults.standard.string(forKey: "reveal") { await conversation.wrappedValue?.reveal(item) }
            #endif
        }
    }
}
