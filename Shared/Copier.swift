import Foundation

/// Copies every adopted Agent session's Transcripts into its Vault while the app runs, so a
/// Conversation outlives its machine and the Agent's own clean-up (ADR 0008). A round asks each
/// machine's follower once for what the copies lack, and the Vault's once to store it (ADR 0020).
@MainActor
final class Copier {
    private static let round = Duration.seconds(30)

    private weak var vault: Vault?
    private var task: Task<Void, Never>?
    /// How long each Transcript copy in the Vault is, by session and file name, as the Vault's
    /// follower last said.
    private var copies: [String: [String: Int]] = [:]

    init(vault: Vault) { self.vault = vault }

    func stop() {
        task?.cancel()
        task = nil
    }

    func start() {
        guard task == nil else { return }
        task = Task { [weak self] in
            while !Task.isCancelled {
                await self?.copyAll()
                try? await Task.sleep(for: Self.round)
            }
        }
    }

    private func copyAll() async {
        guard let vault, vault.online else { return }
        await copy(vault.all(.session))
        await vault.mirror.update()
    }

    private struct Read: Decodable {
        struct Session: Decodable {
            struct File: Decodable {
                let path: String
                let from: Int?
                let data: String?
            }
            let id: String
            let transcript: String?
            let files: [File]?
        }
        let sessions: [Session]
    }

    private struct Stored: Decodable { let copies: [String: [String: Int]] }

    /// Brings these sessions' copies up to date now, as closing a Task does before it stops them.
    func copy(_ sessions: [Record]) async {
        guard let vault else { return }
        let sessions = sessions.filter { $0.body.pane != nil }
        let unknown = sessions.filter { copies[$0.id] == nil && TaskActions.machine($0.body.machine) != vault.machine }
        if !unknown.isEmpty { await store([], of: unknown.map(\.id), in: vault) }
        var appends: [[String: Any]] = []
        for (alias, all) in Dictionary(grouping: sessions, by: \.body.machine) {
            let machine = TaskActions.machine(alias)
            let local = machine == vault.machine
            // Without its copies' lengths a session would be read from the start, for nothing.
            let group = all.filter { local || copies[$0.id] != nil }
            guard !group.isEmpty else { continue }
            var request: [String: Any] = ["op": "transcripts", "sessions": group.map { session -> [String: Any] in
                var asked: [String: Any] = ["id": session.id, "session": session.body.pane ?? "", "copies": copies[session.id] ?? [:],
                                            "transcripts": vault.records[session.id]?.body.transcripts ?? []]
                (asked["name"], asked["path"]) = (session.body.name, session.body.path)
                return asked
            }]
            if local { request["vault"] = vault.folder }
            let ran = await machine.follower.ask(request)
            guard let read = try? JSONDecoder().decode(Read.self, from: Data(ran.out.utf8)) else { continue }
            for session in read.sessions {
                if let path = session.transcript, var record = vault.records[session.id], !(record.body.transcripts ?? []).contains(path) {
                    record.body.transcripts = (record.body.transcripts ?? []) + [path]
                    vault.write(record)
                }
                for file in session.files ?? [] where !local {
                    guard let data = file.data, let from = file.from else { continue }
                    appends.append(["id": session.id, "file": (file.path as NSString).lastPathComponent, "from": from, "data": data])
                }
            }
        }
        if !appends.isEmpty { await store(appends, of: [], in: vault) }
    }

    /// Hands the Vault's follower what the machines read, and learns how long every copy of these
    /// sessions now is, so the next round asks only for the rest.
    private func store(_ appends: [[String: Any]], of sessions: [String], in vault: Vault) async {
        let ran = await vault.machine.follower.ask(["op": "store", "dir": vault.folder, "sessions": sessions, "appends": appends])
        guard let stored = try? JSONDecoder().decode(Stored.self, from: Data(ran.out.utf8)) else { return }
        copies.merge(stored.copies) { $1 }
    }
}
