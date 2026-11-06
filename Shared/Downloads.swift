import SwiftUI

/// Files and folders on their way from a machine to this device. They outlive the Tab that
/// started them, so their progress shows wherever the user is.
@MainActor
final class Downloads: ObservableObject {
    static let shared = Downloads()

    struct Item: Identifiable {
        enum State {
            case running
            /// Saved on the Mac; on the phone, waiting to be shared.
            case done(URL)
            case failed(String)
        }

        let id = UUID()
        let name: String
        let machine: String
        var received = 0
        /// The file's size; a folder is packed as it travels, so its size is not known ahead.
        var expected: Int?
        var state = State.running

        var running: Bool { if case .running = state { true } else { false } }
        var fraction: Double? { expected.flatMap { $0 > 0 ? min(Double(received) / Double($0), 1) : nil } }
    }

    @Published private(set) var items: [Item] = []
    private var tasks: [UUID: Task<Void, Never>] = [:]
    /// The most recent finished downloads kept in the list.
    private static let kept = 8

    var running: [Item] { items.filter(\.running) }
    /// How far the running downloads have come together, when every size is known.
    var fraction: Double? {
        let running = running
        guard !running.isEmpty, running.allSatisfy({ $0.expected != nil }) else { return nil }
        let total = running.reduce(0) { $0 + ($1.expected ?? 0) }
        return total > 0 ? Double(running.reduce(0) { $0 + $1.received }) / Double(total) : nil
    }

    /// Fetches a file, or a folder packed as .tar.gz, from `machine` into the Mac's Downloads, or
    /// a temporary folder on the phone for the share sheet.
    @discardableResult
    func start(_ path: String, from machine: Machine) -> UUID {
        let packed = path.hasSuffix("/")
        let item = (packed ? String(path.dropLast()) : path) as NSString
        let download = Item(name: item.lastPathComponent + (packed ? ".tar.gz" : ""), machine: machine.title)
        items.insert(download, at: 0)
        tasks[download.id] = Task { [weak self] in
            let state = await Self.fetch(path, packed: packed, named: download.name, from: machine) { received, expected in
                self?.update(download.id) { ($0.received, $0.expected) = (received, expected) }
            }
            self?.tasks[download.id] = nil
            guard !Task.isCancelled else { return }
            self?.update(download.id) { $0.state = state }
            self?.trim()
        }
        return download.id
    }

    func cancel(_ id: UUID) {
        tasks.removeValue(forKey: id)?.cancel()
        items.removeAll { $0.id == id }
    }

    func clear() { items.removeAll { !$0.running } }

    private func update(_ id: UUID, _ change: (inout Item) -> Void) {
        guard let index = items.firstIndex(where: { $0.id == id }) else { return }
        change(&items[index])
    }

    private func trim() {
        let finished = items.filter { !$0.running }
        guard finished.count > Self.kept else { return }
        let dropped = Set(finished.dropFirst(Self.kept).map(\.id))
        items.removeAll { dropped.contains($0.id) }
    }

    private static func fetch(_ path: String, packed: Bool, named name: String, from machine: Machine,
                              progress: @escaping (Int, Int?) -> Void) async -> Item.State {
        let item = (packed ? String(path.dropLast()) : path) as NSString
        let tar = "tar -czf - -C \(shellPath(item.deletingLastPathComponent)) \(quote("./" + item.lastPathComponent))"
        // The size comes first, then the bytes as base64 in lines that each decode alone.
        let send = packed ? "echo && \(tar)" : "wc -c < \(shellPath(path)) && cat \(shellPath(path))"
        var data = Data()
        var expected: Int?
        var sized = false
        var broken = false
        // dash before 0.5.13 has no pipefail; there the size check alone catches a short file.
        let ran = await machine.fetch("! (set -o pipefail) 2>/dev/null || set -o pipefail; \(send) | base64 | tr -d '\\n' | fold -w 65536") { line in
            guard sized else {
                sized = true
                expected = Int(line.trimmingCharacters(in: .whitespaces))
                return progress(0, expected)
            }
            guard let chunk = Data(base64Encoded: line) else { return broken = true }
            data.append(chunk)
            progress(data.count, expected)
        }
        guard ran.ok, !broken, packed ? !data.isEmpty : data.count == expected else {
            return .failed("\(name) could not be read from \(machine.title).")
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
            return .done(target)
            #else
            let folder = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try data.write(to: folder.appending(path: name))
            return .done(folder.appending(path: name))
            #endif
        } catch {
            return .failed("\(name) could not be saved: \(error.localizedDescription)")
        }
    }

