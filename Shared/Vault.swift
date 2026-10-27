import Foundation

/// A Vault's copy on this Mac: every record, the Vault's change counter it has seen, and the
/// changes not yet accepted by the Vault's Host. Writes land here first and reach the Host
/// whenever it is reachable (ADR 0008).
@MainActor
final class Vault: ObservableObject, Identifiable, Records {
    /// The Vault's name on its Host, and the Host: an ssh alias, nil for the Mac. The phone
    /// also keeps which of its Hosts it reaches that alias by.
    struct Place: Codable, Hashable {
        let name: String
        let alias: String?
        var host: UUID?
    }

    private struct Change: Codable {
        let id: String
        let kind: Record.Kind
        let body: Record.Body
        let base: Int
        let deleted: Bool
    }

    private struct Saved: Codable {
        /// Not `seq`, so a copy saved while acknowledge could skip records fetches them all again.
        var since: Int?
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
    private(set) lazy var copier = Copier(vault: self)
    private(set) lazy var mirror = Mirror(vault: self, folder: file.deletingPathExtension().appending(path: "transcripts"))

    nonisolated var id: String { place.name }
    var name: String { place.name }
    var folder: String { "~/.sesh/vaults/\(place.name)" }

    init(_ place: Place) {
        self.place = place
        #if os(iOS)
        if let alias = place.alias, let host = place.host { Machine.bind(alias, to: host) }
        #endif
        machine = .named(place.alias)
        load()
        // Every device writes the same record, so the Host keeps one whichever arrives first.
        if records[scratchpad] == nil { write(Record(id: scratchpad, kind: .task, body: .init(title: "Scratchpad", kind: "scratchpad"))) }
    }

    var scratchpad: String { "scratchpad-" + place.name }

    private var file: URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appending(path: "\(Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "Sesh")/vaults/\(place.name).json")
    }

    private func load() {
        guard let data = try? Data(contentsOf: file), let saved = try? JSONDecoder().decode(Saved.self, from: data) else { return }
        seq = saved.since ?? 0
        records = saved.records
        queue = saved.queue
        pending = queue.count
    }

    private func save() {
        let saved = Saved(since: seq, records: records, queue: queue)
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? JSONEncoder().encode(saved).write(to: file, options: .atomic)
    }

    // MARK: Writing

    /// Changes a record here at once and queues it for the Host. An editor passes the seq it
    /// started from as `base`, so the Host keeps a conflict copy if another device wrote since.
    func write(_ record: Record) { write(record, base: nil) }

    func write(_ record: Record, base: Int?) {
        var record = record
        let base = base ?? records[record.id]?.seq ?? 0
        record.seq = base
        records[record.id] = record
        queue.removeAll { $0.id == record.id }
        queue.append(Change(id: record.id, kind: record.kind, body: record.body, base: base, deleted: record.deleted))
        pending = queue.count
        save()
        Task { await flush() }
    }

    // MARK: Talking to the Host

    /// Keeps this copy in step with the Host until `stop`, coming back after every drop.
    func start() {
        guard following == nil else { return }
        copier.start()
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

    /// Connects again now, rather than at the next try, when offline.
    func retry() {
        guard !online else { return }
        stop()
        start()
    }

    private func connect() async {
        let ready = await machine.prepare()
        guard ready == nil else { return problem = ready }
        let made = await machine.run("\(Helper.path) vault init \(folder)")
        guard made.ok else { return problem = made.problem }
        problem = nil
        online = true
        if let head = try? JSONDecoder().decode(Line.self, from: Data(made.out.utf8)).seq, head < seq { await restore() }
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

    /// The Host is older than this copy, as after its disk was erased: it gets back every record
    /// it lacks and the recent Transcripts, and this copy reads it all again.
    private func restore() async {
        var have: Set<String> = []
        let pulled = await machine.fetch("\(Helper.path) vault pull \(folder)") { text in
            if let id = try? JSONDecoder().decode(Line.self, from: Data(text.utf8)).id { have.insert(id) }
        }
        guard pulled.ok else { return }
        for record in records.values where !have.contains(record.id) && !queue.contains(where: { $0.id == record.id }) {
            queue.append(Change(id: record.id, kind: record.kind, body: record.body, base: 0, deleted: record.deleted))
        }
        seq = 0
        pending = queue.count
        save()
        await mirror.giveBack()
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
    /// or the Host would take it for a clash with our own earlier write. Only `follow` moves
    /// `seq`, or a push from a copy that lags would skip every record it had not yet seen.
    private func acknowledge(_ text: String) {
        guard let line = try? JSONDecoder().decode(Line.self, from: Data(text.utf8)), line.t == "record",
              let id = line.id, let lineSeq = line.seq else { return }
        if let index = queue.firstIndex(where: { $0.id == id }) {
            let change = queue[index]
            queue[index] = Change(id: id, kind: change.kind, body: change.body, base: lineSeq, deleted: change.deleted)
        }
        records[id]?.seq = lineSeq
    }
}
