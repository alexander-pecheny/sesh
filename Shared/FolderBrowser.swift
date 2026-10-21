import SwiftUI

/// A file's or folder's Document, browsed as in Finder: the folder's folders and files in a
/// list, and the file picked shown beside it to read or edit, or on a phone in place of the list.
struct FolderBrowser: View {
    @Environment(\.colorScheme) private var colorScheme
    let document: DocumentText
    let vault: Vault
    let root: String
    let machine: Machine
    /// The folder listed, as the machine spells it, ending in a slash.
    @State private var folder: String?
    @State private var entries: [String] = []
    @State private var problem: String?
    /// The file or folder being brought to the Mac's Downloads.
    @State private var downloading: String?
    @State private var received = 0
    /// The file's size; a folder is packed as it travels, so its size is not known ahead.
    @State private var expected: Int?
    #if os(iOS)
    /// A downloaded file waiting in the share sheet, to save to Files or send on.
    @State private var shared: URL?
    #endif
    @State private var picked: String?
    /// Each file opened, kept so that unsaved edits survive picking another.
    @State private var files: [String: DocumentText] = [:]

    init(document: DocumentText) {
        self.document = document
        vault = document.vault
        machine = document.machine
        let path = document.path ?? "/"
        root = document.folder ? path : (path as NSString).deletingLastPathComponent + "/"
        if let browsed = document.browsed {
            _folder = State(initialValue: browsed.folder)
            _picked = State(initialValue: browsed.picked)
            _files = State(initialValue: browsed.files)
        } else if !document.folder {
            _picked = State(initialValue: path)
            _files = State(initialValue: [path: document])
        }
    }

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
        .task { await open(folder ?? root) }
        .onDisappear { document.browsed = (folder ?? root, picked, files) }
        #if os(iOS)
        .sheet(isPresented: Binding(get: { shared != nil }, set: { if !$0 { shared = nil } })) {
            if let shared { ShareSheet(url: shared) }
        }
        #endif
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(folder ?? root).font(.ui(Metric.small)).foregroundStyle(flavour(.subtext0))
                .lineLimit(1).truncationMode(.head)
                .padding(.horizontal, Metric.pad)
                .padding(.vertical, Metric.gap)
                .contextMenu {
                    let here = folder ?? root
                    if here != "/" {
                        Button("Download as .tar.gz", systemImage: "arrow.down.circle") { Task { await download(here) } }
                    }
                    Button("Copy Path", systemImage: "doc.on.doc") { Pasteboard.copy(here) }
                }
            if let downloading {
                VStack(alignment: .leading, spacing: Metric.gap) {
                    Label("Downloading \(downloading)…", systemImage: "arrow.down.circle")
                    if let expected, expected > 0 {
                        ProgressView(value: Double(min(received, expected)), total: Double(expected))
                    }
                    Text(expected.map { "\(Self.size(received)) of \(Self.size($0))" } ?? "\(Self.size(received)) so far").monospacedDigit()
                }
                .font(.ui(Metric.small)).foregroundStyle(flavour(.subtext0)).padding(.horizontal, Metric.pad)
            }
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
                    #if os(macOS)
                    .simultaneousGesture(TapGesture(count: 2).onEnded { if !name.hasSuffix("/") { Task { await download(path) } } })
                    #endif
                    .contextMenu {
                        Button(name.hasSuffix("/") ? "Download as .tar.gz" : "Download", systemImage: "arrow.down.circle") {
                            Task { await download(path) }
                        }
                        Button("Copy Path", systemImage: "doc.on.doc") { Pasteboard.copy(path) }
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

    /// Saves a file, or a folder packed as .tar.gz, in the Mac's Downloads and shows it in
    /// Finder; the phone offers it in the share sheet instead.
    private func download(_ path: String) async {
        let packed = path.hasSuffix("/")
        let item = (packed ? String(path.dropLast()) : path) as NSString
        let name = item.lastPathComponent + (packed ? ".tar.gz" : "")
        downloading = name
        received = 0
        expected = nil
        defer { downloading = nil }
        let tar = "tar -czf - -C \(shellPath(item.deletingLastPathComponent)) \(quote("./" + item.lastPathComponent))"
        // The size comes first, then the bytes as base64 in lines that each decode alone.
        let send = packed ? "echo && \(tar)" : "wc -c < \(shellPath(path)) && cat \(shellPath(path))"
        var data = Data()
        var sized = false
        var broken = false
        let ran = await machine.fetch("set -o pipefail; \(send) | base64 | tr -d '\\n' | fold -w 65536") { line in
            guard sized else {
                sized = true
                return expected = Int(line.trimmingCharacters(in: .whitespaces))
            }
            guard let chunk = Data(base64Encoded: line) else { return broken = true }
            data.append(chunk)
            received = data.count
        }
        guard ran.ok, !broken, packed ? !data.isEmpty : data.count == expected else {
            return problem = "\(name) could not be read from \(machine.title)."
        }
        do {
            #if os(macOS)
            let downloads = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask)[0]
            var target = downloads.appending(path: name)
            var copy = 1
            while FileManager.default.fileExists(atPath: target.path) {
                copy += 1
                target = downloads.appending(path: "\(copy) \(name)")
            }
            try data.write(to: target)
            NSWorkspace.shared.activateFileViewerSelecting([target])
            #else
            let folder = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try data.write(to: folder.appending(path: name))
            shared = folder.appending(path: name)
            #endif
        } catch {
            problem = "\(name) could not be saved: \(error.localizedDescription)"
        }
    }

    private static func size(_ bytes: Int) -> String {
        Int64(bytes).formatted(.byteCount(style: .file))
    }

    static func parent(of folder: String) -> String {
        let up = (String(folder.dropLast()) as NSString).deletingLastPathComponent
        return up.hasSuffix("/") ? up : up + "/"
    }
}

#if os(iOS)
private struct ShareSheet: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [url], applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
#endif
