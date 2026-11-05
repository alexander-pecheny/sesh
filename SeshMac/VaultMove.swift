import Foundation

/// Moves a Vault to another Host (ADR 0024): its database and Transcript copies, its open Tasks'
/// Worktrees with their uncommitted work, and its Agent sessions, which come back ended there.
/// Everything travels through this Mac, since the two Hosts may not reach each other. The Vault
/// menu and `Sesh vault move` both run it.
@MainActor
final class VaultMove {
    struct Outcome {
        /// Where the Vault is now; nil when it stayed where it was.
        var place: Vault.Place?
        /// What the user should know or still do, in plain sentences.
        var notes: [String] = []
    }

    private static let chunk = 4 << 20

    private let vault: Vault
    private let target: Machine
    private let step: (String) -> Void
    private var notes: [String] = []
    private var made = false
    private let token = UUID().uuidString.lowercased()

    private var source: Machine { vault.machine }
    private var from: String { source.title }
    private var to: String { target.title }
    private var outbox: String { "~/.sesh/outbox/\(token)" }
    private var inbox: String { "~/.sesh/inbox/\(token)" }
    private var marker: String { "\(vault.folder)/moved.json" }

    init(_ vault: Vault, to target: Machine, step: @escaping (String) -> Void) {
        self.vault = vault
        self.target = target
        self.step = step
    }

    /// `places` are every Vault this Mac knows, since a Host is in one Vault at most.
    func run(beside places: [Vault.Place]) async -> Outcome {
        guard let alias = vault.place.alias else { return refuse("A Vault kept on this Mac cannot move yet.") }
        guard let destination = target.alias, destination != alias else { return refuse("Choose a Host other than \(from), where the Vault is now.") }
        if let other = places.first(where: { $0.alias == destination && $0.name != vault.name }) {
            return refuse("\(to) holds the Vault \(other.name) already, and a Host is in one Vault at most.")
        }
        step("Preparing \(from) and \(to)")
        for machine in [source, target] {
            if let problem = await machine.prepare() { return refuse(problem) }
        }
        if (await target.run("[ -e \(shellPath(vault.folder + "/vault.db")) ]")).ok {
            return refuse("\(to) has a Vault named \(vault.name) already.")
        }
        guard let oldHome = await home(of: source), let newHome = await home(of: target) else {
            return refuse("Sesh could not read the home folders on \(from) and \(to).")
        }
        let move = Move(from: alias, to: destination, oldHome: oldHome, newHome: newHome)
        await stop(Move.plan(Array(vault.records.values), from: alias))
        step("Sending this Mac's changes to \(from)")
        guard await vault.settle(within: .seconds(60)) else {
            return refuse("Sesh could not send this Mac's changes to \(from), so the Vault stays there, and its Agents there, "
                + "which Sesh had stopped, can be resumed. \(vault.problem ?? "")")
        }
        var moved = Moved.resolved((await Machine.mac.run("ssh -G \(quote(destination))")).out, alias: destination)
        moved.done = false
        guard await mark(moved) else { return refuse("Sesh could not mark the Vault on \(from) as moving, so it stays there.") }
        let carried = await carry(move)
        _ = await source.run("rm -rf \(shellPath(outbox))")
        _ = await target.run("rm -rf \(shellPath(inbox))")
        guard carried else {
            _ = await source.run("rm -f \(shellPath(marker))")
            if made { _ = await target.run("rm -rf \(shellPath(vault.folder))") }
            return Outcome(notes: notes)
        }
        moved.done = true
        if !(await mark(moved)) { notes.append("Sesh could not mark the Vault on \(from) as moved, so other devices will not follow it to \(to) by themselves.") }
        if let config = TaskActions.claudeConfig(vault, on: source) {
            TaskActions.setClaudeConfig(Move.portable(config), vault, on: target)
        }
        return Outcome(place: .init(name: vault.name, alias: destination), notes: notes)
    }

    private func refuse(_ why: String) -> Outcome { Outcome(notes: notes + [why]) }

    private func fail(_ why: String) -> Bool {
        notes.append(why)
        return false
    }

    private func home(of machine: Machine) async -> String? {
        let ran = await machine.run("printf %s \"$HOME\"")
        return ran.ok && !ran.out.isEmpty ? ran.out : nil
    }

    private func mark(_ moved: Moved) async -> Bool {
        guard let data = try? JSONEncoder().encode(moved) else { return false }
        return (await source.put(data, to: marker)).ok
    }

    /// Takes the last of each running Agent's Transcript into the Vault, stops it, and takes what
    /// it wrote while stopping, so the old Host has no writer left. Terminals close with it.
    private func stop(_ plan: Move.Plan) async {
        let running = plan.sessions.filter { $0.body.pane != nil }
        step("Copying Transcripts and stopping the Agents on \(from)")
        await vault.copier.copy(running)
        for session in running {
            guard let pane = session.body.pane, case .found(let found) = await Mux.look(for: pane, on: source), found.agent != nil,
                  session.body.name == nil || found.name == session.body.name else { continue }
            _ = await source.follower.stop(pane)
        }
        await vault.copier.copy(running)
        for pane in plan.terminals.compactMap(\.body.pane) { await Mux.close(pane, on: source) }
        if !plan.terminals.isEmpty {
            notes.append("\(plan.terminals.count) Terminal\(plan.terminals.count == 1 ? "" : "s") on \(from) closed, since a shell cannot move.")
        }
    }

