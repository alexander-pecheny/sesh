import Foundation
import GhosttyKit

/// The phone's attached views of herdr panes, by Terminal record or by Agent session: each an
/// ordinary Session to the pane's Host whose remote command is `herdr terminal attach`.
@MainActor
final class Terminals {
    var sessions: [String: SeshSession] = [:]

    func forget(_ key: String) { sessions.removeValue(forKey: key)?.close() }
}

extension Library {
    /// The Session showing `pane`, made on first look and kept while the Tab is open; nil
    /// with the reason when the pane is gone or its Host is not on this phone.
    func session(for key: String, pane: String, on machine: Machine, app: ghostty_app_t) async -> Result<SeshSession, Herdr.Failure> {
        if let known = terminals.sessions[key] { return .success(known) }
        guard let host = machine.host else { return .failure(Herdr.Failure("\(machine.title) is not reachable from the phone.")) }
        let terminal: String
        switch await Herdr.look(for: pane, on: machine) {
        case .found(let found): terminal = found
        case .gone: return .failure(Herdr.Failure("The pane is gone from \(machine.title)."))
        case .unreachable(let problem): return .failure(Herdr.Failure("Sesh could not reach \(machine.title): \(problem)"))
        }
        if let known = terminals.sessions[key] { return .success(known) }
        let session = SeshSession(host: host, store: Store.shared, app: app,
                                  command: "herdr terminal attach \(quote(terminal)) --takeover")
        terminals.sessions[key] = session
        return .success(session)
    }
}
