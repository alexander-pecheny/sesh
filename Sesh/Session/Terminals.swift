import Foundation
import GhosttyKit

/// The phone's attached views of sesh-mux panes, by Terminal record or by Agent session: each a
/// terminal on its Host's Link running `sesh-mux terminal attach`.
@MainActor
final class Terminals {
    var sessions: [String: SeshSession] = [:]

    func forget(_ key: String) { sessions.removeValue(forKey: key)?.close() }
}

extension Library {
    /// The Session showing `pane`, made on first look and kept while the Tab is open; nil
    /// with the reason when the pane is gone or its Host is not on this phone.
    func session(for key: String, pane: String, on machine: Machine, app: ghostty_app_t) async -> Result<SeshSession, Mux.Failure> {
        if let known = terminals.sessions[key] { return .success(known) }
        guard let link = machine.link else { return .failure(Mux.Failure("\(machine.title) is not reachable from the phone.")) }
        let terminal: String
        switch await Mux.look(for: pane, on: machine) {
        case .found(let found): terminal = found.terminal_id
        case .gone: return .failure(Mux.Failure("The pane is gone from \(machine.title)."))
        case .unreachable(let problem): return .failure(Mux.Failure("Sesh could not reach \(machine.title): \(problem)"))
        }
        let server = await machine.prepare() == nil ? Helper.moshServer : nil
        if let known = terminals.sessions[key] { return .success(known) }
        let session = SeshSession(link: link, app: app, command: Mux.attach(terminal), server: server)
        terminals.sessions[key] = session
        return .success(session)
    }
}