    static func size(_ bytes: Int) -> String { Int64(bytes).formatted(.byteCount(style: .file)) }
}

/// One download's name, progress and actions.
struct DownloadRow: View {
    @Environment(\.flavour) private var flavour
    let item: Downloads.Item
    var share: ((URL) -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.tiny) {
            HStack {
                Label(item.name, systemImage: "arrow.down.circle").lineLimit(1).truncationMode(.middle)
                Spacer()
                switch item.state {
                case .running:
                    Button("Cancel", systemImage: "xmark.circle.fill") { Downloads.shared.cancel(item.id) }
                        .labelStyle(.iconOnly).buttonStyle(.borderless)
                case .done(let url):
                    #if os(macOS)
                    Button("Show in Finder", systemImage: "magnifyingglass.circle.fill") { NSWorkspace.shared.activateFileViewerSelecting([url]) }
                        .labelStyle(.iconOnly).buttonStyle(.borderless)
                    #else
                    if let share { Button("Share", systemImage: "square.and.arrow.up") { share(url) }.labelStyle(.iconOnly) }
                    #endif
                case .failed: EmptyView()
                }
            }
            if item.running, let fraction = item.fraction { ProgressView(value: fraction) }
            Text(caption).monospacedDigit()
                .foregroundStyle(failed ? flavour(.red) : flavour(.subtext0))
                .lineLimit(2)
        }
        .font(.ui(Metric.small))
    }

    private var failed: Bool { if case .failed = item.state { true } else { false } }

    private var caption: String {
        switch item.state {
        case .running: item.expected.map { "\(Downloads.size(item.received)) of \(Downloads.size($0)) from \(item.machine)" }
            ?? "\(Downloads.size(item.received)) so far from \(item.machine)"
        case .done: "\(Downloads.size(item.received)), done"
        case .failed(let why): why
        }
    }
}

#if os(macOS)
/// The toolbar's downloads button: a ring that fills while anything downloads, opening the list.
struct DownloadsButton: View {
    @ObservedObject private var downloads = Downloads.shared
    @State private var open = false

    var body: some View {
        if !downloads.items.isEmpty {
            Button { open.toggle() } label: {
                ZStack {
                    if !downloads.running.isEmpty {
                        Circle().stroke(.secondary.opacity(0.3), lineWidth: 2)
                        if let fraction = downloads.fraction {
                            Circle().trim(from: 0, to: fraction).stroke(.tint, style: StrokeStyle(lineWidth: 2, lineCap: .round)).rotationEffect(.degrees(-90))
                        } else {
                            ProgressView().controlSize(.small)
                        }
                    }
                    Image(systemName: "arrow.down").imageScale(.small)
                }
                .frame(width: Metric.control * 0.7, height: Metric.control * 0.7)
            }
            .help("Downloads")
            .accessibilityLabel("Downloads")
            .popover(isPresented: $open, arrowEdge: .bottom) {
                VStack(alignment: .leading, spacing: Metric.pad) {
                    ForEach(downloads.items) { DownloadRow(item: $0) }
                    if downloads.items.contains(where: { !$0.running }) {
                        Button("Clear Finished") { downloads.clear() }.buttonStyle(.link).font(.ui(Metric.small))
                    }
                }
                .padding(Metric.pad)
                .frame(width: 320)
            }
        }
    }
}
#endif
