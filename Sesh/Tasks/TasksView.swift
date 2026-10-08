import SwiftUI

/// The main screen: every Vault as a section of folders and Tasks, the most recently changed
/// first, with its Archive and Unfiled Agent sessions; or, while searching, the hits.
struct TasksView: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @StateObject private var search = Search()
    @State private var sheet: TaskSheet?
    @State private var settings = false
    /// The order of the Tasks, taken when the screen shows or is pulled to refresh.
    @State private var order: [String: Int] = [:]

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var searching: Bool { !search.query.trimmingCharacters(in: .whitespaces).isEmpty }

    var body: some View {
        Group {
            if searching {
                SearchView(search: search)
            } else if library.vaults.isEmpty {
                empty
            } else {
                List {
                    ForEach(library.vaults) { VaultSection(vault: $0, sheet: $sheet) }
                }
                .refreshable {
                    library.vaults.forEach { $0.retry() }
                    try? await Task.sleep(for: .seconds(1))
                    order = library.order()
                }
                .environment(\.taskOrder, order)
                .onAppear { order = library.order() }
            }
        }
        .scrollContentBackground(.hidden)
        .background(flavour(.base))
        .navigationTitle("Tasks")
        .searchable(text: $search.query, prompt: "Tasks, Journals, Documents, Conversations")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { settings = true } label: { Label("Settings", image: "settings") }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    ForEach(library.vaults) { vault in
                        Button("New Task in \(vault.name)") { sheet = .task(vault, parent: nil) }
                    }
                    Button("Add Vault") { sheet = .vault }
                } label: { Label("Add", image: "plus") }
            }
        }
        .sheet(item: $sheet) { $0.view }
        .sheet(isPresented: $settings) { SettingsView() }
        .onAppear { search.library = library }
    }

    private var empty: some View {
        VStack(spacing: Metric.wide) {
            Image.lucide("list-checks", size: 48).foregroundStyle(flavour(.overlay1))
            Text("No Vaults yet").font(.ui(17)).foregroundStyle(flavour(.text))
            Text("A Vault keeps Tasks on one of your Hosts, in ~/.sesh/vaults.")
                .font(.ui(Metric.label)).foregroundStyle(flavour(.subtext0)).multilineTextAlignment(.center)
            Button("Add a Vault") { sheet = .vault }
                .font(.ui(Metric.body))
                .buttonStyle(.borderedProminent)
        }
        .padding(Metric.wide)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

private struct VaultSection: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    @Binding var sheet: TaskSheet?
    @AppStorage private var open: Bool

    init(vault: Vault, sheet: Binding<TaskSheet?>) {
        self.vault = vault
        _sheet = sheet
        _open = AppStorage(wrappedValue: true, "open." + vault.name)
    }

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Section {
            if open {
                Children(vault: vault, parent: nil, sheet: $sheet)
                ArchiveGroup(vault: vault, sheet: $sheet)
                if let alias = vault.place.alias {
                    UnfiledGroup(items: library.unfiled[alias] ?? [], sheet: $sheet)
                }
            }
        } header: {
            HStack(spacing: Metric.gap) {
                Button {
                    withAnimation(.easeOut(duration: 0.15)) { open.toggle() }
                } label: {
                    HStack(spacing: Metric.tiny) {
                        Image.lucide("chevron-right", size: Metric.small)
                            .rotationEffect(.degrees(open ? 90 : 0))
                        Text(vault.name)
                    }
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(vault.name)
                .accessibilityValue(open ? "expanded" : "collapsed")
                Circle().fill(vault.online ? flavour(.green) : flavour(.overlay0)).frame(width: 6, height: 6)
                    .accessibilityLabel(vault.online ? "online" : "offline")
                if vault.pending > 0 { Text("\(vault.pending) to send").foregroundStyle(flavour(.overlay1)) }
                Spacer()
                Menu {
                    Button("New Task") { sheet = .task(vault, parent: nil) }
                    Button("New Folder") { sheet = .folder(vault, parent: nil) }
                } label: { Image.lucide("plus", size: Metric.title) }
                    .accessibilityLabel("Add to \(vault.name)")
            }
            .font(.ui(Metric.caption))
        } footer: {
            if let problem = vault.problem {
                Text(problem).font(.ui(Metric.small)).foregroundStyle(flavour(.red)).lineLimit(3)
            }
        }
        .listRowBackground(flavour(.mantle))
    }
}

/// The folders and Tasks directly inside `parent`, the most recently changed first, in the
/// order they had when the screen appeared. New ones come first.
private struct Children: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let parent: String?
    @Binding var sheet: TaskSheet?
    @Environment(\.taskOrder) private var order

    var body: some View {
        ForEach(library.children(of: parent, in: vault, order: order)) { record in
            if record.kind == .folder {
                FolderRow(vault: vault, folder: record, sheet: $sheet)
            } else {
                TaskRow(vault: vault, task: record, sheet: $sheet)
            }
        }
    }
}

