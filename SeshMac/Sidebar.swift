import SwiftUI
import UniformTypeIdentifiers

/// The Vaults, each a tree of folders and Tasks in the order the user put them.
struct Sidebar: View {
    @EnvironmentObject private var library: Library
    @State private var adding = false
    @State private var naming: Naming?

    /// What the name sheet is for.
    struct Naming: Identifiable {
        enum Goal { case task, folder, rename(Record) }
        let id = UUID()
        let goal: Goal
        let vault: Vault
        let parent: String?

        var title: String {
            switch goal {
            case .task: "New Task"
            case .folder: "New Folder"
            case .rename: "Rename"
            }
        }
    }

    var body: some View {
        List(selection: $library.selection) {
            ForEach(library.vaults) { vault in
                VaultSection(vault: vault, naming: $naming)
            }
            UnfiledSection(title: "On this Mac", items: library.unfiled[""] ?? [])
        }
        .listStyle(.sidebar)
        #if DEBUG
        // A hidden window draws no translucent material, so a snapshot needs a solid one.
        .scrollContentBackground(UserDefaults.standard.string(forKey: "snapshot") == nil ? .automatic : .hidden)
        #endif
        .safeAreaInset(edge: .bottom) {
            HStack {
                Button { adding = true } label: { Label("Add Vault", systemImage: "plus") }
                    .buttonStyle(.borderless)
                Spacer()
            }
            .padding(Metric.gap)
        }
        .sheet(isPresented: $adding) { AddVault() }
        .sheet(item: $naming) { naming in
            NameSheet(naming: naming)
        }
    }
}

private struct VaultSection: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    @Binding var naming: Sidebar.Naming?
    @State private var open = true

    var body: some View {
        // The sidebar's own fold shows its chevron only on hover; this one always shows.
        Section {
            if open {
                Children(vault: vault, parent: nil, naming: $naming)
                ArchiveGroup(vault: vault, naming: $naming)
                if let alias = vault.place.alias {
                    UnfiledGroup(items: library.unfiled[alias] ?? [])
                }
            }
        } header: {
            HStack {
                Button {
                    withAnimation(.easeOut(duration: 0.15)) { open.toggle() }
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "chevron.right")
                            .font(.caption2.weight(.semibold))
                            .rotationEffect(.degrees(open ? 90 : 0))
                        Text(vault.name)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                Circle().fill(vault.online ? .green : .secondary).frame(width: 6, height: 6)
                    .help(vault.problem ?? (vault.online ? "Up to date with \(vault.machine.title)" : "Offline"))
                if vault.pending > 0 { Text("\(vault.pending)").font(.caption2).foregroundStyle(.secondary) }
                Spacer()
                Menu {
                    Button("New Task") { naming = .init(goal: .task, vault: vault, parent: nil) }
                    Button("New Folder") { naming = .init(goal: .folder, vault: vault, parent: nil) }
                } label: { Image(systemName: "plus") }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .fixedSize()
            }
            .dropDestination(for: String.self) { ids, _ in move(ids, into: nil) }
        }
    }

    private func move(_ ids: [String], into parent: String?) -> Bool {
        Tree.move(ids, into: parent, in: vault)
    }
}

/// The folders and Tasks directly inside `parent`, the most recently changed first.
private struct Children: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let parent: String?
    @Binding var naming: Sidebar.Naming?

    var body: some View {
        let rows = Tree.children(of: parent, in: vault)
            .map { ($0, library.changed($0, in: vault)) }
            .sorted { $0.1 > $1.1 }
            .map(\.0)
        ForEach(rows) { record in
            if record.kind == .folder {
                FolderRow(vault: vault, folder: record, naming: $naming)
            } else {
                TaskRow(vault: vault, task: record, naming: $naming).tag(record.id)
            }
        }

    }
}

private struct FolderRow: View {
    @ObservedObject var vault: Vault
    let folder: Record
    @Binding var naming: Sidebar.Naming?
    @State private var open = true

