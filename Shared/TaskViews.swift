import SwiftUI

/// Opens the New Tab sheet for the Task.
struct NewTabButton<Label: View>: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @ViewBuilder let label: (_ opening: Bool) -> Label
    @State private var problem: String?
    @State private var shown: NewTabSheet.Shown?
    @State private var opening = false

    var body: some View {
        Button { shown = .init() } label: { label(opening) }
            .disabled(opening)
            .sheet(item: $shown) { shown in
                NewTabSheet(vault: vault, task: task, shown: shown, failed: { self.shown = $0 }, terminal: openTerminal)
            }
            .alert("Sesh could not start it", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
                Button("OK", role: .cancel) {}
            } message: { Text(problem ?? "") }
    }

    private func openTerminal(on machine: Machine) {
        opening = true
        Task {
            problem = await library.openTerminal(in: task, on: machine)
            opening = false
        }
    }
}

extension TabItem {
    /// A Scratchpad's Tab names its machine, since no Worktree in the header does.
    @MainActor
    func title(in vault: Vault) -> String {
        switch self {
        case .journal: return "Journal"
        case .session(let id), .terminal(let id):
            guard let record = vault.records[id] else { return "Untitled" }
            let title = record.kind == .session ? record.body.title ?? "Untitled" : "Terminal"
            guard let task = record.body.task, vault.records[task]?.scratchpad == true else { return title }
            return "\(title) on \(TaskActions.machine(record.body.machine).title)"
        case .document(let id): return vault.records[id]?.body.title ?? "Untitled"
        case .subagent(_, _, let title), .found(_, _, _, let title): return title
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
        let menu = TabMenu(tab, in: vault, life: { library.life(of: $0) })
        if let record = menu.record { Button("Rename…") { rename(record) } }
        if let path = menu.path { Button("Copy Path") { Pasteboard.copy(path) } }
        if menu.closes { Button("Close Tab") { library.close(tab, in: task) } }
        if let session = menu.resume { Button("Resume Agent session") { Task { await library.resume(session) } } }
        if let session = menu.end {
            Button(menu.endOnlyTitle) { library.askToEnd(session) }
            Button(menu.endTitle) {
                library.close(tab, in: task)
                library.askToEnd(session)
            }
        }
        if let session = menu.permissions {
            Toggle(menu.end == nil ? "Skip Permissions" : "Skip Permissions (Restarts Agent)", isOn: Binding(
                get: { TaskActions.bypass(of: session, in: vault) },
                set: { bypass in Task { await library.setBypass(bypass, for: session) } }))
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
        task(id: library.reloads[id, default: 0]) {
            guard let session = vault.records[id] else { return }
            conversation.wrappedValue = await library.conversation(for: session)
            #if DEBUG
            // `-reveal ITEM` scrolls to one entry, for tests with no clicking.
            if let item = UserDefaults.standard.string(forKey: "reveal") { await conversation.wrappedValue?.reveal(item) }
            #endif
        }
    }
}
