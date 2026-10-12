import Foundation

/// One row of a Vault, as the helper stores it (docs/PLAN.md, Vault protocol).
struct Record: Codable, Identifiable, Equatable {
    enum Kind: String, Codable {
        case folder, task, entry, document, session, tab, conflict, settings
    }

    /// Every field any kind uses; each kind fills its own.
    struct Body: Codable, Equatable {
        var title: String?
        var text: String?
        /// The Task an entry, document, session or conflict belongs to.
        var task: String?
        /// The folder a folder or Task sits in; nil at the Vault's top.
        var parent: String?
        var position: Double?
        /// When the text was last edited, in milliseconds since 1970.
        var edited: Int64?
        /// When an Entry happened, in milliseconds since 1970.
        var at: Int64?
        /// The ssh alias of the machine, or nil for the Mac.
        var machine: String?
        var path: String?
        var branch: String?
        var repo: String?
        var pane: String?
        /// The herdr Workspace a Task's Worktree opened as.
        var workspace: String?
        var agent: String?
        var archived: Bool?
        /// Transcript copies of a session, oldest first.
        var transcripts: [String]?
        var of: String?
        var kind: String?
        /// The Tabs a Task's strip shows on every device, in order.
        var tabs: [TabItem]?
        /// A Vault's settings: what in an Agent's messages links where.
        var links: [LinkRule]?
    }

    let id: String
    var kind: Kind
    var body: Body
    var seq: Int = 0
    var deleted = false
}

/// Text matching `pattern` in an Agent's message links to `url`, where `$0` is the match
/// and `$1` its first group.
struct LinkRule: Codable, Equatable, Identifiable {
    var id = UUID()
    var pattern = ""
    var url = ""

    private enum CodingKeys: String, CodingKey { case pattern, url }

    func links(in text: String) -> [(Range<String.Index>, URL)] {
        guard let regex = try? Regex(pattern) else { return [] }
        return text.matches(of: regex).compactMap { match in
            guard !match.range.isEmpty else { return nil }
            var target = url
            for index in stride(from: match.count - 1, through: 0, by: -1) {
                let part = match[index].substring.map(String.init) ?? ""
                target = target.replacingOccurrences(of: "$\(index)", with: part.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? part)
            }
            return URL(string: target).map { (match.range, $0) }
        }
    }
}

/// The records of one Vault, read and written; the Vault keeps them in step with its Host.
@MainActor
protocol Records: AnyObject {
    var records: [String: Record] { get }
    func write(_ record: Record)
}

extension Records {
    func all(_ kind: Record.Kind) -> [Record] {
        records.values.filter { $0.kind == kind && !$0.deleted }
    }

    func children(_ kind: Record.Kind, task: String) -> [Record] {
        all(kind).filter { $0.body.task == task }
    }

    func create(_ kind: Record.Kind, _ body: Record.Body) -> Record {
        let record = Record(id: UUID().uuidString.lowercased(), kind: kind, body: body)
        write(record)
        return record
    }

    /// The Vault's one settings record, empty until someone saves it.
    var settings: Record {
        records["settings"].flatMap { $0.deleted ? nil : $0 } ?? Record(id: "settings", kind: .settings, body: .init())
    }

    func delete(_ record: Record) {
        var record = record
        record.deleted = true
        write(record)
    }
}
