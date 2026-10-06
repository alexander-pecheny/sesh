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
        }
        .listStyle(.sidebar)
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
            if case .task = naming.goal { NewTaskSheet(vault: naming.vault, parent: naming.parent) } else { NameSheet(naming: naming) }
        }
    }
}

private struct VaultSection: View {
    @ObservedObject var vault: Vault
    @Binding var naming: Sidebar.Naming?

    var body: some View {
        Section {
            Children(vault: vault, parent: nil, naming: $naming)
        } header: {
            HStack {
                Text(vault.name)
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

/// The folders and Tasks directly inside `parent`.
private struct Children: View {
    @ObservedObject var vault: Vault
    let parent: String?
    @Binding var naming: Sidebar.Naming?

    var body: some View {
        let rows = Tree.children(of: parent, in: vault)
        ForEach(rows) { record in
            if record.kind == .folder {
                FolderRow(vault: vault, folder: record, naming: $naming)
            } else {
                TaskRow(vault: vault, task: record, naming: $naming).tag(record.id)
            }
        }
        .onMove { from, to in Tree.reorder(rows, from: from, to: to, in: vault) }
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
    @ObservedObject var vault: Vault
    let task: Record
    @Binding var naming: Sidebar.Naming?

    var body: some View {
        Text(task.body.title ?? "Untitled")
            .lineLimit(2)
            .draggable(task.id)
            .contextMenu {
                Button("Rename") { naming = .init(goal: .rename(task), vault: vault, parent: nil) }
            }
    }
}

/// Positions are fractions, so a move rewrites one record rather than every sibling.
enum Tree {
    @MainActor
    static func children(of parent: String?, in vault: Vault) -> [Record] {
        (vault.all(.folder) + vault.all(.task).filter { $0.body.archived != true })
            .filter { $0.body.parent == parent }
            .sorted { ($0.body.position ?? 0, $0.id) < ($1.body.position ?? 0, $1.id) }
    }

    @MainActor
    static func next(in parent: String?, of vault: Vault) -> Double {
        (children(of: parent, in: vault).compactMap(\.body.position).max() ?? 0) + 1
    }

    @MainActor
    static func reorder(_ rows: [Record], from: IndexSet, to: Int, in vault: Vault) {
        var order = rows
        order.move(fromOffsets: from, toOffset: to)
        guard let moved = from.first.map({ rows[$0] }), let index = order.firstIndex(of: moved) else { return }
        let before = index > 0 ? order[index - 1].body.position ?? 0 : nil
        let after = index + 1 < order.count ? order[index + 1].body.position ?? 0 : nil
        var record = moved
        record.body.position = switch (before, after) {
        case let (before?, after?): (before + after) / 2
        case let (before?, nil): before + 1
        case let (nil, after?): after - 1
        case (nil, nil): 0
        }
        vault.write(record)
    }

    /// Drops records into a folder, refusing to put a folder inside itself.
    @MainActor
    static func move(_ ids: [String], into parent: String?, in vault: Vault) -> Bool {
        var moved = false
        for id in ids {
            guard var record = vault.records[id], record.kind == .folder || record.kind == .task,
                  record.body.parent != parent, !contains(id, parent, in: vault) else { continue }
            record.body.parent = parent
            record.body.position = next(in: parent, of: vault)
            vault.write(record)
            moved = true
        }
        return moved
    }

    @MainActor
    private static func contains(_ folder: String, _ target: String?, in vault: Vault) -> Bool {
        var current = target
        while let id = current {
            if id == folder { return true }
            current = vault.records[id]?.body.parent
        }
        return false
    }

    @MainActor
    static func deleteFolder(_ folder: Record, in vault: Vault) {
        guard children(of: folder.id, in: vault).isEmpty else { return }
        vault.delete(folder)
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
            let task = vault.create(.task, .init(title: trimmed, parent: naming.parent, position: Tree.next(in: naming.parent, of: vault)))
            library.selection = task.id
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
