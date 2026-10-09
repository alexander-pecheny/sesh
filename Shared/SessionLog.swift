import Foundation

/// One Agent session's Session log as the Conversation shows it (ADR 0010), built from the
/// lines Sesh's transcript helper prints: the follower's items for a pane, or a Transcript
/// file's entries, which the helper has already turned into the same entries (ADR 0006).
struct SessionLog {
    static let protocols: Set<Int> = [1, 2, 3]
    /// The first protocol that can page back through a Transcript with `history`.
    private static let paging = 2
    /// The first protocol in which the follower sends the Session log rather than entries.
    private static let follower = 3

    struct Entry: Codable, Identifiable, Hashable {
        let id: String
        let kind: String
        let summary: String
        /// When the Agent wrote it, as ISO 8601.
        var at: String?
        var text: String?
        var images: [String]?
        var seconds: Double?
        var tool: String?
        var name: String?
        var file: String?
        var command: String?
        var description: String?
        var call: String?
        var diff: String?
        var added: Int?
        var removed: Int?
        var error: Bool?
        var truncated: Bool?
        var items: [Todo]?
        var questions: [Question]?
        var answers: [String]?
    }

    struct Todo: Codable, Equatable, Hashable {
        let text: String
        let status: String
    }

    struct Question: Codable, Hashable {
        struct Option: Codable, Hashable {
            let label: String
            let description: String?
        }
        let question: String
        let header: String?
        let multi: Bool?
        let options: [Option]
    }

    struct Permission: Decodable, Identifiable, Hashable {
        let id: String
        let tool: String
        let summary: String
        let command: String?
        let file: String?
        let reason: String?
        /// A menu read off the screen, with no hook to describe it: its choices, each picked by a key.
        let options: [Choice]?

        struct Choice: Decodable, Equatable, Hashable {
            let key: String
            let label: String
        }
    }

    /// What the Agent left running in the background, a command or a subagent.
    struct Background: Decodable, Equatable, Identifiable {
        let call: String
        let label: String
        /// A subagent, which has a Transcript of its own, rather than a command.
        var agent: Bool?
        var id: String { call }
    }

    /// One item of the log. Its id names its row, which an entry from the Transcript takes
    /// over from the screen's item, so the row keeps its place and id.
    struct Item: Codable {
        let id: String
        var ord: Int
        var seq = 0
        var final: Bool
        var gone = false
        var entry: Entry
        var version = 0

        private enum CodingKeys: String, CodingKey { case id, ord, seq, final, gone, entry }
    }

    /// What the list shows: one entry, a switch of Transcript, or a run of reads, searches and
    /// fetches as one line.
    struct Row: Identifiable, Equatable {
        enum Content {
            case entry(Entry)
            case switched(reason: String)
            case lookups([Entry])
        }

        let id: String
        var content: Content
        /// Changes whenever anything the row draws does, its tool's result included.
        var version: Int

        var entries: [Entry] {
            switch content {
            case .entry(let entry): [entry]
            case .lookups(let entries): entries
            case .switched: []
            }
        }

        func contains(_ id: String) -> Bool { entries.contains { $0.id == id } }

        static func == (one: Row, other: Row) -> Bool { one.id == other.id && one.version == other.version }
    }

    private struct Line: Decodable {
        let t: String
        let `protocol`: Int?
        let agent: String?
        let transcript: String?
        let state: String?
        let status: String?
        let reason: String?
        let cursor: String?
        let id: String?
        let seq: Int?
        let more: Bool?
        let message: String?
        let replaces: String?
        let items: [Entry]?
        let tasks: [Background]?
        let background: [Background]?
        let permissions: [Permission]?
    }

