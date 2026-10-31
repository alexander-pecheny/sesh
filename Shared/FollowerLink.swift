import Foundation
import Network
import OSLog

/// A device's way to one machine's follower (ADR 0012): every session's summary, the Session
/// logs it watches, their earlier pages, and what the user sends an Agent. Each stream comes
/// back after a drop, from where it left off, and is refused if the helper speaks another
/// protocol.
@MainActor
final class FollowerLink {
    nonisolated static let `protocol` = 5
    /// Every stream's start, end and retry, and every catch-up, kept by the system for days:
    /// `log show --last 1d --predicate 'subsystem == "me.pecheny.sesh" AND category == "sync"'`.
    nonisolated static let log = Logger(subsystem: "me.pecheny.sesh", category: "sync")

    private unowned let machine: Runner
    private let pause: Duration
    private let online: () -> Bool

    /// Where the machine's streams can be written to, one stream carries every session's
    /// summary and each watched session's items too, so opening a session costs a line to
    /// the follower rather than a stream of its own.
    private var hub: Task<Void, Never>?
    private var ask: ((String) -> Void)?
    private var members = 0
    private var summaries: (online: (Bool) -> Void, line: (String) -> Void)?
    private var watchers: [String: Watcher] = [:]
    private var vaults: [String: VaultWatcher] = [:]

    private struct VaultWatcher {
        let from: () -> Int
        let online: (Bool) -> Void
        let line: (String) -> Void
    }

    private struct Watcher {
        let from: () -> Int
        let attaching: () -> Void
        let line: (String) -> Void
        let problem: (String) -> Void
    }

    private struct Routed: Decodable {
        let session: String?
        let vault: String?
    }

    init(_ machine: Runner, pause: Duration = .seconds(2), online: @escaping () -> Bool = Reachability.online) {
        self.machine = machine
        self.pause = pause
        self.online = online
        _ = online()
    }

    /// How many drops in a row, the network up, it takes before the user hears of them.
    nonisolated static let patience = 5

    /// Every session's summary, until the calling task is cancelled; `online` says whenever the
    /// follower starts or stops answering.
    func sessions(online: @escaping (Bool) -> Void, line: @escaping (String) -> Void) async {
        if machine.talks {
            summaries = (online, line)
            await member()
            summaries = nil
            return
        }
        await keep({ "\(Helper.path) attach --sessions" }, hello: { online(true) }, problem: { _ in }, line: line) { _ in
            online(false)
            return false
        }
    }

    /// One session's Session log, its items from the last number `seq` says is held.
    /// `attaching` is told each time, the first included, before the follower is asked again.
    func watch(_ session: String, from seq: @escaping () -> Int, attaching: @escaping () -> Void = {},
               line: @escaping (String) -> Void, problem: @escaping (String) -> Void) async {
        if machine.talks {
            let watcher = Watcher(from: seq, attaching: attaching, line: line, problem: problem)
            watchers[session] = watcher
            ask.map { ask in Self.watch(session, with: watcher, over: ask) }
            await member()
            watchers[session] = nil
            ask?(Self.request(["op": "unwatch", "session": session]))
            return
        }
        await keep({ "\(Helper.path) attach --watch \(quote(seq() > 0 ? "\(session):\(seq())" : session))" },
                   attaching: attaching, problem: problem, line: line) { ended in
            if ended.status > 0, !ended.lost, !ended.problem.isEmpty { problem(ended.problem) }
            return false
        }
    }

    /// The Vault in `folder`, its records after the number `seq` says is held, over the
    /// machine's one stream where it can be written to; `online` says whenever that stream
    /// starts or stops. Returns why it ended, nil once the calling task is cancelled.
    func vault(_ folder: String, from seq: @escaping () -> Int, online: @escaping (Bool) -> Void,
               line: @escaping (String) -> Void) async -> String? {
        guard machine.talks else {
            online(true)
            let ended = await machine.stream("\(Helper.path) vault follow \(folder) --since \(seq())", line: line)
            return ended.ok || Task.isCancelled ? nil : ended.problem
        }
        let watcher = VaultWatcher(from: seq, online: online, line: line)
        vaults[folder] = watcher
        if let ask {
            online(true)
            Self.follow(folder, with: watcher, over: ask)
        }
        await member()
        vaults[folder] = nil
        ask?(Self.request(["op": "unvault", "dir": folder]))
        return nil
    }

