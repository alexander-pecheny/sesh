import Foundation

/// Every pane as the machines' followers last summarized it, read the way Sesh shows it: who
/// runs where, how each Agent is doing, and which Agents no Task has adopted. Keys are
/// `machine:pane`, with "" for the Mac.
struct Panes: Equatable {
    /// One session's summary as its machine's follower keeps it (ADR 0012).
    struct Summary: Decodable, Equatable {
        let t: String
        let session: String?
        var state: String?
        var agent: String?
        var cwd: String?
        var name: String?
        var done: UInt64?
        var last: String?
        var background: [SessionLog.Background]?
    }

    /// What sesh-mux last said of a pane: its status, and how many turns its Agent has finished.
    struct Live: Equatable {
        let status: String
        let done: UInt64
    }

    enum Mark: Int, Comparable {
        case seen, finished, background, working, waiting
        static func < (a: Mark, b: Mark) -> Bool { a.rawValue < b.rawValue }
    }

    /// An Agent session on one of the machines that no Task has adopted.
    struct Unfiled: Identifiable, Hashable {
        let machine: String?
        let pane: String
        let agent: Agent
        let cwd: String
        let name: String
        /// sesh-mux's name for the Agent, which its session keeps once adopted.
        let named: String?
        var id: String { "\(machine ?? ""):\(pane)" }
    }

    struct Occupant: Equatable {
        let name: String?
        let cwd: String?
    }

    var live: [String: Live] = [:]
    /// How much each idle Claude pane left running in the background.
    var busy: [String: Int] = [:]
    /// Claude panes whose turn is over, so sesh-mux's "working" there is only background work.
    var over: Set<String> = []
    var occupants: [String: Occupant] = [:]
    var unfiled: [String: [Unfiled]] = [:]
    /// When each pane's Transcript last had a message, in milliseconds since 1970.
    var last: [String: Double] = [:]

    init() {}

    /// `summaries` by machine and pane; an Agent none of `sessions` holds is Unfiled.
    init(_ summaries: [String: [String: Summary]], sessions: [Record]) {
        for (machine, panes) in summaries {
            for (pane, summary) in panes where summary.state != "ended" {
                occupants["\(machine):\(pane)"] = Occupant(name: summary.name, cwd: summary.cwd)
            }
        }
        let adopted = Set(sessions.map(key(of:)))
        for (machine, panes) in summaries {
            for (pane, summary) in panes {
                guard let state = summary.state, state != "ended" else { continue }
                let key = "\(machine):\(pane)"
                live[key] = Live(status: state == "background" ? "working" : state, done: summary.done ?? 0)
                busy[key] = summary.background?.count ?? 0
                if state == "background" { over.insert(key) }
                last[key] = summary.last.flatMap(Self.time)
                guard !adopted.contains(key), let agent = summary.agent.flatMap(Agent.init), let cwd = summary.cwd else { continue }
                unfiled[machine, default: []].append(Unfiled(
                    machine: machine.isEmpty ? nil : machine, pane: pane, agent: agent, cwd: cwd,
                    name: summary.name ?? (cwd as NSString).lastPathComponent, named: summary.name))
            }
        }
    }

    /// The summary a follower's line carries, if it is one.
    static func summary(_ line: String) -> Summary? {
        guard let summary = try? JSONDecoder().decode(Summary.self, from: Data(line.utf8)),
              summary.t == "session", summary.session != nil else { return nil }
        return summary
    }

    /// `machine:pane` while the pane runs the session's own Agent. sesh-mux gives a closed pane's id
    /// to the next, so a session whose pane holds another Agent keys nothing live.
    func key(of session: Record) -> String {
        let key = Self.place(of: session)
        guard let occupant = occupants[key], !Self.owns(session, name: occupant.name, cwd: occupant.cwd) else { return key }
        return key + "#replaced"
    }

    /// Whether the session's pane may still hold its Agent, as far as Sesh knows, so stopping it
    /// stops nothing else.
    func holds(_ session: Record) -> Bool {
        session.body.pane != nil && !key(of: session).hasSuffix("#replaced")
    }

    /// An Agent waiting on the user outranks one working, which outranks one finished unseen.
    func mark(of session: Record, seen: [String: UInt64]) -> Mark? {
        let key = key(of: session)
        guard let state = live[key] else { return nil }
        if state.status == "blocked" { return .waiting }
        let background = busy[key, default: 0] > 0
        if state.status == "working" { return background && over.contains(key) ? .background : .working }
        if background { return .background }
        return state.done > seen[key, default: state.done] ? .finished : .seen
    }

    /// The name sesh-mux gives the Agent a session recorded before Sesh kept names owns.
    func name(of session: Record) -> String? {
        guard session.body.name == nil, let occupant = occupants[Self.place(of: session)],
              Self.owns(session, name: occupant.name, cwd: occupant.cwd) else { return nil }
        return occupant.name
    }

    /// Whether the Agent named `name`, working in `cwd`, is the session's own rather than one
    /// sesh-mux put in its pane later: by name, or for a session recorded before Sesh kept
    /// names, by its folder.
    static func owns(_ session: Record, name: String?, cwd: String?) -> Bool {
        if let mine = session.body.name { return name == mine }
        guard let path = session.body.path, let cwd else { return true }
        return (path as NSString).lastPathComponent == (cwd as NSString).lastPathComponent
    }

    private static func place(of session: Record) -> String { "\(session.body.machine ?? ""):\(session.body.pane ?? "")" }

    private static let iso: ISO8601DateFormatter = {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return parser
    }()

    private static func time(_ text: String) -> Double? {
        (iso.date(from: text) ?? ISO8601DateFormatter().date(from: text)).map { $0.timeIntervalSince1970 * 1000 }
    }
}
