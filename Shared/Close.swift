import Foundation

/// Finishing a Task on its machines: first a plan the user reads before agreeing, then a run of
/// it that tries every step, says what failed, and can be run again.
@MainActor
struct Close {
    struct Worktree: Hashable, Identifiable {
        let machine: String?
        /// The machine's name, as the user knows it.
        let place: String
        let repo: String
        let path: String
        let branch: String?
        /// A repository's own checkout is not linked, and Close never removes it.
        let linked: Bool
        /// What `git status` says is uncommitted; empty when clean.
        let uncommitted: String

        var id: String { "\(machine ?? ""):\(path)" }
    }

    struct Plan {
        /// Every folder the Task's Agent sessions work in apart from a repository's own, the
        /// Task's own first.
        let worktrees: [Worktree]
        /// The Task's herdr Workspace on its own machine, closed with its Worktree.
        let workspace: String?
        /// The Agent sessions whose Transcripts are copied one last time.
        let sessions: [Record]

        var agents: [Record] { sessions.filter { $0.body.pane != nil } }
        var removed: [Worktree] { worktrees.filter(\.linked) }
        /// Worktrees whose uncommitted work is lost unless the user keeps the Task open.
        var dirty: [Worktree] { removed.filter { !$0.uncommitted.isEmpty } }
    }

    struct Failure: Hashable {
        let worktree: Worktree
        let problem: String

        var message: String { "The Worktree at \(worktree.path) on \(worktree.place) is still there: \(problem)" }
    }

    let task: Record
    let sessions: [Record]
    let runner: (String?) -> Runner
    let copy: ([Record]) async -> Void

    func plan() async -> Plan {
        let own = task.body.path.map { [(task.body.machine, task.body.repo ?? $0, $0, task.body.branch)] } ?? []
        let others = sessions.compactMap { session -> (String?, String, String, String?)? in
            guard let repo = session.body.repo, let path = session.body.path, path != repo else { return nil }
            return (session.body.machine, repo, path, session.body.branch ?? task.body.branch)
        }
        var seen = Set<String>(), worktrees: [Worktree] = []
        for (machine, repo, path, branch) in own + others where seen.insert("\(machine ?? ""):\(path)").inserted {
            let runner = runner(machine)
            let linked = await Git.linked(path, on: runner)
            worktrees.append(Worktree(
                machine: machine, place: runner.title, repo: repo, path: path, branch: branch, linked: linked,
                uncommitted: linked ? await Git.uncommitted(path, on: runner) : ""))
        }
        let open = await Herdr.workspaces(on: runner(task.body.machine))
        let workspace = task.body.workspace.flatMap { open[$0] != nil ? $0 : nil } ?? open.first { $0.value == task.body.title }?.key
        return Plan(worktrees: worktrees, workspace: workspace, sessions: sessions)
    }

    /// Copies, stops and removes what `plan` lists. A Worktree already gone is passed over, so
    /// a run again after a failure only retries what is left.
    func perform(_ plan: Plan, discard: Bool) async -> [Failure] {
        await copy(plan.sessions)
        for session in plan.agents {
            _ = await runner(session.body.machine).follower.stop(session.body.pane ?? "")
        }
        let own = { (worktree: Worktree) in worktree.machine == task.body.machine && worktree.path == task.body.path }
        var failures: [Failure] = []
        for worktree in plan.removed {
            let runner = runner(worktree.machine)
            guard await Git.linked(worktree.path, on: runner) else { continue }
            let ran = if own(worktree), let workspace = plan.workspace {
                await Herdr.removeWorktree(of: workspace, force: discard, on: runner)
            } else {
                await Git.removeWorktree(worktree.path, of: worktree.repo, force: discard, on: runner)
            }
            if !ran.ok { failures.append(Failure(worktree: worktree, problem: ran.problem)) }
        }
        if !plan.removed.contains(where: own), let workspace = plan.workspace {
            await Herdr.closeWorkspace(workspace, on: runner(task.body.machine))
        }
        return failures
    }
}