    /// A Transcript file's entries from `cursor`, read by the helper without the follower; it
    /// ends with the Agent session.
    func follow(file path: String, agent: Agent, from cursor: @escaping () -> String?, line: @escaping (String) -> Void,
                problem: @escaping (String) -> Void) async {
        let file = "\(Helper.path) follow --file \(shellPath(path)) --agent \(agent.rawValue)"
        await keep({ file + (cursor().map { " --since \(quote($0))" } ?? "") }, follower: false, problem: problem, line: line) { ended in
            if ended.status == 0 { problem("This Agent session has ended.") } else if ended.status > 0, !ended.lost { problem(ended.problem) }
            return ended.status >= 0 && !ended.lost
        }
    }

    func page(_ session: String, before ord: Int, limit: Int) async -> Ran {
        await machine.run("\(Helper.path) page \(quote(session)) --before \(ord) --limit \(limit)")
    }

    /// A message under the id the device gave it, which the follower's item for it keeps (ADR 0015).
    func send(_ text: String, id: String, to session: String) async -> String? { await ask("send", session, [id, text]) }
    /// Takes back a message the Agent was not given, or did not take.
    func unqueue(_ id: String, in session: String) async -> String? { await ask("unqueue", session, [id]) }
    /// Gives the queued messages to the Agent now.
    func hand(_ session: String) async -> String? { await ask("hand", session, []) }
    func keys(_ keys: [String], to session: String) async -> String? { await ask("keys", session, keys) }
    /// One answer per question, as JSON: the labels picked, and any text typed instead.
    func answer(_ answers: String, in session: String) async -> String? { await ask("answer", session, ["--json", answers]) }
    func permit(_ allow: Bool, in session: String) async -> String? { await ask("permit", session, [allow ? "allow" : "deny"]) }
    /// Interrupts the Agent and closes its pane.
    func stop(_ session: String) async -> String? { await ask("stop", session, []) }

    private func ask(_ op: String, _ session: String, _ args: [String]) async -> String? {
        if let problem = await machine.prepare() { return problem }
        let ran = await machine.run(([Helper.path, op] + ([session] + args).map(quote)).joined(separator: " "))
        return ran.ok ? nil : ran.problem
    }

    private struct Said: Decodable {
        let t: String
        let `protocol`: Int?
        let message: String?

        /// Only the follower's own hello and error lines, found without decoding every item.
        init?(_ text: String) {
            guard text.contains(#""t":"hello""#) || text.contains(#""t":"error""#),
                  let said = try? JSONDecoder().decode(Self.self, from: Data(text.utf8)) else { return nil }
            self = said
        }
    }

    /// Holds the hub up until the calling task is cancelled.
    private func member() async {
        members += 1
        if hub == nil { hub = Task { await carry() } }
        while !Task.isCancelled { try? await Task.sleep(for: .seconds(3600)) }
        members -= 1
        guard members == 0, let stopping = hub else { return }
        hub = nil
        stopping.cancel()
        await stopping.value
    }

    /// The hub's stream, which asks again for every watched session each time it comes back.
    private func carry() async {
        await keep({ "\(Helper.path) attach --sessions" }, opened: { [weak self] ask in
            guard let self else { return }
            self.ask = ask
            for (session, watcher) in watchers { Self.watch(session, with: watcher, over: ask) }
            for (folder, watcher) in vaults { Self.follow(folder, with: watcher, over: ask) }
        }, hello: { [weak self] in
            self?.summaries?.online(true)
            self?.vaults.values.forEach { $0.online(true) }
        },
        problem: { [weak self] problem in self?.watchers.values.forEach { $0.problem(problem) } },
        line: { [weak self] text in self?.route(text) }) { [weak self] ended in
            self?.ask = nil
            self?.summaries?.online(false)
            self?.vaults.values.forEach { $0.online(false) }
            if ended.status > 0, !ended.lost, !ended.problem.isEmpty { self?.watchers.values.forEach { $0.problem(ended.problem) } }
            return false
        }
    }

