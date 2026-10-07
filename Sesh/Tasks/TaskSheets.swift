import SwiftUI

/// The sheets the Tasks screens open.
enum TaskSheet: Identifiable {
    case vault
    case task(Vault, parent: String?)
    case folder(Vault, parent: String?)
    case rename(Vault, Record)
    case close(Vault, Record)

    var id: String {
        switch self {
        case .vault: "vault"
        case .task(let vault, let parent): "task:\(vault.id):\(parent ?? "")"
        case .folder(let vault, let parent): "folder:\(vault.id):\(parent ?? "")"
        case .rename(_, let record): "rename:\(record.id)"
        case .close(_, let record): "close:\(record.id)"
        }
    }

    @MainActor @ViewBuilder var view: some View {
        switch self {
        case .vault: AddVaultSheet()
        case .task(let vault, let parent): NewTaskSheet(vault: vault, parent: parent)
        case .folder(let vault, let parent): NameSheet(vault: vault, record: nil, parent: parent)
        case .rename(let vault, let record): NameSheet(vault: vault, record: record, parent: nil)
        case .close(let vault, let task): CloseTaskSheet(vault: vault, task: task)
        }
    }
}

/// A Form in its own navigation bar, with Cancel and one action that may take a while.
private struct SheetForm<Content: View>: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    let title: String
    let action: String
    var role: ButtonRole?
    let enabled: Bool
    let working: Bool
    let perform: () -> Void
    @ViewBuilder let content: () -> Content

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        NavigationStack {
            Form { content() }
                .font(.ui(Metric.label))
                .disabled(working)
                .navigationTitle(title)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(working) }
                    ToolbarItem(placement: .confirmationAction) {
                        if working { ProgressView() } else {
                            Button(action, role: role, action: perform).disabled(!enabled)
                        }
                    }
                }
        }
        .tint(flavour(.mauve))
        .interactiveDismissDisabled(working)
    }
}

/// A Vault by its name on a Host, as the Mac knows that Host: picking the Host that answers
/// to the Mac's ssh alias opens the same Vault on the phone.
struct AddVaultSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @EnvironmentObject private var store: Store
    @State private var name = ""
    @State private var host: UUID?
    @State private var alias = ""

    var body: some View {
        SheetForm(title: "Add Vault", action: "Add", enabled: !Names.slug(name).isEmpty && host != nil && !trimmed.isEmpty,
                  working: false, perform: add) {
            Section {
                LabeledField("Name", "hobby", text: $name)
                Picker("Host", selection: $host) {
                    Text("Choose").tag(UUID?.none)
                    ForEach(store.hosts) { Text($0.title).tag(UUID?.some($0.id)) }
                }
                LabeledField("Name on the Mac", "vps-he", text: $alias)
            } footer: {
                Text("The Vault lives on that Host in ~/.sesh/vaults/\(Names.slug(name).isEmpty ? "NAME" : Names.slug(name)), and this phone keeps a copy. "
                     + "Give the Host the ssh alias the Mac reaches it by, so Tasks started on either open on both.")
            }
        }
        .onChange(of: host) { _, id in
            if let host = store.hosts.first(where: { $0.id == id }) { alias = host.name.isEmpty ? host.address : host.name }
        }
    }

    private var trimmed: String { alias.trimmingCharacters(in: .whitespaces) }

    private func add() {
        library.add(.init(name: Names.slug(name), alias: trimmed, host: host))
        dismiss()
    }
}

struct NameSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ObservedObject var vault: Vault
    let record: Record?
    let parent: String?
    @State private var name = ""

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        SheetForm(title: record == nil ? "New Folder" : "Rename", action: "Save", enabled: !trimmed.isEmpty, working: false, perform: save) {
            TextField(record?.kind == .task ? "What is this Task about, in plain words?" : "Folder name", text: $name)
                .onSubmit(save)
        }
        .onAppear { name = record?.body.title ?? "" }
    }

    private func save() {
        guard !trimmed.isEmpty else { return }
        if var record = record.flatMap({ vault.records[$0.id] }) {
            record.body.title = trimmed
            vault.write(record)
        } else {
            _ = vault.create(.folder, .init(title: trimmed, parent: parent, position: Tree.next(in: parent, of: vault)))
        }
        dismiss()
    }
}

