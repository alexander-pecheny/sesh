import Foundation

/// One search hit, from a Vault's Host or, offline, from this device's copy.
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
    /// Set by Return in the search field: the first hit opens once there is one.
    @Published var openFirst = false

    weak var library: Library?
    private var pending: Task<Void, Never>?
    private static let pause = Duration.milliseconds(300)

    private func schedule() {
        pending?.cancel()
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else {
            searching = false
            return hits = []
        }
        // Searching from the first key, so "Nothing found" never shows before the search has run.
        searching = true
        pending = Task { [weak self] in
            try? await Task.sleep(for: Self.pause)
            guard !Task.isCancelled else { return }
            await self?.run(query)
        }
    }

    private func run(_ query: String) async {
        guard let library else { return }
        defer { if !Task.isCancelled { searching = false } }
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
        return vault.records.values.filter { !$0.deleted && [.task, .entry, .document].contains($0.kind) }.compactMap { record in
            let text = [record.body.title, record.body.text].compactMap { $0 }.joined(separator: " ")
            guard words.allSatisfy({ text.lowercased().contains($0) }) else { return nil }
            var hit = Hit(kind: record.kind.rawValue, id: record.id, task: record.kind == .task ? record.id : record.body.task,
                          session: nil, item: nil, snippet: String(text.prefix(160)))
            hit.vault = vault.name
            return hit
        }
    }
}
