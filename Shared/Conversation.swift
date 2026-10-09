import SwiftUI

/// One Agent session as chat, built from the lines Sesh's transcript helper prints. The
/// helper has already turned every Agent's Transcript into the same entries (ADR 0006).
@MainActor
final class Conversation: ObservableObject {
    static let protocols: Set<Int> = [1, 2]
    /// The first protocol that can page back through a Transcript with `history`.
    private static let paging = 2
    private static let page = 50

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

    enum Item: Identifiable, Equatable {
        case entry(Entry)
        case switched(id: Int, reason: String)

        var id: String {
            switch self {
            case .entry(let entry): entry.id
            case .switched(let id, _): "switch-\(id)"
            }
        }
    }

    private struct Line: Decodable {
        let t: String
        let `protocol`: Int?
        let agent: String?
        let transcript: String?
        let state: String?
        let reason: String?
        let cursor: String?
        let id: String?
        let more: Bool?
        let status: String?
        let items: [Entry]?
        let replaces: String?
    }

    @Published private(set) var items: [Item] = []
    @Published private(set) var results: [String: Entry] = [:]
    @Published private(set) var todo: Entry?
    @Published private(set) var permissions: [Permission] = []
    /// What the Agent's screen shows that its Transcript does not hold yet, as entries shown
    /// after the rest until the Transcript's own take their places.
    @Published private(set) var live: [Entry] = []
    /// The status line on the Agent's screen.
    @Published private(set) var status = ""
    /// The live item whose row an entry took over, by the entry's id, so the row stays put.
    private(set) var rowKeys: [String: String] = [:]
    /// When each live item first showed, since the screen does not date it.
    private var firstSeen: [String: String] = [:]
    private static let dates: ISO8601DateFormatter = {
        let dates = ISO8601DateFormatter()
        dates.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return dates
    }()

    #if DEBUG
    /// `-liveLog PATH` appends each `live` line as the app takes it, timed, to measure how far
    /// the chat trails the terminal.
    private static let liveLog = UserDefaults.standard.string(forKey: "liveLog").flatMap { FileHandle(forWritingAtPath: $0) }

    private static func log(_ line: String) {
        liveLog?.seekToEndOfFile()
        liveLog?.write(Data("\(Date().timeIntervalSince1970)\t\(line)\n".utf8))
    }
    #endif

    /// The row an entry is drawn in: the live item's it replaced, else its own.
    func rowKey(_ id: String) -> String { rowKeys[id] ?? id }

    nonisolated static func isLive(_ id: String) -> Bool { id.hasPrefix("live.") }

    /// Everything the chat shows, in order: the Transcript's entries, then the live items.
    /// Claude saves a question, and the text before it, only once it is answered, so the
    /// question its hook reported stays last.
    var shown: [Item] {
        guard let question = openQuestion else { return items + live.map(Item.entry) }
        return items.dropLast() + live.map(Item.entry) + [question]
    }

    private var openQuestion: Item? {
        guard case .entry(let entry)? = items.last, entry.kind == "question", results[entry.id] == nil else { return nil }
        return items.last
    }

    /// What the Agent left running in the background, a command or a subagent.
    @Published private(set) var background: [Background] = []

    struct Background: Decodable, Equatable, Identifiable {
        let call: String
        let label: String
        /// A subagent, which has a Transcript of its own, rather than a command.
        var agent: Bool?
        var id: String { call }
    }

    /// The entry of the tool call that started `call`, if it is loaded.
    func entry(forCall call: String) -> String? {
        items.lazy.map(\.id).first { $0.hasSuffix(".call.\(call)") }
    }
    @Published private(set) var state = ""
    @Published private(set) var agent: Agent?
    /// Whether entries older than the first one shown can still be fetched.
    @Published private(set) var earlier = false
    @Published var problem: String?
    /// An image from a message, open on the whole screen.
    @Published var viewing: PlatformImage?
    /// The cards and thoughts the reader opened. Kept here, not in the rows, which the list
    /// rebuilds as the Agent works.
    @Published var opened: Set<String> = []

