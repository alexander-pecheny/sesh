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

    /// Changes sent together under one id, which the Host remembers, so sending them again after
    /// a lost answer gets the same answer and writes nothing twice (ADR 0022).
    private struct Batch: Codable {
        let id: String
        let changes: [Change]
    }

    private struct Saved: Codable {
        /// Not `seq`, so a copy saved while acknowledge could skip records fetches them all again.
        var since: Int?
        var records: [String: Record] = [:]
        var queue: [Change] = []
        var batch: Batch?
        var identity: String?
        var shadows: [String: Record]?
    }

    private struct Line: Decodable {
        let t: String
        let seq: Int?
        let id: String?
        let kind: Record.Kind?
        let body: Record.Body?
        let deleted: Bool?
        let message: String?
        let vault_id: String?

        var record: Record? {
            guard t == "record", let id, let kind, let seq else { return nil }
            return Record(id: id, kind: kind, body: body ?? Record.Body(), seq: seq, deleted: deleted ?? false)
        }
    }

    @Published private(set) var records: [String: Record] = [:]
    @Published private(set) var online = false
    @Published private(set) var pending = 0
    @Published var problem: String?

    let place: Place
    let machine: Machine
    private var seq = 0
    private var queue: [Change] = []
    private var batch: Batch?
    /// The id of the Vault on the Host this copy follows; another id means the Host's Vault was made again.
    private var identity: String?
    /// The Host's newest record of each id that follow passed over while a change to it was queued.
    private var shadows: [String: Record] = [:]
    private var flushing = false
    private var retrying: Task<Void, Never>?
    private var backoff = Vault.firstRetry
    private static let firstRetry = Duration.seconds(2)
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
        batch = saved.batch
        identity = saved.identity
        shadows = saved.shadows ?? [:]
        pending = queue.count
    }

    private func save() {
        let saved = Saved(since: seq, records: records, queue: queue, batch: batch, identity: identity, shadows: shadows)
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
        retrying?.cancel()
        retrying = nil
    }

    /// Stops following the Host and deletes this device's copy; the Host keeps the Vault.
    func forget() {
        stop()
        copier.stop()
        try? FileManager.default.removeItem(at: file)
        try? FileManager.default.removeItem(at: file.deletingPathExtension())
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
        if let head = try? JSONDecoder().decode(Line.self, from: Data(made.out.utf8)) {
            if head.seq.map({ $0 < seq }) == true || (identity != nil && head.vault_id != identity) {
                await restore(head.vault_id)
            } else if identity == nil {
                identity = head.vault_id
                save()
            }
        }
        await flush()
        let ended = await machine.follower.vault(folder, from: { [weak self] in self?.seq ?? 0 }, online: { [weak self] up in
            guard let self else { return }
            online = up
            if up { Task { await self.flush() } }
        }, line: { [weak self] in self?.apply($0) })
        if let ended, !Task.isCancelled { problem = ended }
    }

    private func apply(_ text: String) {
        guard let line = try? JSONDecoder().decode(Line.self, from: Data(text.utf8)) else { return }
        switch line.t {
        case "record":
            guard let record = line.record else { return }
            seq = max(seq, record.seq)
            // A change still queued here is newer than what the Host had; acknowledge settles which stays.
            if queue.contains(where: { $0.id == record.id }) {
                shadows[record.id] = record
            } else {
                records[record.id] = record
            }
            save()
        case "head": if let head = line.seq { seq = max(seq, head); save() }
        case "vault_error": problem = line.message
        default: break
        }
    }

    /// The Host is older than this copy, or holds another Vault, as after its disk was erased: it
    /// gets back every record it lacks or holds an older edit of, and the recent Transcripts, and
    /// this copy reads it all again.
    private func restore(_ vault: String?) async {
        var have: [String: Record.Body] = [:]
        let pulled = await machine.fetch("\(Helper.path) vault pull \(folder)") { text in
            if let record = (try? JSONDecoder().decode(Line.self, from: Data(text.utf8)))?.record { have[record.id] = record.body }
        }
        guard pulled.ok else { return }
        for record in records.values where !queue.contains(where: { $0.id == record.id }) {
            let had = have[record.id]
            // Another device may have refilled it from an older copy.
            guard had == nil || had != record.body && had?.edited ?? 0 < record.body.edited ?? 0 else { continue }
            queue.append(Change(id: record.id, kind: record.kind, body: record.body, base: 0, deleted: record.deleted))
        }
        seq = 0
        shadows = [:]
        identity = vault
        pending = queue.count
        save()
        await mirror.giveBack()
    }

    /// Sends the queue as a file, since one Document can outgrow a command line, until it is empty.
    /// A batch whose answer is lost goes again as it was, and a failure is tried again on a timer.
    private func flush() async {
        guard online, !flushing else { return }
        flushing = true
        defer { flushing = false }
        while online, !queue.isEmpty {
            let sending = batch ?? Batch(id: UUID().uuidString.lowercased(), changes: queue)
            batch = sending
            save()
            let lines = sending.changes.compactMap { try? JSONEncoder().encode($0) }.map { String(decoding: $0, as: UTF8.self) }
            let name = "\(folder)/inbox/\(sending.id).jsonl"
            let put = await machine.put(Data((lines.joined(separator: "\n") + "\n").utf8), to: name)
            guard put.ok else { return failed(put.problem) }
            let ran = await machine.run("\(Helper.path) vault push \(folder) \(name) --batch \(sending.id)")
            guard ran.ok else { return failed(ran.problem) }
            let answer = ran.out.split(separator: "\n").compactMap { try? JSONDecoder().decode(Line.self, from: Data($0.utf8)) }
            // The Host's Vault was made again since this copy connected. The next connection
            // refills it and sends the batch again, which the Vault it was written to answers.
            if let identity, let other = answer.last?.vault_id, other != identity {
                return problem = "The Vault on \(machine.title) was made again; Sesh refills it once it reconnects."
            }
            batch = nil
            // Only what was sent leaves the queue; edits made meanwhile wait for the next round.
            for change in sending.changes {
                guard let index = queue.firstIndex(where: { $0.id == change.id }),
                      queue[index].body == change.body, queue[index].deleted == change.deleted else { continue }
                queue.remove(at: index)
            }
            answer.compactMap(\.record).forEach(acknowledge)
            backoff = Self.firstRetry
            pending = queue.count
            save()
        }
    }

    private func failed(_ why: String) {
        problem = why
        guard retrying == nil else { return }
        let delay = backoff
        backoff = min(backoff * 2, .seconds(60))
        retrying = Task { [weak self] in
            try? await Task.sleep(for: delay)
            guard !Task.isCancelled, let self else { return }
            retrying = nil
            await flush()
        }
    }

    /// A record the Host wrote for us, or kept over our losing edit. An edit still queued for it
    /// now builds on its new seq, or the Host would take it for a clash with our own earlier
    /// write. Otherwise this copy takes it, or the newer record follow passed over meanwhile.
    /// Only `follow` moves `seq`, or a push from a copy that lags would skip every record it had
    /// not yet seen.
    private func acknowledge(_ record: Record) {
        if let index = queue.firstIndex(where: { $0.id == record.id }) {
            let change = queue[index]
            queue[index] = Change(id: change.id, kind: change.kind, body: change.body, base: record.seq, deleted: change.deleted)
            records[record.id]?.seq = record.seq
        } else if let shadow = shadows.removeValue(forKey: record.id), shadow.seq > record.seq {
            records[record.id] = shadow
        } else {
            records[record.id] = record
        }
    }
}