    private func carry(_ move: Move) async -> Bool {
        step("Reading the Vault on \(from)")
        var records: [Record] = []
        let pulled = await source.fetch("\(Helper.path) vault pull \(vault.folder)") { line in
            if let record = try? JSONDecoder().decode(Record.self, from: Data(line.utf8)) { records.append(record) }
        }
        guard pulled.ok else { return fail("Sesh could not read the Vault on \(from): \(pulled.problem)") }
        step("Copying the Vault to \(to)")
        guard await copyVault() else { return false }
        let plan = Move.plan(records, from: move.from)
        for repo in plan.repos { await carry(repo, of: plan, move) }
        let placed = await place(plan.sessions, move)
        _ = await target.run(Mux.integrations)
        await checkLogins(plan, move)
        step("Pointing the Vault's records at \(to)")
        return await push(move.rewrite(records, placed: placed))
    }

    private func copyVault() async -> Bool {
        let archive = "\(outbox)/vault.tgz"
        let packed = await source.run("mkdir -p \(shellPath(outbox)) && \(Helper.path) vault export \(vault.folder) \(shellPath(outbox + "/vault.db"))"
            + " && tar czf \(shellPath(archive)) -C \(shellPath(outbox)) vault.db -C \(shellPath(vault.folder)) transcripts")
        guard packed.ok else { return fail("Sesh could not pack the Vault on \(from): \(packed.problem)") }
        if let problem = await transfer(archive, to: "\(inbox)/vault.tgz") { return fail("Sesh could not copy the Vault to \(to): \(problem)") }
        made = true
        let unpacked = await target.run("mkdir -p \(shellPath(vault.folder)) && tar xzf \(shellPath(inbox + "/vault.tgz")) -C \(shellPath(vault.folder))")
        return unpacked.ok || fail("Sesh could not unpack the Vault on \(to): \(unpacked.problem)")
    }

    /// Copies a file from the old Host to the new one through this Mac, a few megabytes at a time.
    private func transfer(_ path: String, to destination: String) async -> String? {
        var offset = 0, part = 0
        while true {
            let read = await source.run("tail -c +\(offset + 1) \(shellPath(path)) | head -c \(Self.chunk) | base64 | tr -d '\\n'")
            guard read.ok, let piece = Data(base64Encoded: read.out) else { return read.problem }
            if piece.isEmpty { break }
            let put = await target.put(piece, to: destination + "." + String(format: "%06d", part))
            guard put.ok else { return put.problem }
            offset += piece.count
            part += 1
        }
        let file = shellPath(destination)
        let joined = await target.run("mkdir -p \"$(dirname \(file))\" && set -- \(file).[0-9]*"
            + " && if [ -e \"$1\" ]; then cat \"$@\" > \(file) && rm -f \"$@\"; else : > \(file); fi")
        return joined.ok ? nil : joined.problem
    }

    /// A repository the open Tasks use, cloned on the new Host when it lacks one, then each of
    /// their Worktrees of it, on the same branch and with the same uncommitted work.
    private func carry(_ repo: String, of plan: Move.Plan, _ move: Move) async {
        step("Carrying \(repo)")
        let there = move.rehome(repo)
        if !(await target.run("git -C \(shellPath(there)) rev-parse --git-dir")).ok {
            let origin = (await source.run("git -C \(shellPath(repo)) remote get-url origin")).out.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !origin.isEmpty else {
                return notes.append("\(repo) has no origin remote, so Sesh could not clone it on \(to), and its Worktrees stay on \(from).")
            }
            let cloned = await target.run("\(Move.quiet) git clone -q \(quote(origin)) \(shellPath(there))")
            guard cloned.ok else {
                return notes.append("Sesh could not clone \(origin) into \(there) on \(to), so the Worktrees of \(repo) stay on \(from): \(cloned.problem)")
            }
        }
        let worksInRepo = plan.sessions.contains { $0.body.path == repo } || plan.tasks.contains { $0.body.machine == move.from && $0.body.path == repo }
        if worksInRepo, !(await Git.clean(repo, on: source)) {
            notes.append("The uncommitted changes in \(repo) itself stay on \(from); Sesh carries those in Worktrees only.")
        }
        for (index, worktree) in plan.worktrees.enumerated() where worktree.repo == repo {
            if let problem = await carry(worktree, index: index, move) {
                notes.append("Sesh could not carry the Worktree \(worktree.path) to \(to): \(problem)")
            }
        }
    }

