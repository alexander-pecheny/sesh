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
    static func session(_ agent: Agent, for task: String, on machine: String?, at place: Place, repo: String?, in vault: some Records) -> Record {
        let count = vault.children(.session, task: task).filter { $0.body.agent == agent.rawValue }.count
        return vault.create(.session, .init(
            title: count == 0 ? agent.title : "\(agent.title) \(count + 1)", task: task,
            position: Double(Date().timeIntervalSince1970), machine: machine, path: place.folder, branch: place.branch,
            repo: repo, pane: place.pane, agent: agent.rawValue))
    }

    /// A new Worktree of `repo` on `branch`, for one Agent session working beside the Task's own,
    /// in the folder herdr would give it. git makes it rather than herdr, which would open it as a
    /// Workspace of its own; its pane goes in the Task's Workspace instead.
    static func worktree(_ branch: String, of repo: String, on runner: Runner) async -> Result<Place, Herdr.Failure> {
        let home = (await runner.run("printf %s \"$HOME\"")).out
        let folder = "\(home)/.herdr/worktrees/\((repo as NSString).lastPathComponent)/\(Names.slug(branch))"
        let ran = await Git.addWorktree(folder, branch: branch, of: repo, on: runner)
        return ran.ok ? .success(Place(folder: folder, branch: branch)) : .failure(Herdr.Failure(ran.problem))
    }

    /// The branch a new Worktree beside the Task's is offered: the Task's with the first free
    /// number after it.
    static func nextBranch(after branch: String, taken: Set<String>) -> String {
        (2...).lazy.map { "\(branch)-\($0)" }.first { !taken.contains($0) } ?? branch
    }
}
