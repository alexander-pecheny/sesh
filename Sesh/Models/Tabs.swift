import GhosttyKit
import SwiftUI

/// The open Tabs. A Session keeps running while its Tab is not the visible one; only its
/// surface stops drawing, which `TerminalView` arranges when it leaves the window.
@MainActor
final class Tabs: ObservableObject {
    @Published private(set) var sessions: [SeshSession] = []
    @Published private(set) var activeID: UUID?

    var active: SeshSession? { sessions.first { $0.id == activeID } }

    func open(_ host: Host, store: Store, app: ghostty_app_t) {
        let session = SeshSession(host: host, store: store, app: app)
        sessions.append(session)
        activeID = session.id
    }

    func show(_ session: SeshSession) { activeID = session.id }

    func showHosts() { activeID = nil }

    func close(_ session: SeshSession) {
        session.close()
        sessions.removeAll { $0.id == session.id }
        if activeID == session.id { activeID = sessions.last?.id }
    }
}
