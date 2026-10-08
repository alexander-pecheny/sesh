import Foundation

/// The Mac's attached views of herdr panes, by Terminal record or by Agent session.
@MainActor
final class Terminals {
    var surfaces: [String: Ghostty.TerminalSurface] = [:]

    func forget(_ key: String) { surfaces[key] = nil }
}

extension Library {
    /// The live view of a pane: `herdr terminal attach` over mosh, started through the Host's
    /// shared ssh connection.
    func surface(for key: String, pane: String, on machine: Machine, gone: @escaping () -> Void) async -> Ghostty.TerminalSurface? {
        if let known = terminals.surfaces[key] { return known }
        // Only herdr saying the pane is gone closes its Tab; an unreachable machine is asked again.
        var terminal: String?
        while terminal == nil, !Task.isCancelled {
            switch await Herdr.look(for: pane, on: machine) {
            case .found(let found): terminal = found
            case .gone:
                gone()
                return nil
            case .unreachable: try? await Task.sleep(for: .seconds(3))
            }
        }
        guard let terminal else { return nil }
        let attach = "herdr terminal attach \(quote(terminal)) --takeover"
        let command: String
        if let alias = machine.alias {
            let remote = quote("exec \"$SHELL\" -lic \(quote(attach))")
            // mosh carries one screen per session, so each Terminal has its own; it starts its
            // server through the Host's one shared ssh connection, so no new one opens.
            let ssh = "ssh -o ControlMaster=auto -o ControlPath=\(Machine.controlPath) -o ControlPersist=600"
            // mosh-client looks the terminal type up locally, and knows no xterm-ghostty.
            command = "TERM=xterm-256color mosh --experimental-remote-ip=remote --ssh=\(quote(ssh)) \(alias) -- sh -c \(remote)"
        } else {
            command = attach
        }
        if let known = terminals.surfaces[key] { return known }
        // Ghostty starts commands with a bare PATH; the login shell has the user's.
        let surface = Ghostty.TerminalSurface(command: "/bin/zsh -lic \(quote(command))", folder: nil)
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
