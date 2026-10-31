import SwiftUI

/// Picks a folder on a machine, or makes one: its folders listed, repositories marked, and the
/// path above to go back up.
struct FolderPicker: View {
    let machine: Machine
    /// Where it opens; the nearest folder that exists, should this one not.
    let start: String
    let home: String?
    let chosen: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var folder: String?
    @State private var entries: [Entry] = []
    @State private var selected: String?
    @State private var hidden = false
    @State private var naming: String?
    @State private var problem: String?
    @FocusState private var focused: Bool

    struct Entry: Hashable {
        let name: String
        let repo: Bool
    }

    var body: some View {
        #if os(macOS)
        content
            .frame(width: 460, height: 420)
            .toolbar { buttons }
        #else
        NavigationStack {
            content
                .navigationTitle("Choose a folder")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { buttons }
        }
        #endif
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 2) {
                ForEach(Array(crumbs.enumerated()), id: \.offset) { index, crumb in
                    if index > 0 { Image(systemName: "chevron.compact.right").foregroundStyle(.tertiary) }
                    Button(crumb.name) { Task { await open(crumb.path) } }
                        .buttonStyle(.plain)
                        .foregroundStyle(index == crumbs.count - 1 ? .primary : .secondary)
                }
                Spacer()
                Toggle("Hidden", isOn: $hidden).toggleStyle(.button).controlSize(.small)
            }
            .padding(10)
            List(selection: $selected) {
                if let naming {
                    TextField("New folder", text: Binding(get: { naming }, set: { self.naming = $0 }))
                        .focused($focused)
                        .onSubmit { Task { await make() } }
                }
                ForEach(shown, id: \.name) { entry in
                    Label(entry.name, systemImage: entry.repo ? "arrow.triangle.branch" : "folder")
                        .tag(entry.name)
                        .contentShape(Rectangle())
                        .onTapGesture(count: 2) { Task { await open(path(of: entry.name)) } }
                        #if os(iOS)
                        .onTapGesture { Task { await open(path(of: entry.name)) } }
                        #endif
                }
            }
            if let problem {
                Text(problem).font(.caption).foregroundStyle(.red).padding(10)
            }
            HStack {
                Button("New Folder", systemImage: "folder.badge.plus") {
                    naming = ""
                    focused = true
                }
                .disabled(folder == nil)
                Spacer()
                Text(entries.contains(where: \.repo) ? "⎇ marks a repository" : "").font(.caption).foregroundStyle(.secondary)
            }
            .padding(10)
        }
        .task { await open(start) }
    }

    @ToolbarContentBuilder private var buttons: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
        ToolbarItem(placement: .confirmationAction) {
            Button("Choose") {
                guard let folder else { return }
                chosen(Start.abbreviate(selected.map { path(of: $0, in: folder) } ?? folder, home: home))
                dismiss()
            }
            .disabled(folder == nil)
        }
    }

    private var shown: [Entry] { entries.filter { hidden || !$0.name.hasPrefix(".") } }

    /// Each folder from the root down to the one listed, named as the path spells it.
    private var crumbs: [(name: String, path: String)] {
        guard let folder else { return [] }
        let shortened = Start.abbreviate(folder, home: home)
        let parts = shortened.split(separator: "/").map(String.init)
        let rooted = shortened.hasPrefix("/")
        return parts.indices.map { index in
            let path = (rooted ? "/" : "") + parts[...index].joined(separator: "/")
            return (parts[index], path)
        }
        .reduce(into: rooted ? [("/", "/")] : []) { $0.append($1) }
    }

    private func path(of name: String, in folder: String? = nil) -> String {
        let folder = folder ?? self.folder ?? "/"
        return folder.hasSuffix("/") ? folder + name : folder + "/" + name
    }

    /// Lists `path`, climbing to the nearest folder that exists.
    private func open(_ path: String) async {
        let path = path == "~" ? "~/" : path
        let script = """
            p=\(shellPath(path)); while [ ! -d "$p" ] && [ "$p" != / ]; do p=$(dirname "$p"); done
            cd -- "$p" && pwd && for d in * .*; do
                [ -d "$d" ] || continue; case "$d" in .|..) continue;; esac
                if [ -e "$d/.git" ]; then echo "r $d"; else echo "d $d"; fi
            done
            """
        let ran = await machine.run(script)
        guard ran.ok else { return problem = ran.problem }
        var lines = ran.out.split(separator: "\n").map(String.init)
        guard !lines.isEmpty else { return }
        folder = lines.removeFirst()
        entries = lines.map { Entry(name: String($0.dropFirst(2)), repo: $0.hasPrefix("r ")) }
            .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        selected = nil
        problem = nil
    }

    private func make() async {
        let name = (naming ?? "").trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty, !name.contains("/"), let folder else { return naming = nil }
        let ran = await machine.run("mkdir -- \(quote(path(of: name, in: folder)))")
        naming = nil
        guard ran.ok else { return problem = ran.problem }
        await open(folder)
        selected = name
    }
}
