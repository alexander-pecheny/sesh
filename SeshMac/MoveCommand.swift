import SwiftUI

/// The app, or `Sesh vault move NAME --to ALIAS` run from a shell, which moves a Vault without a
/// window and exits once it is done.
@main
enum Entry {
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        guard args.starts(with: ["vault", "move"]) else { return SeshMacApp.main() }
        sesh_forget_session_environment()
        setvbuf(stdout, nil, _IOLBF, 0)
        Task { @MainActor in exit(await MoveCommand.run(Array(args.dropFirst(2)))) }
        dispatchMain()
    }
}

/// Moves a Vault as its menu does, printing each step and then what is left to do.
@MainActor
enum MoveCommand {
    static let usage = "usage: Sesh vault move NAME --to HOST [-helpers DIR]"

    static func run(_ args: [String]) async -> Int32 {
        guard let name = args.first, !name.hasPrefix("-"), let flag = args.firstIndex(of: "--to"), flag + 1 < args.count else {
            print(usage)
            return 2
        }
        let alias = args[flag + 1]
        var places = Library.places
        guard let index = places.firstIndex(where: { $0.name == name }) else {
            print("This Mac has no Vault named \(name).")
            return 1
        }
        let vault = Vault(places[index])
        vault.start()
        Task { await giveUpOnQuestions() }
        let outcome = await VaultMove(vault, to: .named(alias)) { print($0 + "…") }.run(beside: places)
        outcome.notes.forEach { print("- " + $0) }
        guard let place = outcome.place else {
            print("The Vault \(name) stays on \(places[index].alias ?? "this Mac").")
            return 1
        }
        places[index] = place
        Library.store(places)
        print("The Vault \(name) is on \(alias) now.")
        return 0
    }

    /// A Host that asks about its key or a password has no one to answer here.
    private static func giveUpOnQuestions() async {
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(1))
            Asking.shared.update()
            guard let link = Asking.shared.link else { continue }
            print("\(link.title) asks a question about its host key or login. Connect to it once in the Sesh app, then run this again.")
            exit(1)
        }
    }
}