    /// Everything the chat shows, in order: the final items, then the screen's. Claude saves
    /// a question, and the text before it, only once it is answered, so the question its
    /// hook reported stays last.
    private(set) var rows: [Row] = []
    /// Each tool call's result, by the call's entry.
    private(set) var results: [String: Entry] = [:]
    private(set) var todo: Entry?
    private(set) var permissions: [Permission] = []
    private(set) var background: [Background] = []
    /// The status line on the Agent's screen.
    private(set) var status = ""
    private(set) var state = ""
    private(set) var agent: String?
    /// The Transcript the helper is reading now.
    private(set) var transcript: String?
    /// The last number the follower stamped, which a reconnect asks to go on from.
    private(set) var seq = 0
    /// Where a Transcript file's entries were read to, which a reconnect asks to go on from.
    private(set) var cursor: String?
    /// Whether the opening batch is in.
    private(set) var loaded = false
    /// Whether items older than the first one held can still be fetched.
    var earlier = false
    var problem: String?
    private var items: [String: Item] = [:]
    /// The row of each entry whose item has another id.
    private var rowKeys: [String: String] = [:]
    /// The items sent since the follower last said hello.
    private var fresh: Set<String> = []
    /// When each screen item first showed, since the screen does not date it.
    private var firstSeen: [String: String] = [:]
    private var last = 0
    private var switches = 0
    private static let dates: ISO8601DateFormatter = {
        let dates = ISO8601DateFormatter()
        dates.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return dates
    }()

    static func isLive(_ id: String) -> Bool { id.hasPrefix("live.") }

    /// The row an entry is drawn in.
    func rowKey(_ id: String) -> String { rowKeys[id] ?? id }

    /// The first item held, which a page of earlier ones ends before.
    var first: Item? { items.values.min { $0.ord < $1.ord } }

    /// The last items held, to keep on the device.
    func kept(_ count: Int) -> [Item] { Array(items.values.sorted { $0.ord < $1.ord }.suffix(count)) }

    /// Takes the items kept on the device, until the follower says what changed since.
    mutating func restore(_ kept: [Item], seq: Int) {
        kept.forEach { put($0, now: Date()) }
        self.seq = seq
        loaded = true
        earlier = true
        rebuild()
    }

    mutating func feed(_ text: String, now: Date = Date()) {
        let data = Data(text.utf8)
        let decoder = JSONDecoder()
        guard let line = try? decoder.decode(Line.self, from: data) else { return }
        switch line.t {
        case "hello":
            agent = line.agent ?? agent
            transcript = line.transcript.flatMap { $0.isEmpty ? nil : $0 } ?? transcript
            if let number = line.protocol, !Self.protocols.contains(number) {
                problem = "The Host's helper speaks protocol \(number), which this Sesh does not know. Update Sesh."
            }
            if (line.protocol ?? 0) >= Self.follower { fresh = [] } else { earlier = (line.protocol ?? 0) >= Self.paging }
        case "error": problem = line.message
        case "item":
            guard let item = try? decoder.decode(Item.self, from: data) else { return }
            seq = max(seq, item.seq)
            fresh.insert(item.id)
            if item.gone { items[item.id] = nil } else { put(item, now: now) }
            rebuild()
        case "session":
            state = line.state ?? state
            status = line.status ?? ""
            agent = line.agent ?? agent
            transcript = line.transcript ?? transcript
            background = line.background ?? []
            permissions = line.permissions ?? []
        case "opened":
            // The follower sent its last items afresh, perhaps from a new log, so any others held are stale.
            items = items.filter { fresh.contains($0.key) }
            seq = line.seq ?? seq
            loaded = true
            earlier = !items.isEmpty
            rebuild()
        case "entry":
            guard let entry = try? decoder.decode(Entry.self, from: data) else { return }
            if let replaced = line.replaces, let live = items[replaced], !live.final {
                items = items.filter { $0.value.final || $0.value.ord > live.ord }
                put(Item(id: replaced, ord: place(entry), final: true, entry: entry), now: now)
            } else {
                add(entry, now: now)
            }
            rebuild()
        case "live":
            status = line.status ?? ""
            items = items.filter { $0.value.final }
            for entry in line.items ?? [] {
                last += 1
                put(Item(id: entry.id, ord: last, final: false, entry: entry), now: now)
            }
            rebuild()
        case "switch":
            switches += 1
            last += 1
            let id = "switch-\(switches)"
            put(Item(id: id, ord: last, final: true, entry: Entry(id: id, kind: "switch", summary: line.reason ?? "other")), now: now)
            earlier = false
            rebuild()
        case "state": state = line.state ?? state
        case "permission":
            guard let permission = try? decoder.decode(Permission.self, from: data) else { return }
            permissions.removeAll { $0.id == permission.id }
            permissions.append(permission)
        case "permission_done": permissions.removeAll { $0.id == line.id }
        case "background": background = line.tasks ?? []
        case "cursor":
            cursor = line.cursor
            // A new Agent session has no Transcript until its first message, so nothing is older;
            // every reconnect says hello again, so this is checked each time.
            if !items.values.contains(where: \.final) { earlier = false }
            loaded = true
        default: break
        }
    }

