import Foundation

/// Where a new Agent session of a Task works and how it is recorded, apart from the machine and
/// the Vault that TaskActions brings.
@MainActor
enum Start {
    /// Where an Agent session works: its folder, the pane made with a fresh Workspace, and the
    /// branch when the folder is a Worktree.
    struct Place: Equatable {
        var folder: String?
        var pane: String?
        var branch: String?
    }

    /// The record of a new Agent session of the Task, carrying its own Worktree.
    static func session(_ agent: Agent, for task: String, on machine: String?, at place: Place, repo: String?, remote: Bool? = nil,
                        name: String? = nil, in vault: some Records) -> Record {
        let count = vault.children(.session, task: task).filter { $0.body.agent == agent.rawValue }.count
        return vault.create(.session, .init(
            title: count == 0 ? agent.title : "\(agent.title) \(count + 1)", task: task,
            position: Double(Date().timeIntervalSince1970), machine: machine, path: place.folder, branch: place.branch,
            repo: repo, pane: place.pane, name: name, agent: agent.rawValue, remote: agent == .claude ? remote : nil))
    }

    /// A new Worktree of `repo` on `branch`, for one Agent session working beside the Task's own,
    /// in the folder herdr would give it. git makes it rather than herdr, which would open it as a
    /// Workspace of its own; its pane goes in the Task's Workspace instead.
    static func worktree(_ branch: String, of repo: String, after main: String, on runner: Runner) async -> Result<Place, Herdr.Failure> {
        let home = (await runner.run("printf %s \"$HOME\"")).out
        let folder = "\(home)/.herdr/worktrees/\((repo as NSString).lastPathComponent)/\(Names.folder(branch))"
        await Git.update(repo, on: runner)
        let base = base(after: main, in: await Git.branches(of: repo, on: runner))
        let ran = await Git.addWorktree(folder, branch: branch, from: base, of: repo, on: runner)
        return ran.ok ? .success(Place(folder: folder, branch: branch)) : .failure(Herdr.Failure(ran.problem))
    }

    /// What a new Worktree branches from: the Task's `main` branch where the repository has it, so
    /// the session carries the Task's work on; nil for what the repository has checked out.
    static func base(after main: String, in branches: Set<String>) -> String? {
        branches.contains(main) ? main : nil
    }

    /// `path` with the machine's `home` folder written as ~, as the user types it.
    static func abbreviate(_ path: String, home: String?) -> String {
        guard let home, !home.isEmpty, path == home || path.hasPrefix(home + "/") else { return path }
        return "~" + path.dropFirst(home.count)
    }

    /// The repositories to offer, newest first, each once however it was written down.
    static func recent(_ repos: [String], home: String?) -> [String] {
        var seen = Set<String>()
        return repos.map { abbreviate($0, home: home) }.filter { seen.insert($0).inserted }.prefix(8).map { $0 }
    }

    /// The folder a git link clones into by default: the repository's own name.
    static func cloneName(of link: String) -> String? {
        let link = link.trimmingCharacters(in: .whitespacesAndNewlines)
        let remote = ["https://", "http://", "ssh://", "git://"].contains { link.hasPrefix($0) }
            || link.range(of: #"^[\w.-]+@[\w.-]+:"#, options: .regularExpression) != nil
        guard remote, !link.contains(" ") else { return nil }
        var name = link.split(whereSeparator: { $0 == "/" || $0 == ":" }).last.map(String.init) ?? ""
        if name.hasSuffix(".git") { name.removeLast(4) }
        return name.isEmpty || link.hasSuffix(":") ? nil : name
    }

    /// Where new folders go by default: the folder most of the recent repositories sit in.
    static func parent(of recents: [String]) -> String {
        let parents = recents.map { ($0 as NSString).deletingLastPathComponent }.filter { !$0.isEmpty }
        let counts = Dictionary(parents.map { ($0, 1) }, uniquingKeysWith: +)
        return parents.first { counts[$0] == counts.values.max() } ?? "~"
    }

    /// What one line of `git clone --progress` says: its phase and how far along it is.
    static func cloning(_ line: String) -> (phase: String, done: Double?)? {
        let line = line.replacingOccurrences(of: "remote: ", with: "").trimmingCharacters(in: .whitespaces)
        guard let colon = line.firstIndex(of: ":"), !line.hasPrefix("fatal"), !line.hasPrefix("error") else { return nil }
        let phase = String(line[..<colon])
        let rest = line[line.index(after: colon)...]
        let percent = rest.split(separator: "%").first.flatMap { Double($0.trimmingCharacters(in: .whitespaces)) }
        return (phase, percent.map { $0 / 100 })
    }

    /// The branch a new Worktree beside the Task's is offered: the Task's with the first free
    /// number after it.
    static func nextBranch(after branch: String, taken: Set<String>) -> String {
        (2...).lazy.map { "\(branch)-\($0)" }.first { !taken.contains($0) } ?? branch
    }
}