private struct FolderRow: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var vault: Vault
    let folder: Record
    @Binding var sheet: TaskSheet?
    @State private var open = true

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        DisclosureGroup(isExpanded: $open) {
            Children(vault: vault, parent: folder.id, sheet: $sheet)
        } label: {
            Label { Text(folder.body.title ?? "Folder").foregroundStyle(flavour(.text)) } icon: {
                Image.lucide("folder").foregroundStyle(flavour(.blue))
            }
            .font(.ui(Metric.title))
            .contextMenu {
                Button("New Task") { sheet = .task(vault, parent: folder.id) }
                Button("New Folder") { sheet = .folder(vault, parent: folder.id) }
                Button("Rename") { sheet = .rename(vault, folder) }
                Menu("Move to") { MoveTargets(vault: vault, moving: folder) }
                Divider()
                Button("Delete Folder", role: .destructive) { Tree.deleteFolder(folder, in: vault) }
                    .disabled(!Tree.children(of: folder.id, in: vault).isEmpty)
            }
        }
    }
}

/// The folders a Task or folder can move into, the Vault's top first.
private struct MoveTargets: View {
    @ObservedObject var vault: Vault
    let moving: Record

    var body: some View {
        Button("Top of \(vault.name)") { _ = Tree.move([moving.id], into: nil, in: vault) }
        ForEach(vault.all(.folder).filter { $0.id != moving.id }.sorted { ($0.body.title ?? "") < ($1.body.title ?? "") }) { folder in
            Button(folder.body.title ?? "Folder") { _ = Tree.move([moving.id], into: folder.id, in: vault) }
        }
    }
}

private struct TaskRow: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @Binding var sheet: TaskSheet?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        NavigationLink(value: task.id) { label }
            .contextMenu {
                Button("Rename") { sheet = .rename(vault, task) }
                if task.body.archived != true {
                    Menu("Move to") { MoveTargets(vault: vault, moving: task) }
                }
                Divider()
                if task.body.archived == true {
                    Button("Reopen") { reopen() }
                } else {
                    Button("Close Task…", role: .destructive) { sheet = .close(vault, task) }
                }
            }
            .swipeActions {
                if task.body.archived == true {
                    Button("Reopen") { reopen() }
                } else {
                    Button("Close") { sheet = .close(vault, task) }.tint(flavour(.peach))
                }
            }
    }

    private var label: some View {
        HStack(spacing: Metric.gap) {
            VStack(alignment: .leading, spacing: 2) {
                Text(task.body.title ?? "Untitled").font(.ui(Metric.title)).foregroundStyle(flavour(.text)).lineLimit(2)
                if task.body.path != nil, let branch = task.body.branch {
                    Text(branch).font(.ui(Metric.small)).foregroundStyle(flavour(.subtext0)).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            MarksView(marks: library.marks(ofTask: task.id))
        }
    }

    private func reopen() {
        var record = vault.records[task.id] ?? task
        record.body.archived = false
        vault.write(record)
    }
}

/// Closed Tasks, out of the way but still readable and searchable.
private struct ArchiveGroup: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var vault: Vault
    @Binding var sheet: TaskSheet?
    @State private var open = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        let archived = vault.all(.task).filter { $0.body.archived == true }.sorted { ($0.body.title ?? "") < ($1.body.title ?? "") }
        if !archived.isEmpty {
            DisclosureGroup(isExpanded: $open) {
                ForEach(archived) { TaskRow(vault: vault, task: $0, sheet: $sheet) }
            } label: {
                Label { Text("Archive (\(archived.count))") } icon: { Image(systemName: "archivebox") }
                    .font(.ui(Metric.body)).foregroundStyle(flavour(.subtext0))
            }
        }
    }
}

/// The Agent sessions on the Vault's Host that no Task has adopted yet.
private struct UnfiledGroup: View {
    @Environment(\.colorScheme) private var colorScheme
    let items: [Library.Unfiled]
    @Binding var sheet: TaskSheet?
    @State private var open = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        if !items.isEmpty {
            DisclosureGroup(isExpanded: $open) {
                ForEach(items) { UnfiledRow(item: $0, sheet: $sheet) }
            } label: {
                Label { Text("Unfiled (\(items.count))") } icon: { Image(systemName: "tray") }
                    .font(.ui(Metric.body)).foregroundStyle(flavour(.subtext0))
            }
        }
    }
}

private struct UnfiledRow: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    let item: Library.Unfiled
    @Binding var sheet: TaskSheet?
    @State private var stopping = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Menu {
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
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                Text(item.name).font(.ui(Metric.body)).foregroundStyle(flavour(.text)).lineLimit(1)
                Text("\(item.agent.title) in \(item.cwd)").font(.ui(Metric.small)).foregroundStyle(flavour(.subtext0))
                    .lineLimit(1).truncationMode(.head)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(.rect)
        }
        .confirmationDialog("Stop \(item.agent.title) in \(item.cwd)?", isPresented: $stopping, titleVisibility: .visible) {
            Button("Stop and Close", role: .destructive) { Task { await library.stop(item) } }
        } message: {
            Text("The Agent is interrupted and its herdr pane closed. Nothing of it is kept in a Vault, as it was never adopted.")
        }
    }
}
