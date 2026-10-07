import Foundation

/// The Mac's attached views of herdr panes, by Terminal record or by Agent session.
@MainActor
final class Terminals {
    var surfaces: [String: Ghostty.TerminalSurface] = [:]
    static var mosh: String??

    func forget(_ key: String) { surfaces[key] = nil }
}

extension Library {
    /// The live view of a pane: `herdr terminal attach` over mosh, or ssh where there is no
    /// mosh, so it survives sleep and a changing network as the phone's Sessions do.
    func surface(for key: String, pane: String, on machine: Machine, gone: @escaping () -> Void) async -> Ghostty.TerminalSurface? {
        if let known = terminals.surfaces[key] { return known }
        guard let terminal = await Herdr.terminal(of: pane, on: machine) else {
            gone()
            return nil
        }
        let attach = "herdr terminal attach \(quote(terminal)) --takeover"
        let command: String
        if let alias = machine.alias {
            let remote = quote("exec \"$SHELL\" -lic \(quote(attach))")
            if Terminals.mosh == nil {
                let found = (await Machine.mac.run("command -v mosh")).out.trimmingCharacters(in: .whitespacesAndNewlines)
                Terminals.mosh = .some(found.isEmpty ? nil : found)
            }
            if let mosh = Terminals.mosh ?? nil {
                // mosh-client looks the terminal type up locally, and knows no xterm-ghostty.
                command = "TERM=xterm-256color \(quote(mosh)) \(alias) -- sh -c \(remote)"
            } else {
                command = "/usr/bin/ssh -t -o ControlPath=none \(alias) \(remote)"
            }
        } else {
            command = attach
        }
        if let known = terminals.surfaces[key] { return known }
        // Ghostty starts commands with a bare PATH; the login shell has the user's, which mosh
        // needs to find its client and ssh.
        let surface = Ghostty.TerminalSurface(command: "/bin/zsh -lic \(quote(command))", folder: nil)
        // The attach ended: the pane is gone, or only the connection, and the next look attaches again.
        surface.onClose = { [weak self] in
            self?.terminals.surfaces[key] = nil
            Task {
                let still = await machine.run("herdr pane get \(quote(pane))")
                if !still.ok { gone() }
                self?.objectWillChange.send()
            }
        }
        terminals.surfaces[key] = surface
        return surface
    }

    func attached(_ key: String) -> Bool { terminals.surfaces[key] != nil }
}
