import Foundation

/// A device's way to one machine's follower (ADR 0012): every session's summary, the Session
/// logs it watches, their earlier pages, and what the user sends an Agent. Each stream comes
/// back after a drop, from where it left off, and is refused if the helper speaks another
/// protocol.
@MainActor
final class FollowerLink {
    nonisolated static let `protocol` = 5

    private unowned let machine: Runner
    private let pause: Duration

    init(_ machine: Runner, pause: Duration = .seconds(2)) {
        self.machine = machine
        self.pause = pause
    }

    /// Every session's summary, until the calling task is cancelled; `online` says whenever the
    /// follower starts or stops answering.
    func sessions(online: @escaping (Bool) -> Void, line: @escaping (String) -> Void) async {
        await keep({ "\(Helper.path) attach --sessions" }, hello: { online(true) }, problem: { _ in }, line: line) { _ in
            online(false)
            return false
        }
    }

    /// One session's Session log, its items from the last number `seq` says is held.
    func watch(_ session: String, from seq: @escaping () -> Int, line: @escaping (String) -> Void,
               problem: @escaping (String) -> Void) async {
        await keep({ "\(Helper.path) attach --watch \(quote(seq() > 0 ? "\(session):\(seq())" : session))" },
                   problem: problem, line: line) { ended in
            if ended.status > 0, !ended.problem.isEmpty { problem(ended.problem) }
            return false
        }
    }

    /// A Transcript file's entries from `cursor`, read by the helper without the follower; it
    /// ends with the Agent session.
    func follow(file path: String, agent: Agent, from cursor: @escaping () -> String?, line: @escaping (String) -> Void,
                problem: @escaping (String) -> Void) async {
        let file = "\(Helper.path) follow --file \(shellPath(path)) --agent \(agent.rawValue)"
        await keep({ file + (cursor().map { " --since \(quote($0))" } ?? "") }, follower: false, problem: problem, line: line) { ended in
            if ended.status == 0 { problem("This Agent session has ended.") } else if ended.status > 0 { problem(ended.problem) }
            return ended.status >= 0
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

    /// Runs `command` again after every drop until the calling task is cancelled or `ended`
    /// says the stream is over, waiting longer each time the machine did not answer.
    private func keep(_ command: @escaping () -> String, follower: Bool = true, hello: @escaping () -> Void = {},
                      problem: @escaping (String) -> Void, line: @escaping (String) -> Void, ended: (Ran) -> Bool) async {
        var wait = pause
        while !Task.isCancelled {
            _ = await machine.prepare()
            var heard = false, refused = false
            let ran = await machine.stream(command()) { text in
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
            guard !Task.isCancelled, !refused, !ended(ran) else { return }
            wait = heard ? pause : min(wait * 2, pause * 4)
            try? await Task.sleep(for: wait)
        }
    }
}
