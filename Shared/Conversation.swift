import SwiftUI

/// One Agent session as chat, built from the lines Sesh's transcript helper prints. The
/// helper has already turned every Agent's Transcript into the same entries (ADR 0006).
@MainActor
final class Conversation: ObservableObject {
    static let protocols: Set<Int> = [1, 2]
    /// The first protocol that can page back through a Transcript with `history`.
    private static let paging = 2
    private static let page = 50

    struct Entry: Decodable, Identifiable, Equatable {
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

    struct Todo: Decodable, Equatable, Hashable {
        let text: String
        let status: String
    }

    struct Question: Decodable, Equatable {
        struct Option: Decodable, Equatable {
            let label: String
            let description: String?
        }
        let question: String
        let header: String?
        let multi: Bool?
        let options: [Option]
    }

    struct Permission: Decodable, Identifiable, Equatable {
        let id: String
        let tool: String
        let summary: String
        let command: String?
        let file: String?
        let reason: String?
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
        let text: String?
        let status: String?
    }

    @Published private(set) var items: [Item] = []
    @Published private(set) var results: [String: Entry] = [:]
    @Published private(set) var todo: Entry?
    @Published private(set) var permissions: [Permission] = []
    /// What the Agent's screen shows that its Transcript does not hold yet, and its status line.
    @Published private(set) var live = Live()

    struct Live: Equatable {
        var text = ""
        var status = ""
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
            live = Live()
        case "entry":
            if let entry = try? JSONDecoder().decode(Entry.self, from: data) { add(entry) }
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
        case "live": live = Live(text: line.text ?? "", status: line.status ?? "")
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
                items.append(.entry(entry))
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

    func send(_ text: String) async -> String? {
        guard let pane else { return "This Agent session is not running, so it cannot take a message." }
        guard state != "working" else {
            queued.append(Queued(text: text))
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
        guard !sendingQueued, !waiting.isEmpty else { return }
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

    /// Stops the Agent's turn and sends the queue once it has. herdr can go on reporting
    /// "working" while Claude only waits on its background agents, so the queue goes anyway.
    func interrupt() async {
        await stop()
        try? await Task.sleep(for: .milliseconds(1500))
        await sendNow()
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