    private static func watch(_ session: String, with watcher: Watcher, over ask: (String) -> Void) {
        watcher.attaching()
        ask(request(["op": "watch", "session": session, "since": watcher.from()]))
    }

    private static func follow(_ folder: String, with watcher: VaultWatcher, over ask: (String) -> Void) {
        ask(request(["op": "vault", "dir": folder, "since": watcher.from()]))
    }

    private static func request(_ fields: [String: Any]) -> String {
        (try? JSONSerialization.data(withJSONObject: fields)).map { String(decoding: $0, as: UTF8.self) } ?? ""
    }

    /// The follower's hello goes to everyone; a line about one session or Vault to it alone.
    private func route(_ text: String) {
        if text.contains(#""vault":"#), let folder = (try? JSONDecoder().decode(Routed.self, from: Data(text.utf8)))?.vault {
            return vaults[folder]?.line(text) ?? ()
        }
        summaries?.line(text)
        if text.contains(#""t":"hello""#) { return watchers.values.forEach { $0.line(text) } }
        guard let session = (try? JSONDecoder().decode(Routed.self, from: Data(text.utf8)))?.session else { return }
        watchers[session]?.line(text)
    }

    /// Runs `command` again after every drop until the calling task is cancelled or `ended`
    /// says the stream is over, waiting longer each time the machine did not answer. A drop
    /// becomes a problem only once it repeats `patience` times with the network up.
    private func keep(_ command: @escaping () -> String, follower: Bool = true,
                      opened: ((@escaping (String) -> Void) -> Void)? = nil, hello: @escaping () -> Void = {},
                      attaching: () -> Void = {}, problem: @escaping (String) -> Void, line: @escaping (String) -> Void,
                      ended: (Ran) -> Bool) async {
        var wait = pause, drops = 0, told = false
        while !Task.isCancelled {
            attaching()
            _ = await machine.prepare()
            var heard = false, refused = false
            let text = command(), carrier = machine.carrier ?? "local", started = ContinuousClock.now
            Self.log.notice("\(self.machine.title, privacy: .public) over \(carrier, privacy: .public): \(text, privacy: .public)")
            let heed: (String) -> Void = { text in
                guard !refused else { return }
                heard = true
                if follower, let said = Said(text) {
                    if said.t == "error" { return problem(said.message ?? "The follower failed.") }
                    guard said.protocol == Self.protocol else {
                        refused = true
                        return problem(SessionLog.unknown(said.protocol ?? 0))
                    }
                    hello()
                }
                line(text)
            }
            let ran = if let opened { await machine.stream(text, opened: opened, line: heed) } else { await machine.stream(text, line: heed) }
            let lasted = ContinuousClock.now - started, said = heard ? "after output" : "before any output"
            Self.log.notice("\(self.machine.title, privacy: .public) stream ended \(said, privacy: .public) after \(lasted, privacy: .public), status \(ran.status): \(ran.err, privacy: .public)")
            drops = ran.lost ? (heard ? 1 : drops + 1) : 0
            if drops < Self.patience { told = false } else if !told, online() {
                told = true
                problem(ran.problem)
            }
            guard !Task.isCancelled, !refused, !ended(ran) else { return }
            wait = heard ? pause : min(wait * 2, pause * 4)
            Self.log.notice("\(self.machine.title, privacy: .public) stream retries in \(wait, privacy: .public)")
            try? await Task.sleep(for: wait)
        }
    }
}

/// Whether the device has a working network, as the system last said.
enum Reachability {
    private static let monitor = {
        let monitor = NWPathMonitor()
        monitor.start(queue: DispatchQueue(label: "me.pecheny.sesh.network"))
        return monitor
    }()

    static func online() -> Bool { monitor.currentPath.status == .satisfied }
}
