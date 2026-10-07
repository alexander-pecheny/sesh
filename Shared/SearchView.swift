import SwiftUI

struct SearchView: View {
    @EnvironmentObject private var library: Library
    @ObservedObject var search: Search

    var body: some View {
        List {
            if !search.offline.isEmpty {
                Label("\(search.offline.joined(separator: ", ")) searched from this \(Self.device)'s copy, without Conversations.", systemImage: "wifi.slash")
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

    #if os(iOS)
    private static let device = "phone"
    #else
    private static let device = "Mac"
    #endif

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
