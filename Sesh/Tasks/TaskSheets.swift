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
    @State private var addingHost = false

    private var taken: Bool { library.vaults.contains { $0.name == Names.slug(name) } }

    var body: some View {
        SheetForm(title: "Add Vault", action: "Add", enabled: !Names.slug(name).isEmpty && !taken && host != nil && !trimmed.isEmpty,
                  working: false, perform: add) {
            Section {
                LabeledField("Name", "hobby", text: $name)
                if store.hosts.isEmpty {
                    Button("Add a Host first") { addingHost = true }
                } else {
                    Picker("Host", selection: $host) {
                        Text("Choose").tag(UUID?.none)
                        ForEach(store.hosts) { Text($0.title).tag(UUID?.some($0.id)) }
                    }
                }
                LabeledField("Name on the Mac", "vps-he", text: $alias)
            } footer: {
                if taken { Text("There is a Vault named \(Names.slug(name)) already.").foregroundStyle(.red) }
                Text("The Vault lives on that Host in ~/.sesh/vaults/\(Names.slug(name).isEmpty ? "NAME" : Names.slug(name)), and this phone keeps a copy. "
                     + "Give the Host the ssh alias the Mac reaches it by, so Tasks started on either open on both.")
            }
        }
        .sheet(isPresented: $addingHost) { HostFormView(host: Host()) }
        .onChange(of: store.hosts.count) { if host == nil, store.hosts.count == 1 { host = store.hosts.first?.id } }
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
    @FocusState private var focused: Bool

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        SheetForm(title: record == nil ? "New Folder" : "Rename", action: "Save", enabled: !trimmed.isEmpty, working: false, perform: save) {
            TextField(record?.kind == .task ? "What is this Task about, in plain words?" : "Folder name", text: $name)
                .focused($focused)
                .onSubmit(save)
        }
        .onAppear {
            name = record?.body.title ?? ""
            focused = true
        }
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

/// A title in plain words and nothing else; the branch is named for it in the background and
/// the repository asked for when an Agent session starts.
struct NewTaskSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let parent: String?
    @State private var title = ""
    @FocusState private var focused: Bool

    private var trimmed: String { title.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        SheetForm(title: "New Task", action: "Create", enabled: !trimmed.isEmpty, working: false, perform: create) {
            TextField("What is this Task about, in plain words?", text: $title)
                .focused($focused)
                .submitLabel(.done)
                .onSubmit(create)
        }
        .onAppear { focused = true }
    }

    private func create() {
        guard !trimmed.isEmpty else { return }
        dismiss()
        library.newTask(trimmed, in: vault, parent: parent)
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