/// A title in plain words, and optionally a new Worktree on the Vault's Host with a branch
/// suggested from it.
struct NewTaskSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let parent: String?
    @State private var title = ""
    @State private var worktree = false
    @State private var repo = ""
    @State private var branch = ""
    @State private var branchEdited = false
    @State private var working = false
    @State private var problem: String?
    /// Kept so that a retry after a failed Worktree does not make a second Task.
    @State private var made: Record?

    private var machine: Machine { vault.machine }
    private var trimmed: String { title.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var recentKey: String { "repo.\(vault.place.alias ?? "")" }

    var body: some View {
        SheetForm(title: "New Task", action: "Create", enabled: !trimmed.isEmpty && (!worktree || (!repo.isEmpty && !branch.isEmpty)),
                  working: working, perform: { Task { await create() } }) {
            Section {
                TextField("What is this Task about, in plain words?", text: $title, axis: .vertical)
            }
            Section {
                Toggle("Work in a new Worktree", isOn: $worktree)
                if worktree {
                    LabeledContent("Host", value: machine.title)
                    LabeledField("Repository", "~/src/project", text: $repo)
                    LabeledField("Branch", "", text: Binding(get: { branch }, set: { branch = $0; branchEdited = true }))
                }
            } footer: {
                if worktree { Text("herdr makes the Worktree from the repository's main checkout, as a Workspace named after the Task.") }
            }
            if let problem {
                Section { Text(problem).font(.system(size: Metric.caption, design: .monospaced)).foregroundStyle(.red) }
            }
        }
        .onAppear { repo = UserDefaults.standard.string(forKey: recentKey) ?? "" }
        .onChange(of: title) { if !branchEdited { branch = TaskActions.branch(for: title, user: machine.host?.user ?? "sesh") } }
    }

    private func create() async {
        working = true
        defer { working = false }
        let task = made ?? vault.create(.task, .init(
            title: trimmed, parent: parent, position: Tree.next(in: parent, of: vault), edited: Int64(Date().timeIntervalSince1970 * 1000)))
        made = task
        guard worktree else { return finish(task) }
        UserDefaults.standard.set(repo, forKey: recentKey)
        let folder = repo.hasPrefix("~/") ? (await machine.run("printf %s \"$HOME\"")).out + repo.dropFirst(1) : repo
        if let failed = await TaskActions.makeWorktree(for: task, repo: folder, branch: branch, on: machine, in: vault) {
            problem = "The Task is made, but its Worktree is not: \(failed)"
            return
        }
        finish(task)
    }

    private func finish(_ task: Record) {
        dismiss()
        library.selection = task.id
    }
}

/// What closing a Task will do, said before it is done; uncommitted work must be given up
/// explicitly.
struct CloseTaskSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @State private var uncommitted: String?
    @State private var discard = false
    @State private var working = false
    @State private var problem: String?

    private var sessions: Int { vault.children(.session, task: task.id).count }

    var body: some View {
        SheetForm(title: "Close Task", action: "Close", role: .destructive,
                  enabled: uncommitted != nil && (uncommitted?.isEmpty != false || discard), working: working,
                  perform: { Task { await close() } }) {
            Section(task.body.title ?? "Task") {
                Text(sessions == 1 ? "Stops its Agent session." : "Stops its \(sessions) Agent sessions.")
                if let path = task.body.path {
                    Text("Removes the Worktree at \(path) on \(TaskActions.machine(task.body.machine).title); the branch \(task.body.branch ?? "") stays.")
                }
                Text("Archives the Task. Its Journal, Documents and Conversations stay searchable.")
            }
            if uncommitted == nil {
                Section { HStack { ProgressView(); Text("Looking for uncommitted work…") } }
            } else if let uncommitted, !uncommitted.isEmpty {
                Section("The Worktree has uncommitted changes") {
                    Text(uncommitted).font(.system(size: Metric.small, design: .monospaced))
                    Toggle("Discard them", isOn: $discard)
                }
            }
            if let problem {
                Section { Text(problem).foregroundStyle(.red) }
            }
        }
        .task { uncommitted = await TaskActions.uncommitted(in: task) }
    }

    private func close() async {
        working = true
        defer { working = false }
        problem = await TaskActions.close(task, in: vault, library: library, discard: discard)
        if problem == nil { dismiss() }
    }
}
