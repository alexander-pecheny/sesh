import Foundation

/// The Mac's attached views of herdr panes, by Terminal record or by Agent session.
@MainActor
final class Terminals {
    var surfaces: [String: Ghostty.TerminalSurface] = [:]

    func forget(_ key: String) { surfaces[key] = nil }
}

extension Library {
    /// The live view of a pane: `herdr terminal attach`, on the Mac itself or on a Host's Link,
    /// over mosh where the Host is reached directly.
    func surface(for key: String, pane: String, on machine: Machine, gone: @escaping () -> Void) async -> Ghostty.TerminalSurface? {
        if let known = terminals.surfaces[key] { return known }
        // Only herdr saying the pane is gone closes its Tab; an unreachable machine is asked again.
        var terminal: String?
        while terminal == nil, !Task.isCancelled {
            switch await Herdr.look(for: pane, on: machine) {
            case .found(let found): terminal = found.terminal_id
            case .gone:
                gone()
                return nil
            case .unreachable: try? await Task.sleep(for: .seconds(3))
            }
        }
        guard let terminal else { return nil }
        let attach = Herdr.attach(terminal)
        if let known = terminals.surfaces[key] { return known }
        let surface: Ghostty.TerminalSurface
        if let link = machine.link {
            // The mosh-server the helper installer put on the Host.
            _ = await machine.prepare()
            let server = Helper.moshServer
            surface = Ghostty.TerminalSurface { [weak link] cols, rows, events in
                guard let link else { return nil }
                return try? await link.terminal(attach, server: server, cols: cols, rows: rows, events: events).get()
            }
        } else {
            // Ghostty starts commands with a bare PATH; the login shell has the user's.
            surface = Ghostty.TerminalSurface(command: "/bin/zsh -lic \(quote(attach))", folder: nil)
        }
        surface.pasteImage = { data, ext in await machine.upload(data, ext: ext) }
        // The attach ended: the pane is gone, or only the connection, and the next look attaches again.
        surface.onClose = { [weak self] in
            self?.terminals.surfaces[key] = nil
            Task {
                if case .gone = await Herdr.look(for: pane, on: machine) { gone() }
                self?.objectWillChange.send()
            }
        }
        terminals.surfaces[key] = surface
        return surface
    }

    func attached(_ key: String) -> Bool { terminals.surfaces[key] != nil }
}