    private func carry(_ worktree: Move.Folder, index: Int, _ move: Move) async -> String? {
        let path = move.rehome(worktree.path)
        if (await target.run("[ -e \(shellPath(path)) ]")).ok { return "\(path) is there already, so Sesh left it as it is." }
        let out = "\(outbox)/worktree-\(index)", into = "\(inbox)/worktree-\(index)"
        let packed = await source.run(Move.pack(worktree.path, into: out))
        let lines = packed.out.components(separatedBy: "\n")
        guard packed.ok, lines.count >= 3 else { return packed.problem }
        let (head, branch, bundled) = (lines[0], lines[1], lines[2] == "bundle")
        for file in ["changes.patch"] + (bundled ? ["commits.bundle"] : []) {
            if let problem = await transfer("\(out)/\(file)", to: "\(into)/\(file)") { return problem }
        }
        let unpacked = await target.run(Move.unpack(
            repo: move.rehome(worktree.repo), path: path, head: head, branch: branch,
            bundle: bundled ? "\(into)/commits.bundle" : nil, patch: "\(into)/changes.patch", keep: "~/.sesh/unapplied/\(token)-\(index).patch"))
        return unpacked.ok ? nil : unpacked.problem
    }


    /// Puts each session's last Transcript, from the Vault's copy now on the new Host, where its
    /// Agent looks for it there, as resuming on another Host does.
    private func place(_ sessions: [Record], _ move: Move) async -> [String: Move.Spot] {
        step("Placing Transcripts on \(to)")
        var placed: [String: Move.Spot] = [:]
        for session in sessions {
            guard let agent = session.body.agent.flatMap(Agent.init), let last = session.body.transcripts?.last else { continue }
            let title = session.body.title ?? agent.title
            let folder = session.body.path.map(move.rehome)
            if let folder, folder.contains("/.sesh/scratch/") { _ = await target.run("mkdir -p \(shellPath(folder))") }
            let copy = "\(vault.folder)/transcripts/\(session.id)/\((last as NSString).lastPathComponent)"
            let config = agent == .claude ? Move.account(of: last).map { " --config \(shellPath(move.rehome($0)))" } ?? "" : ""
            let ran = await target.run("\(Helper.path) transcripts place \(shellPath(copy)) --agent \(agent.rawValue)\(config)")
            guard ran.ok, let spot = try? JSONDecoder().decode(Placed.self, from: Data(ran.out.utf8)) else {
                notes.append("“\(title)” cannot resume on \(to), since Sesh could not place its Transcript there: \(ran.problem)")
                continue
            }
            placed[session.id] = Move.Spot(path: spot.path, cwd: spot.cwd)
            if let folder, spot.cwd != folder { notes.append("“\(title)” resumes in \(spot.cwd), since \(folder) is not on \(to).") }
        }
        return placed
    }

    /// Says which Agents the moved sessions need that the new Host lacks or has not logged in.
    /// Sesh never copies a login.
    private func checkLogins(_ plan: Move.Plan, _ move: Move) async {
        let agents = Agent.allCases.filter(plan.agents.contains)
        guard !agents.isEmpty else { return }
        let accounts = plan.accounts.map(move.rehome)
        let ran = await target.run(Move.logins(agents: agents, accounts: accounts, home: move.newHome))
        for line in ran.out.split(separator: "\n").map(String.init) {
            let words = line.split(separator: " ", maxSplits: 2).map(String.init)
            guard words.count >= 2, let agent = Agent(rawValue: words[1]) else { continue }
            switch (words[0], agent, words.count > 2 ? words[2] : nil) {
            case ("missing", _, _):
                notes.append("\(agent.title) is not installed on \(to), or not on its PATH. Install it there before resuming its sessions.")
            case ("out", .claude, let account?):
                let command = account == move.newHome + "/.claude" ? "claude" : "CLAUDE_CONFIG_DIR=\(account) claude"
                notes.append("Claude in \(account) is not logged in on \(to). Log in there with \(command).")
            case ("out", .codex, _):
                notes.append("Codex is not logged in on \(to). Log in there with codex login.")
            case ("out", _, _):
                notes.append("\(agent.title) is not logged in on \(to). Start \(agent.rawValue) there and log in with /login.")
            default: break
            }
        }
    }


    private func push(_ changes: [Record]) async -> Bool {
        struct Change: Encodable {
            let id: String
            let kind: Record.Kind
            let body: Record.Body
            let base: Int
            let deleted: Bool
        }
        guard !changes.isEmpty else { return true }
        let lines = changes.compactMap { try? JSONEncoder().encode(Change(id: $0.id, kind: $0.kind, body: $0.body, base: $0.seq, deleted: $0.deleted)) }
        let file = "\(vault.folder)/inbox/\(token).jsonl"
        let put = await target.put(Data((lines.map { String(decoding: $0, as: UTF8.self) }.joined(separator: "\n") + "\n").utf8), to: file)
        guard put.ok else { return fail("Sesh could not send the moved records to \(to): \(put.problem)") }
        let ran = await target.run("\(Helper.path) vault push \(vault.folder) \(file) --batch \(token)")
        return ran.ok || fail("Sesh could not write the moved records on \(to): \(ran.problem)")
    }
}
