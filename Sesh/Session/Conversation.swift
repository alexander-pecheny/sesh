import SwiftUI

/// One Agent session as chat, built from the lines `herdr agent follow` prints. herdr has
/// already turned every Agent's Transcript into the same entries (ADR 0005).
@MainActor
final class Conversation: ObservableObject {
    static let protocols: Set<Int> = [1]

    struct Entry: Decodable, Identifiable, Equatable {
        let id: String
        let kind: String
        let summary: String
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
        let state: String?
        let reason: String?
        let cursor: String?
        let id: String?
    }

    @Published private(set) var items: [Item] = []
    @Published private(set) var results: [String: Entry] = [:]
    @Published private(set) var todo: Entry?
    @Published private(set) var permissions: [Permission] = []
    @Published private(set) var state = ""
    @Published private(set) var agent: Agent?
    @Published var problem: String?

    let pane: String
    private(set) weak var projects: Projects?
    private var cursor: String?
    private var expanded: Set<String> = []
    private var images: [String: UIImage] = [:]

    init(pane: String, agent: Agent?, projects: Projects?) {
        self.pane = pane
        self.agent = agent
        self.projects = projects
    }

    /// Follows until the screen goes away, picking up after the last cursor whenever the
    /// Link drops, so a reconnect neither repeats nor misses an entry.
    func follow() async {
        while !Task.isCancelled, let projects {
            let since = cursor.map { " --since \(quote($0))" } ?? ""
            let ended = await projects.stream("herdr agent follow \(quote(pane))\(since)") { [weak self] in
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
            if let number = line.protocol, !Self.protocols.contains(number) {
                problem = "herdr speaks protocol \(number), which this Sesh does not know. Update Sesh."
            }
        case "entry":
            if let entry = try? JSONDecoder().decode(Entry.self, from: data) { add(entry) }
        case "state": state = line.state ?? state
        case "switch":
            items.append(.switched(id: items.count, reason: line.reason ?? "other"))
            todo = nil
        case "permission":
            guard let permission = try? JSONDecoder().decode(Permission.self, from: data) else { return }
            permissions.removeAll { $0.id == permission.id }
            permissions.append(permission)
        case "permission_done": permissions.removeAll { $0.id == line.id }
        case "cursor": cursor = line.cursor
        default: break
        }
    }

    private func add(_ entry: Entry) {
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

    // MARK: Talking to the Agent

    private func run(_ command: String) async -> String? {
        guard let projects else { return nil }
        let ran = await projects.run(command)
        return ran.ok ? nil : ran.problem
    }

    func send(_ text: String) async -> String? {
        await run("herdr agent prompt \(quote(pane)) \(quote(text))")
    }

    func stop() async {
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
        return await run("herdr agent answer \(quote(pane)) --json \(quote(String(decoding: data, as: UTF8.self)))")
    }

    func permit(_ allow: Bool) async {
        problem = await run("herdr agent permit \(quote(pane)) \(allow ? "allow" : "deny")")
    }

    /// herdr cuts long output down; the whole entry is fetched the first time it is opened.
    func expand(_ result: Entry) async {
        guard result.truncated == true, let projects, expanded.insert(result.id).inserted else { return }
        let ran = await projects.run("herdr agent entry \(quote(pane)) \(quote(result.id))")
        guard ran.ok, let entry = try? JSONDecoder().decode(Entry.self, from: Data(ran.out.utf8)) else {
            expanded.remove(result.id)
            return
        }
        add(entry)
    }

    func image(_ path: String) async -> UIImage? {
        if let known = images[path] { return known }
        guard let projects else { return Self.sample }
        let ran = await projects.run("base64 < \(quote(path)) | tr -d '\\n'")
        let image = Data(base64Encoded: ran.out).flatMap(UIImage.init)
        images[path] = image
        return image
    }

    func link() async -> URL? { await projects?.link(for: pane) }

    /// What a fixture's images look like, since there is no Host to fetch them from.
    private static let sample = UIGraphicsImageRenderer(size: CGSize(width: 300, height: 200)).image { context in
        let colours = [Catppuccin.Flavour.mocha(.mauve), Catppuccin.Flavour.mocha(.blue)].map { UIColor($0).cgColor }
        let gradient = CGGradient(colorsSpace: nil, colors: colours as CFArray, locations: nil)!
        context.cgContext.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: 300, y: 200), options: [])
    }
}
