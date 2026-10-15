import SwiftUI

/// A folder on a machine, browsed as in Finder: its folders and files in a list, and the file
/// picked shown beside it to read or edit, or on a phone in place of the list.
struct FolderBrowser: View {
    @Environment(\.colorScheme) private var colorScheme
    let vault: Vault
    let root: String
    let machine: Machine
    /// The folder listed, as the machine spells it, ending in a slash.
    @State private var folder: String?
    @State private var entries: [String] = []
    @State private var problem: String?
    @State private var picked: String?
    /// Each file opened, kept so that unsaved edits survive picking another.
    @State private var files: [String: DocumentText] = [:]

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Group {
            #if os(macOS)
            HStack(spacing: 0) {
                list.frame(width: 260)
                Divider()
                detail.frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            #else
            if picked == nil {
                list
            } else {
                VStack(spacing: 0) {
                    Button { picked = nil } label: {
                        Label(folder.map { ($0 as NSString).lastPathComponent } ?? "Folder", systemImage: "chevron.left")
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .padding(.horizontal, Metric.pad)
                    .padding(.vertical, Metric.gap)
                    detail
                }
            }
            #endif
        }
        .background(flavour(.base))
        .task { await open(root) }
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(folder ?? root).font(.ui(Metric.small)).foregroundStyle(flavour(.subtext0))
                .lineLimit(1).truncationMode(.head)
                .padding(.horizontal, Metric.pad)
                .padding(.vertical, Metric.gap)
            if let problem {
                Text(problem).font(.ui(Metric.small)).foregroundStyle(flavour(.red)).padding(.horizontal, Metric.pad)
            }
            List {
                if let folder, folder != "/" {
                    row("..", icon: "arrow.turn.left.up", selected: false) { Task { await open(Self.parent(of: folder)) } }
                }
                ForEach(entries, id: \.self) { name in
                    let path = (folder ?? root) + name
                    row(name, icon: name.hasSuffix("/") ? "folder" : "doc", selected: path == picked) {
                        if name.hasSuffix("/") { Task { await open(path) } } else { pick(path) }
                    }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
        }
    }

    private func row(_ name: String, icon: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Label(name, systemImage: icon).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading).contentShape(.rect)
        }
        .buttonStyle(.plain)
        .font(.ui(Metric.label))
        .foregroundStyle(flavour(.text))
        .listRowBackground(selected ? flavour(.surface0) : Color.clear)
        .listRowSeparator(.hidden)
    }

    @ViewBuilder private var detail: some View {
        if let picked, let file = files[picked] {
            #if os(macOS)
            DocumentTab(document: file).id(picked)
            #else
            DocumentScreen(document: file).id(picked)
            #endif
        } else {
            Text("Pick a file to read or edit it.").font(.ui(Metric.label)).foregroundStyle(flavour(.subtext0))
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func pick(_ path: String) {
        if files[path] == nil { files[path] = DocumentText(vault: vault, path: path, machine: machine) }
        picked = path
    }

    /// Lists `path`, folders first; `pwd` names it as the machine does, so `..` can climb past it.
    private func open(_ path: String) async {
        let ran = await machine.run("cd -- \(shellPath(path)) && pwd && ls -1p")
        guard ran.ok else { return problem = ran.problem }
        var lines = ran.out.split(separator: "\n").map(String.init)
        guard !lines.isEmpty else { return }
        let here = lines.removeFirst()
        folder = here.hasSuffix("/") ? here : here + "/"
        entries = lines.sorted { ($0.hasSuffix("/") ? 0 : 1, $0.lowercased()) < ($1.hasSuffix("/") ? 0 : 1, $1.lowercased()) }
        problem = nil
    }

    static func parent(of folder: String) -> String {
        let up = (String(folder.dropLast()) as NSString).deletingLastPathComponent
        return up.hasSuffix("/") ? up : up + "/"
    }
}