    /// Keyed by the row, which a live item's entry keeps when the Transcript replaces it.
    func isOpen(_ id: String) -> Binding<Bool> {
        let key = rowKey(id)
        return Binding { [weak self] in self?.opened.contains(key) == true } set: { [weak self] open in
            if open { self?.opened.insert(key) } else { self?.opened.remove(key) }
        }
    }

    /// What the user has picked and typed in a question card, by entry, until it is sent.
    var answers: [String: (picked: [Int: Set<String>], typed: [Int: String])] = [:]

    /// Where the entries come from: a live pane, or a Transcript copy in a Vault when the
    /// Agent session's machine is off.
    enum Source: Equatable {
        case pane(String)
        case file(path: String, agent: Agent)
    }

    let source: Source
    /// Opens a file the Agent named, relative paths taken from the Agent's folder; nil where
    /// there is nowhere to open it.
    var openPath: ((String) -> Void)?
    /// The repository the Agent works in, for linking `#213` to its pull request.
    @Published var repo: Repo?
    /// Keeps one entry as a Bookmark; nil where there is no Journal to keep it in.
    var bookmark: ((Entry) -> Void)?
    var copyLink: ((Entry) -> Void)?
    /// What is typed and not sent; it outlives the view, which goes with every switch of Task.
    var draft = "" { didSet { if draft != oldValue { saveDraft?(draft) } } }
    var saveDraft: ((String) -> Void)?
    /// Where the reader left the chat, kept while the app runs.
    let spot = ChatList.Spot()
    /// The entry a link or a search result asked to see, scrolled to and marked.
    /// The item a link pointed at, highlighted until it fades three seconds later.
    @Published var focus: String? {
        didSet {
            guard let focus, focus != oldValue else { return }
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .seconds(3))
                if self?.focus == focus { self?.focus = nil }
            }
        }
    }
    /// Opens the Transcript of the subagent a tool call started, given the call's id.
    var openSubagent: ((_ call: String, _ title: String) -> Void)?
    /// The Transcript the helper is reading now.
    private(set) var transcript: String?
    /// Whether the helper has sent the first batch of entries, which ends with a cursor.
    @Published private(set) var loaded = false
    private(set) weak var runner: Runner?
    private var cursor: String?
    private var loading = false
    private var expanded: Set<String> = []
    private var images: [String: PlatformImage] = [:]

    init(source: Source, agent: Agent?, runner: Runner?) {
        self.source = source
        self.agent = agent
        self.runner = runner
    }

    convenience init(pane: String, agent: Agent?, runner: Runner?) {
        self.init(source: .pane(pane), agent: agent, runner: runner)
    }

    /// The live pane, which only a running Agent session has.
    var pane: String? { if case .pane(let pane) = source { pane } else { nil } }

    private var target: String {
        switch source {
        case .pane(let pane): quote(pane)
        case .file(let path, let agent): "--file \(shellPath(path)) --agent \(agent.rawValue)"
        }
    }

    /// Follows until the screen goes away, picking up after the last cursor whenever the
    /// Link drops, so a reconnect neither repeats nor misses an entry.
    func follow() async {
        // The helper follows only a pane that runs an Agent, and a starting one does not yet.
        while starting, !Task.isCancelled { try? await Task.sleep(for: .milliseconds(200)) }
        if case .pane(let pane) = source { return await attach(pane) }
        // A helper still being installed would fail, and a failed follow is not retried.
        await prepare?()
        while !Task.isCancelled, let runner {
            let since = cursor.map { " --since \(quote($0))" } ?? ""
            let ended = await runner.stream("\(Helper.path) follow \(target)\(since)") { [weak self] in
                self?.apply($0)
            }
            guard !Task.isCancelled else { return }
            if ended.status == 0 { return problem = "This Agent session has ended." }
            if ended.status > 0 { return problem = ended.problem }
            try? await Task.sleep(for: .seconds(2))
        }
    }

    func apply(_ text: String) {
        let data = Data(text.utf8)
        guard let line = try? JSONDecoder().decode(Line.self, from: data) else { return }
        switch line.t {
        case "hello":
            agent = line.agent.flatMap(Agent.init) ?? agent
            transcript = line.transcript.flatMap { $0.isEmpty ? nil : $0 } ?? transcript
            if let number = line.protocol, !Self.protocols.contains(number) {
                problem = "The Host's helper speaks protocol \(number), which this Sesh does not know. Update Sesh."
            }
            earlier = (line.protocol ?? 0) >= Self.paging
        case "entry":
            guard let entry = try? JSONDecoder().decode(Entry.self, from: data) else { return }
            if let replaced = line.replaces, let index = live.firstIndex(where: { $0.id == replaced }) {
                rowKeys[entry.id] = replaced
                live.removeFirst(index + 1)
            }
            add(entry)
        case "state":
            state = line.state ?? state
            if state != "working", !waiting.isEmpty { Task { await sendQueued() } }
        case "switch":
            items.append(.switched(id: items.count, reason: line.reason ?? "other"))
            todo = nil
            earlier = false
        case "permission":
            guard let permission = try? JSONDecoder().decode(Permission.self, from: data) else { return }
            permissions.removeAll { $0.id == permission.id }
            permissions.append(permission)
        case "permission_done": permissions.removeAll { $0.id == line.id }
        case "live":
            #if DEBUG
            Self.log(text)
            #endif
            status = line.status ?? ""
            let now = Self.dates.string(from: Date())
            live = (line.items ?? []).map { item in
                var item = item
                item.at = firstSeen[item.id] ?? now
                firstSeen[item.id] = item.at
                return item
            }
        case "background":
            struct Tasks: Decodable { let tasks: [Background] }
            background = (try? JSONDecoder().decode(Tasks.self, from: data))?.tasks ?? []
        case "cursor":
            cursor = line.cursor
            // A new Agent session has no Transcript until its first message, so nothing is older;
            // every reconnect says hello again, so this is checked each time.
            if items.isEmpty { earlier = false }
            loaded = true
        default: break
        }
    }

    // MARK: The Session log (ADR 0010)

    private struct LogItem: Codable {
        let id: String
        let ord: Int
        let seq: Int
        let final: Bool
        let gone: Bool
        let entry: Entry
    }

    private struct LogLine: Decodable {
        let t: String
        let state: String?
        let status: String?
        let agent: String?
        let transcript: String?
        let background: [Background]?
        let permissions: [Permission]?
        let more: Bool?
    }

    private var logItems: [String: LogItem] = [:]
    private var logSeq = 0
    private var rebuilding = false
    /// Runs before the first attach, as a machine must have its helper first.
    var prepare: (() async -> Void)?

    /// Where this session's last items are kept on the device, so opening it shows them at
    /// once and asks the follower only for what changed since.
    var cache: URL? {
        didSet { loadCache() }
    }

    /// Each helper version keeps a log of its own, numbered and ordered its own way, so a
    /// cache is good only with the version that wrote it.
    private struct Cache: Codable {
        let seq: Int
        let items: [LogItem]
        var version: String?
    }
    private static let kept = 80
    private var saving = false

    private func loadCache() {
        guard let cache, let data = try? Data(contentsOf: cache),
              let kept = try? JSONDecoder().decode(Cache.self, from: data), kept.version == Helper.version else { return }
        logItems = Dictionary(kept.items.map { ($0.id, $0) }, uniquingKeysWith: { _, last in last })
        logSeq = kept.seq
        rebuild()
        loaded = true
        earlier = true
    }

    /// Written a second after the last change, not on every screen update.
    private func saveCache() {
        guard let cache, !saving else { return }
        saving = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            guard let self else { return }
            saving = false
            let items = logItems.values.sorted { $0.ord < $1.ord }.suffix(Self.kept)
            guard let data = try? JSONEncoder().encode(Cache(seq: logSeq, items: Array(items), version: Helper.version)) else { return }
            try? FileManager.default.createDirectory(at: cache.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? data.write(to: cache, options: .atomic)
        }
    }

    /// Items that arrive together, as the opening batch does, are shown in one go.
    private func scheduleRebuild() {
        guard !rebuilding else { return }
        rebuilding = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            rebuilding = false
            rebuild()
            saveCache()
        }
    }

    /// Follows the pane's Session log; after a drop it asks again from the last item it has.
    private func attach(_ pane: String) async {
        await prepare?()
        while !Task.isCancelled, let runner {
            let watch = logSeq > 0 ? "\(pane):\(logSeq)" : pane
            let ended = await runner.stream("\(Helper.path) attach --watch \(quote(watch))") { [weak self] in self?.applyLog($0) }
            guard !Task.isCancelled else { return }
            if ended.status > 0, !ended.problem.isEmpty { problem = ended.problem }
            try? await Task.sleep(for: .seconds(2))
        }
    }

    private func applyLog(_ text: String) {
        let data = Data(text.utf8)
        guard let line = try? JSONDecoder().decode(LogLine.self, from: data) else { return }
        switch line.t {
        case "item":
            guard let item = try? JSONDecoder().decode(LogItem.self, from: data) else { return }
            logSeq = max(logSeq, item.seq)
            if item.gone { logItems[item.id] = nil } else { logItems[item.id] = item }
            scheduleRebuild()
        case "session":
            if let state = line.state, state != self.state {
                self.state = state
                if state != "working", !waiting.isEmpty { Task { await sendQueued() } }
            }
            status = line.status ?? ""
            agent = line.agent.flatMap(Agent.init) ?? agent
            transcript = line.transcript ?? transcript
            background = line.background ?? []
            permissions = line.permissions ?? []
        case "opened":
            loaded = true
            earlier = !logItems.isEmpty
        default: break
        }
    }

    /// The log's items as the Conversation shows them: final ones in order, then the screen's,
    /// each row keyed by the item, so a screen row the Transcript takes over stays put.
    private func rebuild() {
        let sorted = logItems.values.sorted { $0.ord < $1.ord }
        var rows: [Item] = []
        var shownLive: [Entry] = []
        for item in sorted {
            var entry = item.entry
            switch entry.kind {
            case "result": if let call = entry.call { results[call] = entry }
            case "todo": todo = entry
            case "switch": rows.append(.switched(id: item.ord, reason: entry.summary))
            default:
                if item.final {
                    if item.id != entry.id { rowKeys[entry.id] = item.id }
                    if entry.kind == "user", let text = entry.text {
                        queued.removeAll { $0.handed && text.contains($0.text.trimmingCharacters(in: .whitespacesAndNewlines)) }
                    }
                    rows.append(.entry(entry))
                } else {
                    entry.at = entry.at ?? firstSeen[item.id] ?? Self.dates.string(from: Date())
                    firstSeen[item.id] = entry.at
                    shownLive.append(entry)
                }
            }
        }
        if rows != items { items = rows }
        if shownLive != live { live = shownLive }
    }

    /// One page of the log before its first item, read from the Transcript where needed.
    private func loadEarlierFromLog(_ pane: String) async {
        guard let runner, let first = logItems.values.map(\.ord).min() else { return earlier = false }
        let ran = await runner.run("\(Helper.path) page \(quote(pane)) --before \(first) --limit \(Self.page)")
        guard ran.ok else { return earlier = false }
        var more = false
        for text in ran.out.split(separator: "\n") {
            let data = Data(text.utf8)
            if let line = try? JSONDecoder().decode(LogLine.self, from: data), line.t == "page_done" { more = line.more ?? false }
            if let item = try? JSONDecoder().decode(LogItem.self, from: data), !item.gone { logItems[item.id] = item }
        }
        rebuild()
        saveCache()
        earlier = more
    }

    private func add(_ entry: Entry) {
        if entry.kind == "user", let text = entry.text {
            queued.removeAll { $0.handed && text.contains($0.text.trimmingCharacters(in: .whitespacesAndNewlines)) }
        }
        switch entry.kind {
        case "result": if let call = entry.call { results[call] = entry }
        case "todo": todo = entry
        default:
            if let index = items.firstIndex(where: { $0.id == entry.id }) {
                items[index] = .entry(entry)
            } else {
                items.insert(.entry(entry), at: items.count - (openQuestion == nil ? 0 : 1))
            }
        }
    }

    /// Pages back until `id` is loaded, then shows it. An id the Transcript no longer has
    /// leaves the Conversation at its oldest entry.
    func reveal(_ id: String) async {
        for _ in 0..<50 where !loaded { try? await Task.sleep(for: .milliseconds(200)) }
        while !items.contains(where: { $0.id == id }), earlier {
            let count = items.count
            await loadEarlier()
            if items.count == count { break }
        }
        focus = items.contains(where: { $0.id == id }) ? id : items.first?.id
    }

    /// Fetches the page of entries before the first one shown and puts it above.
    func loadEarlier() async {
        if case .pane(let pane) = source {
            guard earlier, !loading else { return }
            loading = true
            defer { loading = false }
            return await loadEarlierFromLog(pane)
        }
        guard earlier, !loading, let runner,
              let first = items.lazy.compactMap({ if case .entry(let entry) = $0 { entry } else { nil } }).first
        else { return }
        loading = true
        defer { loading = false }
        let ran = await runner.run("\(Helper.path) history \(target) --before \(quote(first.id)) --last \(Self.page)")
        guard ran.ok else { return earlier = false }
        var older: [Item] = []
        for text in ran.out.split(separator: "\n") {
            let data = Data(text.utf8)
            guard let line = try? JSONDecoder().decode(Line.self, from: data) else { continue }
            if line.t == "history" { earlier = line.more ?? false }
            guard line.t == "entry", let entry = try? JSONDecoder().decode(Entry.self, from: data) else { continue }
            switch entry.kind {
            case "result": if let call = entry.call { results[call] = results[call] ?? entry }
            case "todo": todo = todo ?? entry
            default: if !items.contains(where: { $0.id == entry.id }) { older.append(.entry(entry)) }
            }
        }
        items.insert(contentsOf: older, at: 0)
    }

    // MARK: Talking to the Agent

    private func run(_ command: String) async -> String? {
        guard let runner else { return nil }
        let ran = await runner.run(command)
        return ran.ok ? nil : ran.problem
    }

    /// While the Agent starts up, which can take seconds, messages wait in the queue.
    @Published var starting = false

    func send(_ text: String) async -> String? {
        guard let pane else { return "This Agent session is not running, so it cannot take a message." }
        guard state != "working", !starting else {
            queued.append(Queued(text: text))
            // As typed into its terminal: the Agent queues it itself and reads it at its next step.
            if !starting, agent != .pi { await hand() }
            return nil
        }
        return await run("herdr agent prompt \(quote(pane)) \(quote(text))")
    }

    /// A message written while the Agent works, held here until it finishes or the user
    /// decides, so it can still be taken back.
    struct Queued: Identifiable, Equatable {
        let id = UUID()
        let text: String
        var handed = false
    }

    @Published private(set) var queued: [Queued] = []
    private var sendingQueued = false

    /// Messages not yet handed to the Agent.
    var waiting: [Queued] { queued.filter { !$0.handed } }

    /// Also called by the Library while no view follows the Conversation.
    func sendQueued() async {
        guard !sendingQueued, !starting, !waiting.isEmpty else { return }
        sendingQueued = true
        defer { sendingQueued = false }
        await hand()
    }

    /// The Agent holds a handed message until it reads it, and only then writes it to the
    /// Transcript; it stays shown, as handed, until it is there.
    private func hand() async {
        guard let pane, !waiting.isEmpty else { return }
        let ids = Set(waiting.map(\.id))
        let text = waiting.map(\.text).joined(separator: "\n\n")
        mark(ids, handed: true)
        if let failed = await run("herdr agent prompt \(quote(pane)) \(quote(text))") {
            problem = failed
            mark(ids, handed: false)
        }
    }

    private func mark(_ ids: Set<UUID>, handed: Bool) {
        for index in queued.indices where ids.contains(queued[index].id) { queued[index].handed = handed }
    }

    /// Takes a queued message back, for the input box.
    func unqueue(_ message: Queued) -> String {
        queued.removeAll { $0 == message }
        return message.text
    }

    /// Stops the Agent's turn and has it read the queue at once. Claude does that itself for a
    /// message it holds, on ctrl+enter, so the message lands once; otherwise the queue goes after
    /// the stop, even while herdr reports "working" for Claude only waiting on its background agents.
    func interrupt() async {
        guard let pane else { return }
        if agent == .claude, queued.contains(where: \.handed) {
            problem = await run("herdr agent send-keys \(quote(pane)) ctrl+enter")
            return
        }
        await stop()
        try? await Task.sleep(for: .milliseconds(1500))
        await sendNow()
    }

    /// Picks a choice in a menu read off the Agent's screen.
    func choose(_ choice: Permission.Choice) async {
        guard let pane else { return }
        problem = await run("herdr pane send-keys \(quote(pane)) \(quote(choice.key))")
    }

    /// Hands the queue to the Agent at once; Claude takes a message mid-turn, or while it
    /// waits on background work, and reads it at its next step.
    func sendNow() async { await hand() }

    func stop() async {
        guard let pane else { return }
        problem = await run("herdr agent send-keys \(quote(pane)) esc")
    }

    /// One answer per question, in order: the labels picked, and any text typed instead.
    func answer(_ answers: [(options: [String], text: String)]) async -> String? {
        let json = answers.map { answer -> [String: Any] in
            var object: [String: Any] = ["options": answer.options]
            if !answer.text.isEmpty { object["text"] = answer.text }
            return object
        }
        let data = (try? JSONSerialization.data(withJSONObject: json)) ?? Data("[]".utf8)
        return await run("\(Helper.path) answer \(target) --json \(quote(String(decoding: data, as: UTF8.self)))")
    }

    func permit(_ allow: Bool) async {
        problem = await run("\(Helper.path) permit \(target) \(allow ? "allow" : "deny")")
    }

    /// The helper cuts long output down; the whole entry is fetched the first time it is opened.
    func expand(_ result: Entry) async {
        guard result.truncated == true, let runner, expanded.insert(result.id).inserted else { return }
        let ran = await runner.run("\(Helper.path) entry \(target) \(quote(result.id))")
        guard ran.ok, let entry = try? JSONDecoder().decode(Entry.self, from: Data(ran.out.utf8)) else {
            expanded.remove(result.id)
            return
        }
        add(entry)
    }

    func image(_ path: String) async -> PlatformImage? {
        if let known = images[path] { return known }
        guard let runner else { return Self.sample }
        let ran = await runner.run("base64 < \(quote(path)) | tr -d '\\n'")
        let image = Data(base64Encoded: ran.out).flatMap(PlatformImage.init(data:))
        images[path] = image
        return image
    }

    func upload(_ data: Data, ext: String) async -> String? { await runner?.upload(data, ext: ext) }

    #if os(iOS)
    func link() async -> URL? {
        guard let pane else { return nil }
        return await (runner as? Machine)?.link?.link(for: pane)
    }

    /// What a fixture's images look like, since there is no Host to fetch them from.
    private static let sample = UIGraphicsImageRenderer(size: CGSize(width: 300, height: 200)).image { context in
        let colours = [Catppuccin.Flavour.mocha(.mauve), Catppuccin.Flavour.mocha(.blue)].map { UIColor($0).cgColor }
        let gradient = CGGradient(colorsSpace: nil, colors: colours as CFArray, locations: nil)!
        context.cgContext.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: 300, y: 200), options: [])
    }
    #else
    func link() async -> URL? { nil }

    private static let sample: PlatformImage? = nil
    #endif
}