    /// Takes one page of earlier items, as `page` or `history` prints it, and puts it above.
    mutating func page(_ text: String) {
        let decoder = JSONDecoder()
        let held = Set(items.values.map(\.entry.id))
        var older: [Entry] = []
        for text in text.split(separator: "\n") {
            let data = Data(text.utf8)
            guard let line = try? decoder.decode(Line.self, from: data) else { continue }
            switch line.t {
            case "page_done", "history": earlier = line.more ?? false
            case "item": if let item = try? decoder.decode(Item.self, from: data), !item.gone { put(item, now: Date()) }
            case "entry": if let entry = try? decoder.decode(Entry.self, from: data), !held.contains(entry.id) { older.append(entry) }
            default: break
            }
        }
        let first = first?.ord ?? 0
        for (offset, entry) in older.enumerated() {
            put(Item(id: entry.id, ord: first - older.count + offset, final: true, entry: entry), now: Date())
        }
        rebuild()
    }

    /// Puts the whole of an entry the helper had cut down in its place.
    mutating func expand(_ entry: Entry) {
        add(entry, now: Date())
        rebuild()
    }

    private mutating func add(_ entry: Entry, now: Date) {
        if var held = items.values.first(where: { $0.entry.id == entry.id }) {
            held.entry = entry
            put(held, now: now)
        } else {
            put(Item(id: entry.id, ord: place(entry), final: true, entry: entry), now: now)
        }
    }

    /// A new entry of a Transcript file goes last, but above a question still unanswered.
    private mutating func place(_ entry: Entry) -> Int {
        last += 1
        let shown = items.values.filter { $0.final && !["result", "todo"].contains($0.entry.kind) }
        guard !["result", "todo"].contains(entry.kind), let question = shown.max(by: { $0.ord < $1.ord }),
              question.entry.kind == "question", !items.values.contains(where: { $0.entry.call == question.entry.id })
        else { return last }
        items[question.id]?.ord = last
        return question.ord
    }

    private mutating func put(_ item: Item, now: Date) {
        var item = item
        if !item.final {
            item.entry.at = item.entry.at ?? firstSeen[item.id] ?? Self.dates.string(from: now)
            firstSeen[item.id] = item.entry.at
        }
        item.version = item.entry.hashValue
        items[item.id] = item
    }

    private mutating func rebuild() {
        var finals: [Item] = []
        var live: [Item] = []
        var answers: [String: Item] = [:]
        todo = nil
        rowKeys = [:]
        for item in items.values.sorted(by: { $0.ord < $1.ord }) {
            if item.id != item.entry.id { rowKeys[item.entry.id] = item.id }
            switch item.entry.kind {
            case "result": if let call = item.entry.call { answers[call] = item }
            case "todo": todo = item.entry
            case "switch":
                todo = nil
                finals.append(item)
            default: if item.final { finals.append(item) } else { live.append(item) }
            }
        }
        results = answers.mapValues(\.entry)
        var shown = finals + live
        if let question = finals.last, question.entry.kind == "question", answers[question.entry.id] == nil {
            shown.remove(at: finals.count - 1)
            shown.append(question)
        }
        rows = []
        for item in shown {
            let entry = item.entry
            let version = Self.mix(item.version, answers[entry.id]?.version)
            if entry.kind == "switch" {
                rows.append(Row(id: item.id, content: .switched(reason: entry.summary), version: version))
            } else if entry.kind != "tool" || !["read", "search", "fetch"].contains(entry.tool) {
                rows.append(Row(id: item.id, content: .entry(entry), version: version))
            } else if case .lookups(let run)? = rows.last?.content {
                rows[rows.count - 1].content = .lookups(run + [entry])
                rows[rows.count - 1].version = Self.mix(rows[rows.count - 1].version, version)
            } else {
                rows.append(Row(id: item.id, content: .lookups([entry]), version: version))
            }
        }
    }

    private static func mix(_ one: Int, _ other: Int?) -> Int {
        var hasher = Hasher()
        hasher.combine(one)
        hasher.combine(other)
        return hasher.finalize()
    }
}