    var body: some View {
        DisclosureGroup(isExpanded: $open) {
            Children(vault: vault, parent: folder.id, naming: $naming)
        } label: {
            Label(folder.body.title ?? "Folder", systemImage: "folder")
                .draggable(folder.id)
                .dropDestination(for: String.self) { ids, _ in Tree.move(ids, into: folder.id, in: vault) }
                .contextMenu {
                    Button("New Task") { naming = .init(goal: .task, vault: vault, parent: folder.id) }
                    Button("New Folder") { naming = .init(goal: .folder, vault: vault, parent: folder.id) }
                    Button("Rename") { naming = .init(goal: .rename(folder), vault: vault, parent: nil) }
                    Divider()
                    Button("Delete Folder", role: .destructive) { Tree.deleteFolder(folder, in: vault) }
                        .disabled(!Tree.children(of: folder.id, in: vault).isEmpty)
                }
        }
    }
}

private struct TaskRow: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @Binding var naming: Sidebar.Naming?
    @State private var closing = false

    var body: some View {
        HStack(spacing: Metric.gap) {
            Text(task.body.title ?? "Untitled").lineLimit(2)
            Spacer(minLength: 0)
            MarkView(mark: library.mark(ofTask: task.id))
        }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(.rect)
            .draggable(task.id)
            .dropDestination(for: String.self) { ids, _ in
                for id in ids { library.receive(id, into: task.id) }
                return !ids.isEmpty
            }
            .contextMenu {
                Button("Rename") { naming = .init(goal: .rename(task), vault: vault, parent: nil) }
                Divider()
                if task.body.archived == true {
                    Button("Reopen") {
                        var record = vault.records[task.id] ?? task
                        record.body.archived = false
                        vault.write(record)
                    }
                } else {
                    Button("Close Task…") { closing = true }
                }
            }
            .sheet(isPresented: $closing) { CloseTaskSheet(vault: vault, task: task) }
    }
}

private struct NameSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    let naming: Sidebar.Naming
    @State private var name = ""

    var body: some View {
        Form {
            TextField(naming.title, text: $name, prompt: Text(prompt))
                .textFieldStyle(.roundedBorder)
                .labelsHidden()
                .multilineTextAlignment(.leading)
                .onSubmit(save)
        }
        .padding()
        .frame(width: 380)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            ToolbarItem(placement: .confirmationAction) { Button("Save", action: save).disabled(trimmed.isEmpty) }
        }
        .onAppear { if case .rename(let record) = naming.goal { name = record.body.title ?? "" } }
    }

    private var prompt: String {
        if case .folder = naming.goal { return "Folder name" }
        return "What is this Task about, in plain words?"
    }

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    private func save() {
        guard !trimmed.isEmpty else { return }
        let vault = naming.vault
        switch naming.goal {
        case .task:
            library.newTask(trimmed, in: vault, parent: naming.parent)
        case .folder:
            _ = vault.create(.folder, .init(title: trimmed, parent: naming.parent, position: Tree.next(in: naming.parent, of: vault)))
        case .rename(let record):
            var record = vault.records[record.id] ?? record
            record.body.title = trimmed
            vault.write(record)
        }
        dismiss()
    }
}

private struct AddVault: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @State private var name = ""
    @State private var alias = ""

    var body: some View {
        Form {
            TextField("Name", text: $name, prompt: Text("hobby"))
            TextField("Host", text: $alias, prompt: Text("ssh alias, empty for this Mac"))
            Text("The Vault lives on that Host under ~/.sesh/vaults, and this Mac keeps a copy.")
                .font(.caption).foregroundStyle(.secondary)
        }
        .padding()
        .frame(width: 380)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            ToolbarItem(placement: .confirmationAction) {
                Button("Add") {
                    let host = alias.trimmingCharacters(in: .whitespaces)
                    library.add(.init(name: Names.slug(name), alias: host.isEmpty ? nil : host))
                    dismiss()
                }
                .disabled(Names.slug(name).isEmpty)
            }
        }
    }
}

/// The Agent sessions on the Mac that no Task has adopted, under every Vault.
private struct UnfiledSection: View {
    let title: String
    let items: [Library.Unfiled]

    var body: some View {
        if !items.isEmpty {
            Section(title) { UnfiledGroup(items: items) }
        }
    }
}

private struct UnfiledGroup: View {
    let items: [Library.Unfiled]
    @State private var open = false

