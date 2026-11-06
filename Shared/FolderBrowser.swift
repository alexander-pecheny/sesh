import SwiftUI

/// A file's or folder's Document, browsed as in Finder: the folder's folders and files in a
/// list, and the file picked shown beside it to read or edit, or on a phone in place of the list.
struct FolderBrowser: View {
    @Environment(\.flavour) private var flavour
    let document: DocumentText
    let vault: Vault
    let root: String
    let machine: Machine
    /// The folder listed, as the machine spells it, ending in a slash.
    @State private var folder: String?
    @State private var entries: [String] = []
    @State private var problem: String?
    @ObservedObject private var downloads = Downloads.shared
    /// The downloads started here, whose finished files the phone offers to share.
    @State private var started: Set<UUID> = []
    #if os(iOS)
    /// A downloaded file waiting in the share sheet, to save to Files or send on.
    @State private var shared: URL?
    #endif
    @State private var picked: String?
    /// Each file opened, kept so that unsaved edits survive picking another.
    @State private var files: [String: DocumentText] = [:]
    #if os(macOS)
    @AppStorage("folderListWidth") private var listWidth = 260.0
    #endif

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

    var body: some View {
        Group {
            #if os(macOS)
            // HSplitView lays a rebuilt list out at its own width and the saved one is lost.
            HStack(spacing: 0) {
                list.frame(width: listWidth)
                SplitHandle(width: $listWidth, range: 160...720)
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
        .onChange(of: finished) { before, after in if let new = after.first(where: { !before.contains($0) }) { shared = new } }
        .sheet(isPresented: Binding(get: { shared != nil }, set: { if !$0 { shared = nil } })) {
            if let shared { ShareSheet(url: shared) }
        }
        #endif
    }

    #if os(iOS)
    private var finished: [URL] {
        downloads.items.compactMap { item in
            guard started.contains(item.id), case .done(let url) = item.state else { return nil }
            return url
        }
    }
    #endif

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
            ForEach(downloads.items.filter { started.contains($0.id) }) { item in
                #if os(iOS)
                DownloadRow(item: item) { shared = $0 }.padding(.horizontal, Metric.pad)
                #else
                if item.running { DownloadRow(item: item).padding(.horizontal, Metric.pad) }
                #endif
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
            Label { TruncatedName(name: name) } icon: { Image(systemName: icon) }
                .frame(maxWidth: .infinity, alignment: .leading).contentShape(.rect)
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
        started.insert(downloads.start(path, from: machine))
    }

    static func parent(of folder: String) -> String {
        let up = (String(folder.dropLast()) as NSString).deletingLastPathComponent
        return up.hasSuffix("/") ? up : up + "/"
    }
}

/// A name on one line that shows itself whole on hover, once it no longer fits.
private struct TruncatedName: View {
    let name: String
    @State private var shown: CGFloat = 0
    @State private var whole: CGFloat = 0

    var body: some View {
        Text(name).lineLimit(1)
            .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { shown = $0 }
            .background(alignment: .leading) {
                Text(name).fixedSize().hidden()
                    .onGeometryChange(for: CGFloat.self) { $0.size.width } action: { whole = $0 }
            }
            .help(whole > shown + 1 ? name : "")
    }
}

#if os(macOS)
/// The line between a list and what it shows, dragged to set the list's width.
private struct SplitHandle: View {
    @Environment(\.flavour) private var flavour
    @Binding var width: Double
    let range: ClosedRange<Double>
    @State private var start: Double?

    var body: some View {
        Rectangle().fill(flavour(.surface0)).frame(width: 1)
            .padding(.horizontal, Metric.tiny)
            .contentShape(.rect)
            .onHover { inside in if inside { NSCursor.resizeLeftRight.push() } else { NSCursor.pop() } }
            .gesture(DragGesture(minimumDistance: 1, coordinateSpace: .global)
                .onChanged { drag in
                    let from = start ?? width
                    start = from
                    width = min(max(from + drag.translation.width, range.lowerBound), range.upperBound)
                }
                .onEnded { _ in start = nil })
            .padding(.horizontal, -Metric.tiny)
    }
}
#endif

#if os(iOS)
private struct ShareSheet: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: [url], applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
#endif
