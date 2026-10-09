import SwiftUI

/// One Agent session as chat, shown from its Session log.
@MainActor
final class Conversation: ObservableObject {
    private static let page = 50

    private var log = SessionLog()
    @Published private(set) var rows: [SessionLog.Row] = []
    @Published private(set) var results: [String: SessionLog.Entry] = [:]
    @Published private(set) var todo: SessionLog.Entry?
    @Published private(set) var permissions: [SessionLog.Permission] = []
    /// The status line on the Agent's screen.
    @Published private(set) var status = ""

    /// The row an entry is drawn in: the screen item's it took over, else its own.
    func rowKey(_ id: String) -> String { log.rowKey(id) }

    /// What the Agent left running in the background, a command or a subagent.
    @Published private(set) var background: [SessionLog.Background] = []

    /// The entry of the tool call that started `call`, if it is loaded.
    func entry(forCall call: String) -> String? {
        rows.lazy.flatMap(\.entries).map(\.id).first { $0.hasSuffix(".call.\(call)") }
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
    var bookmark: ((SessionLog.Entry) -> Void)?
    var copyLink: ((SessionLog.Entry) -> Void)?
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
    /// Whether the helper has sent the first batch of entries.
    @Published private(set) var loaded = false
    private(set) weak var runner: Runner?
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
            let since = log.cursor.map { " --since \(quote($0))" } ?? ""
            let ended = await runner.stream("\(Helper.path) follow \(target)\(since)") { [weak self] in self?.receive($0) }
            guard !Task.isCancelled else { return }
            if ended.status == 0 { return problem = "This Agent session has ended." }
            if ended.status > 0 { return problem = ended.problem }
            try? await Task.sleep(for: .seconds(2))
        }
    }

    /// Follows the pane's Session log (ADR 0010); after a drop it asks again from the last
    /// item it has.
    private func attach(_ pane: String) async {
        await prepare?()
        while !Task.isCancelled, let runner {
            let watch = log.seq > 0 ? "\(pane):\(log.seq)" : pane
            let ended = await runner.stream("\(Helper.path) attach --watch \(quote(watch))") { [weak self] in self?.receive($0) }
            guard !Task.isCancelled else { return }
            if ended.status > 0, !ended.problem.isEmpty { problem = ended.problem }
            try? await Task.sleep(for: .seconds(2))
        }
    }

    /// Takes one line the helper printed. Lines that arrive together, as the opening batch
    /// does, are shown in one go.
    func receive(_ line: String) {
        log.feed(line)
        guard !publishing else { return }
        publishing = true
        DispatchQueue.main.async { [weak self] in self?.publish() }
    }

    private var publishing = false
    private var savedSeq = 0

    private func publish() {
        publishing = false
        if rows != log.rows { rows = log.rows }
        if results != log.results { results = log.results }
        if todo != log.todo { todo = log.todo }
        if permissions != log.permissions { permissions = log.permissions }
        if background != log.background { background = log.background }
        if status != log.status { status = log.status }
        if earlier != log.earlier { earlier = log.earlier }
        if loaded != log.loaded { loaded = log.loaded }
        transcript = log.transcript ?? transcript
        if let named = log.agent.flatMap(Agent.init), named != agent { agent = named }
        if let failed = log.problem {
            problem = failed
            log.problem = nil
        }
        if state != log.state {
            state = log.state
            if state != "working", !waiting.isEmpty { Task { await sendQueued() } }
        }
        if queued.contains(where: \.handed) {
            let said = rows.flatMap(\.entries).filter { $0.kind == "user" && !SessionLog.isLive($0.id) }.compactMap(\.text)
            queued.removeAll { message in
                message.handed && said.contains { $0.contains(message.text.trimmingCharacters(in: .whitespacesAndNewlines)) }
            }
        }
        if log.seq != savedSeq {
            savedSeq = log.seq
            saveCache()
        }
    }

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
        let items: [SessionLog.Item]
        var version: String?
    }
    private static let kept = 80
    private var saving = false

    private func loadCache() {
        guard let cache, let data = try? Data(contentsOf: cache),
              let kept = try? JSONDecoder().decode(Cache.self, from: data), kept.version == Helper.version else { return }
        log.restore(kept.items, seq: kept.seq)
        savedSeq = kept.seq
        publish()
    }

    /// Written a second after the last change, not on every screen update.
    private func saveCache() {
        guard let cache, !saving else { return }
        saving = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            guard let self else { return }
            saving = false
            guard let data = try? JSONEncoder().encode(Cache(seq: log.seq, items: log.kept(Self.kept), version: Helper.version)) else { return }
            try? FileManager.default.createDirectory(at: cache.deletingLastPathComponent(), withIntermediateDirectories: true)
            try? data.write(to: cache, options: .atomic)
        }
    }

    /// Pages back until `id` is loaded, then shows it. An id the Transcript no longer has
    /// leaves the Conversation at its oldest entry.
    func reveal(_ id: String) async {
        for _ in 0..<50 where !loaded { try? await Task.sleep(for: .milliseconds(200)) }
        while !rows.contains(where: { $0.contains(id) }), earlier {
            let count = rows.count
            await loadEarlier()
            if rows.count == count { break }
        }
        focus = rows.contains(where: { $0.contains(id) }) ? id : rows.lazy.flatMap(\.entries).first?.id
    }

    /// Fetches the page of entries before the first one shown and puts it above.
    func loadEarlier() async {
        guard earlier, !loading, let runner else { return }
        guard let first = log.first else {
            log.earlier = false
            return publish()
        }
        loading = true
        defer { loading = false }
        let command = switch source {
        case .pane(let pane): "\(Helper.path) page \(quote(pane)) --before \(first.ord) --limit \(Self.page)"
        case .file: "\(Helper.path) history \(target) --before \(quote(first.entry.id)) --last \(Self.page)"
        }
        let ran = await runner.run(command)
        if ran.ok { log.page(ran.out) } else { log.earlier = false }
        publish()
        saveCache()
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
    func choose(_ choice: SessionLog.Permission.Choice) async {
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
    func expand(_ result: SessionLog.Entry) async {
        guard result.truncated == true, let runner, expanded.insert(result.id).inserted else { return }
        let ran = await runner.run("\(Helper.path) entry \(target) \(quote(result.id))")
        guard ran.ok, let entry = try? JSONDecoder().decode(SessionLog.Entry.self, from: Data(ran.out.utf8)) else {
            expanded.remove(result.id)
            return
        }
        log.expand(entry)
        publish()
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
