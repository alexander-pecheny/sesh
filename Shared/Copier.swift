import Foundation

/// Copies every adopted Agent session's Transcripts into its Vault while the app runs, so a
/// Conversation outlives its machine and the Agent's own clean-up (ADR 0008).
@MainActor
final class Copier {
    private static let round = Duration.seconds(30)
    /// Bytes moved per Transcript per round between two machines, so one huge file cannot
    /// hold up the rest.
    private static let chunk = 4 << 20

    private weak var vault: Vault?
    private var task: Task<Void, Never>?

    init(vault: Vault) { self.vault = vault }

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
    }

    /// Brings these sessions' copies up to date now, as closing a Task does before it stops them.
    func copy(_ sessions: [Record]) async {
        guard let vault else { return }
        for session in sessions {
            guard let pane = session.body.pane else { continue }
            let machine = TaskActions.machine(session.body.machine)
            var reported: String?
            if case .found(let found) = await Herdr.look(for: pane, on: machine) { reported = found.agent_session?.path }
            if let path = await reported.asyncOr({ await self.found(pane, on: machine) }),
               !(session.body.transcripts ?? []).contains(path) {
                var record = vault.records[session.id] ?? session
                record.body.transcripts = (record.body.transcripts ?? []) + [path]
                vault.write(record)
            }
            for path in vault.records[session.id]?.body.transcripts ?? [] {
                await copy(path, of: session, from: machine, into: vault)
            }
        }
    }

    /// herdr names Claude's session only by id; the helper finds its Transcript from that, and
    /// its first line says where.
    private func found(_ pane: String, on machine: Machine) async -> String? {
        struct Hello: Decodable { let transcript: String? }
        let ran = await machine.run("\(Helper.path) follow \(quote(pane)) --last 0 | head -n 1")
        let path = (try? JSONDecoder().decode(Hello.self, from: Data(ran.out.utf8)))?.transcript
        return path?.isEmpty == false ? path : nil
    }

    private func copy(_ path: String, of session: Record, from machine: Machine, into vault: Vault) async {
        let helper = "\(Helper.path) vault"
        if machine == vault.machine {
            _ = await machine.run("\(helper) copy \(vault.folder) \(session.id) --from \(quote(path))")
            return
        }
        struct Size: Decodable { let size: Int }
        let name = (path as NSString).lastPathComponent
        let have = await vault.machine.run("\(helper) size \(vault.folder) \(session.id) \(quote(name))")
        guard let size = try? JSONDecoder().decode(Size.self, from: Data(have.out.utf8)).size else { return }
        let read = await machine.run("tail -c +\(size + 1) \(quote(path)) | head -c \(Self.chunk) | base64 | tr -d '\\n'")
        guard read.ok, let bytes = Data(base64Encoded: read.out), !bytes.isEmpty else { return }
        let staged = "\(vault.folder)/inbox/\(UUID().uuidString.lowercased()).bytes"
        guard (await vault.machine.put(bytes, to: staged)).ok else { return }
        _ = await vault.machine.run(
            "\(helper) append \(vault.folder) \(session.id) \(quote(name)) --offset \(size) \(staged); rm -f \(staged)")
    }
}

private extension Optional where Wrapped == String {
    func asyncOr(_ fallback: () async -> String?) async -> String? {
        if let self { return self }
        return await fallback()
    }
}
