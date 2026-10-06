import Foundation

/// One row of a Vault, as the helper stores it (docs/PLAN.md, Vault protocol).
struct Record: Codable, Identifiable, Equatable {
    enum Kind: String, Codable {
        case folder, task, entry, document, session, tab, conflict
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
    }

    let id: String
    var kind: Kind
    var body: Body
    var seq: Int = 0
    var deleted = false
}

/// A Vault's copy on this Mac: every record, the Vault's change counter it has seen, and the
/// changes not yet accepted by the Vault's Host. Writes land here first and reach the Host
/// whenever it is reachable (ADR 0008).
@MainActor
final class Vault: ObservableObject, Identifiable {
    struct Place: Codable, Hashable {
        let name: String
        let alias: String?
    }

    private struct Change: Codable {
        let id: String
        let kind: Record.Kind
        let body: Record.Body
        let base: Int
        let deleted: Bool
    }

    private struct Saved: Codable {
        var seq = 0
        var records: [String: Record] = [:]
        var queue: [Change] = []
    }

    private struct Line: Decodable {
        let t: String
        let seq: Int?
        let id: String?
        let kind: Record.Kind?
        let body: Record.Body?
        let deleted: Bool?
    }

    @Published private(set) var records: [String: Record] = [:]
    @Published private(set) var online = false
    @Published private(set) var pending = 0
    @Published var problem: String?

    let place: Place
    let machine: Machine
    private var seq = 0
    private var queue: [Change] = []
    private var flushing = false
    private var following: Task<Void, Never>?

    nonisolated var id: String { place.name }
    var name: String { place.name }
    var folder: String { "~/.sesh/vaults/\(place.name)" }

    init(_ place: Place) {
        self.place = place
        machine = .named(place.alias)
        load()
    }

    private var file: URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appending(path: "Sesh/vaults/\(place.name).json")
    }

    private func load() {
        guard let data = try? Data(contentsOf: file), let saved = try? JSONDecoder().decode(Saved.self, from: data) else { return }
        seq = saved.seq
        records = saved.records
        queue = saved.queue
        pending = queue.count
    }

    private func save() {
        let saved = Saved(seq: seq, records: records, queue: queue)
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? JSONEncoder().encode(saved).write(to: file, options: .atomic)
    }

    // MARK: Reading

    func all(_ kind: Record.Kind) -> [Record] {
        records.values.filter { $0.kind == kind && !$0.deleted }
    }

    func children(_ kind: Record.Kind, task: String) -> [Record] {
        all(kind).filter { $0.body.task == task }
    }

    // MARK: Writing

    /// Changes a record here at once and queues it for the Host.
    func write(_ record: Record) {
        var record = record
        let base = records[record.id]?.seq ?? 0
        record.seq = base
        records[record.id] = record
        queue.removeAll { $0.id == record.id }
        queue.append(Change(id: record.id, kind: record.kind, body: record.body, base: base, deleted: record.deleted))
        pending = queue.count
        save()
        Task { await flush() }
    }

    func create(_ kind: Record.Kind, _ body: Record.Body) -> Record {
        let record = Record(id: UUID().uuidString.lowercased(), kind: kind, body: body)
        write(record)
        return record
    }

    func delete(_ record: Record) {
        var record = record
        record.deleted = true
        write(record)
    }

    // MARK: Talking to the Host

    /// Keeps this copy in step with the Host until `stop`, coming back after every drop.
    func start() {
        guard following == nil else { return }
        following = Task { [weak self] in
            while !Task.isCancelled, let self {
                await self.connect()
                self.online = false
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    func stop() {
        following?.cancel()
        following = nil
    }

    private func connect() async {
        let ready = await machine.prepare()
        guard ready == nil else { return problem = ready }
        let made = await machine.run("\(Helper.path) vault init \(folder)")
        guard made.ok else { return problem = made.problem }
        problem = nil
        online = true
        await flush()
        let ended = await machine.stream("\(Helper.path) vault follow \(folder) --since \(seq)") { [weak self] in
            self?.apply($0)
        }
        if !ended.ok, !Task.isCancelled { problem = ended.problem }
    }

    private func apply(_ text: String) {
        guard let line = try? JSONDecoder().decode(Line.self, from: Data(text.utf8)) else { return }
        switch line.t {
        case "record":
            guard let id = line.id, let kind = line.kind, let lineSeq = line.seq else { return }
            seq = max(seq, lineSeq)
            // A change still queued here is newer than what the Host had.
            guard !queue.contains(where: { $0.id == id }) else { return }
            records[id] = Record(id: id, kind: kind, body: line.body ?? Record.Body(), seq: lineSeq, deleted: line.deleted ?? false)
            save()
        case "head": if let head = line.seq { seq = max(seq, head); save() }
        default: break
        }
    }

    /// Sends the queue as a file, since one Document can outgrow a command line, until it is empty.
    private func flush() async {
        guard online, !flushing else { return }
        flushing = true
        defer { flushing = false }
        while online, !queue.isEmpty {
            let sent = queue
            let lines = sent.compactMap { try? JSONEncoder().encode($0) }.map { String(decoding: $0, as: UTF8.self) }
            let name = "\(folder)/inbox/\(UUID().uuidString.lowercased()).jsonl"
            let put = await machine.put(Data((lines.joined(separator: "\n") + "\n").utf8), to: name)
            guard put.ok else { return problem = put.problem }
            let ran = await machine.run("\(Helper.path) vault push \(folder) \(name)")
            guard ran.ok else { return problem = ran.problem }
            // Only what was sent leaves the queue; edits made meanwhile wait for the next round.
            for change in sent {
                guard let index = queue.firstIndex(where: { $0.id == change.id }),
                      queue[index].body == change.body, queue[index].deleted == change.deleted else { continue }
                queue.remove(at: index)
            }
            for text in ran.out.split(separator: "\n") { acknowledge(String(text)) }
            pending = queue.count
            save()
        }
    }

    /// A record the Host wrote for us. An edit still queued for it now builds on its new seq,
    /// or the Host would take it for a clash with our own earlier write.
    private func acknowledge(_ text: String) {
        guard let line = try? JSONDecoder().decode(Line.self, from: Data(text.utf8)), line.t == "record",
              let id = line.id, let lineSeq = line.seq, let index = queue.firstIndex(where: { $0.id == id })
        else { return apply(text) }
        let change = queue[index]
        queue[index] = Change(id: id, kind: change.kind, body: change.body, base: lineSeq, deleted: change.deleted)
        records[id]?.seq = lineSeq
        seq = max(seq, lineSeq)
    }
}
