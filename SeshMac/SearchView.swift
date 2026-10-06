import SwiftUI

/// One search hit, from a Vault's Host or, offline, from this Mac's copy.
struct Hit: Identifiable, Decodable, Hashable {
    let kind: String
    let id: String
    let task: String?
    let session: String?
    let item: String?
    let snippet: String
    var vault = ""

    private enum CodingKeys: String, CodingKey { case kind, id, task, session, item, snippet }
}

@MainActor
final class Search: ObservableObject {
    @Published var query = "" { didSet { schedule() } }
    @Published private(set) var hits: [Hit] = []
    @Published private(set) var searching = false
    /// Vaults searched from the copy here, without their Conversations.
    @Published private(set) var offline: [String] = []

    weak var library: Library?
    private var pending: Task<Void, Never>?
    private static let pause = Duration.milliseconds(300)

    private func schedule() {
        pending?.cancel()
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return hits = [] }
        pending = Task { [weak self] in
            try? await Task.sleep(for: Self.pause)
            guard !Task.isCancelled else { return }
            await self?.run(query)
        }
    }

    private func run(_ query: String) async {
        guard let library else { return }
        searching = true
        defer { searching = false }
        var found: [Hit] = []
        var missed: [String] = []
        for vault in library.vaults {
            let ran = await vault.online ? vault.machine.run("\(Helper.path) vault search \(vault.folder) \(quote(query)) --limit 50") : Ran(status: -1, out: "", err: "")
            guard !Task.isCancelled else { return }
            if ran.ok {
                found += ran.out.split(separator: "\n").compactMap { line in
                    var hit = try? JSONDecoder().decode(Hit.self, from: Data(line.utf8))
                    hit?.vault = vault.name
                    return hit
                }
            } else {
                missed.append(vault.name)
                found += Self.local(query, in: vault)
            }
        }
        hits = found
        offline = missed
    }

    /// Every word somewhere in a record's title or text, as the Host's search would want.
    private static func local(_ query: String, in vault: Vault) -> [Hit] {
        let words = query.lowercased().split(separator: " ")
        return vault.records.values.filter { !$0.deleted && $0.kind != .session }.compactMap { record in
            let text = [record.body.title, record.body.text].compactMap { $0 }.joined(separator: " ")
            guard words.allSatisfy({ text.lowercased().contains($0) }) else { return nil }
            var hit = Hit(kind: record.kind.rawValue, id: record.id, task: record.kind == .task ? record.id : record.body.task,
                          session: nil, item: nil, snippet: String(text.prefix(160)))
            hit.vault = vault.name
            return hit
        }
    }
}

struct SearchView: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var search: Search

    var body: some View {
        List {
            if !search.offline.isEmpty {
                Label("\(search.offline.joined(separator: ", ")) searched from this Mac's copy, without Conversations.", systemImage: "wifi.slash")
                    .font(.caption).foregroundStyle(.secondary)
            }
            ForEach(search.hits) { hit in
                Button { open(hit) } label: { HitRow(hit: hit) }.buttonStyle(.plain)
            }
        }
        .overlay {
            if search.searching { ProgressView() } else if search.hits.isEmpty {
                Text("Nothing found").foregroundStyle(.secondary)
            }
        }
    }

    private func open(_ hit: Hit) {
        guard let task = hit.task, let vault = library.vaults.first(where: { $0.name == hit.vault }) else { return }
        switch hit.kind {
        case "transcript":
            if let session = hit.session, let url = Library.link(vault: vault.name, session: session, item: hit.item ?? "") {
                _ = library.follow(url)
            }
        case "document": library.open(.document(hit.id), in: task)
        case "session": library.open(.session(hit.id), in: task)
        default: library.open(.journal, in: task)
        }
        search.query = ""
    }
}

private struct HitRow: View {
    @EnvironmentObject private var library: Library
    let hit: Hit

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.tiny) {
            HStack(spacing: Metric.gap) {
                Image(systemName: icon).foregroundStyle(.secondary)
                Text(taskTitle).font(.ui(Metric.caption)).foregroundStyle(.secondary)
            }
            Text(Self.marked(hit.snippet)).lineLimit(3)
        }
        .padding(.vertical, Metric.tiny)
        .contentShape(.rect)
    }

    private var taskTitle: String {
        let vault = library.vaults.first { $0.name == hit.vault }
        let task = hit.task.flatMap { vault?.records[$0]?.body.title } ?? "No Task"
        return "\(hit.vault) · \(task)"
    }

    private var icon: String {
        switch hit.kind {
        case "transcript": "bubble.left.and.text.bubble.right"
        case "document": "doc.text"
        case "task": "checklist"
        default: "book"
        }
    }

    /// The Host marks matches with `**`; they become bold here.
    static func marked(_ snippet: String) -> AttributedString {
        var result = AttributedString()
        for (index, part) in snippet.components(separatedBy: "**").enumerated() {
            var piece = AttributedString(part)
            if index % 2 == 1 { piece.font = .body.bold() }
            result += piece
        }
        return result
    }
}