    var body: some View {
        if !items.isEmpty {
            DisclosureGroup(isExpanded: $open) {
                ForEach(items) { UnfiledRow(item: $0) }
            } label: {
                Label("Unfiled (\(items.count))", systemImage: "tray").foregroundStyle(.secondary)
                    // A group only folds; it is nothing to open.
                    .selectionDisabled()
            }
        }
    }
}

private struct UnfiledRow: View {
    @EnvironmentObject private var library: Library
    let item: Library.Unfiled
    @State private var stopping = false

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(item.name).lineLimit(1)
            Text("\(item.agent.title) in \(item.cwd)").font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
        }
        .draggable(Library.unfiledPrefix + item.id)
        .help("Drag onto a Task to adopt it")
        .contextMenu {
            Menu("Adopt into") {
                ForEach(library.vaults) { vault in
                    Section(vault.name) {
                        ForEach(vault.all(.task).filter { $0.body.archived != true }.sorted { ($0.body.title ?? "") < ($1.body.title ?? "") }) { task in
                            Button(task.body.title ?? "Untitled") { library.adopt(item, into: task.id) }
                        }
                    }
                }
            }
            Menu("Adopt into a new Task") {
                ForEach(library.vaults) { vault in
                    Button(vault.name) {
                        let task = vault.create(.task, .init(title: item.name, position: Tree.next(in: nil, of: vault)))
                        library.adopt(item, into: task.id)
                    }
                }
            }
            Divider()
            Button("Stop and Close…", role: .destructive) { stopping = true }
        }
        .confirmationDialog("Stop \(item.agent.title) in \(item.cwd)?", isPresented: $stopping) {
            Button("Stop and Close", role: .destructive) { Task { await library.stop(item) } }
        } message: {
            Text("The Agent is interrupted and its herdr pane closed. Nothing of it is kept in a Vault, as it was never adopted.")
        }
    }
}

/// Closed Tasks, out of the way but still readable and searchable.
private struct ArchiveGroup: View {
    @ObservedObject var vault: Vault
    @Binding var naming: Sidebar.Naming?
    @State private var open = false

    var body: some View {
        let archived = vault.all(.task).filter { $0.body.archived == true }.sorted { ($0.body.title ?? "") < ($1.body.title ?? "") }
        if !archived.isEmpty {
            DisclosureGroup(isExpanded: $open) {
                ForEach(archived) { TaskRow(vault: vault, task: $0, naming: $naming).tag($0.id) }
            } label: {
                Label("Archive (\(archived.count))", systemImage: "archivebox").foregroundStyle(.secondary)
                    .selectionDisabled()
            }
        }
    }
}

/// What closing a Task will do, said before it is done; uncommitted work must be given up
/// explicitly.
private struct CloseTaskSheet: View {
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
        VStack(alignment: .leading, spacing: Metric.pad) {
            Text("Close “\(task.body.title ?? "Task")”?").font(.headline)
            VStack(alignment: .leading, spacing: Metric.tiny) {
                Text(sessions == 1 ? "Stops its Agent session." : "Stops its \(sessions) Agent sessions.")
                if let path = task.body.path {
                    Text("Removes the Worktree at \(path) on \(TaskActions.machine(task.body.machine).title); the branch \(task.body.branch ?? "") stays.")
                }
                Text("Archives the Task. Its Journal, Documents and Conversations stay searchable.")
            }
            .font(.callout).foregroundStyle(.secondary)
            if let uncommitted, !uncommitted.isEmpty {
                Text("The Worktree has uncommitted changes:").font(.callout)
                ScrollView { Text(uncommitted).font(.system(.caption, design: .monospaced)).frame(maxWidth: .infinity, alignment: .leading) }
                    .frame(maxHeight: 120)
                Toggle("Discard them", isOn: $discard)
            }
            if let problem { Text(problem).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Button(role: .destructive) { Task { await close() } } label: {
                    if working { ProgressView().controlSize(.small) } else { Text("Close Task") }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(working || uncommitted == nil || (uncommitted?.isEmpty == false && !discard))
            }
        }
        .padding()
        .frame(width: 460)
        .task { uncommitted = await TaskActions.uncommitted(in: task) }
    }

    private func close() async {
        working = true
        defer { working = false }
        problem = await TaskActions.close(task, in: vault, library: library, discard: discard)
        if problem == nil { dismiss() }
    }
}
