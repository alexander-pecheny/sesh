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
        var seen = Set<String>()
        let wanted = (own + others).filter { seen.insert("\(machine($0.0)):\($0.2)").inserted }
        async let open = Herdr.workspaces(on: runner(task.body.machine))
        let found = await withTaskGroup(of: (String, [String: String]).self) { group in
            for machine in Set(wanted.map { machine($0.0) }) {
                let paths = wanted.filter { self.machine($0.0) == machine }.map(\.2)
                group.addTask { @MainActor in (machine, await Git.survey(paths, on: runner(machine.isEmpty ? nil : machine))) }
            }
            return await group.reduce(into: [:]) { $0[$1.0] = $1.1 }
        }
        let worktrees = wanted.map { machine, repo, path, branch in
            let uncommitted = found[self.machine(machine)]?[path]
            return Worktree(machine: machine, place: runner(machine).title, repo: repo, path: path, branch: branch,
                            linked: uncommitted != nil, uncommitted: uncommitted ?? "")
        }
        let workspaces = await open
        let workspace = task.body.workspace.flatMap { workspaces[$0] != nil ? $0 : nil }
            ?? workspaces.first { $0.value == task.body.title }?.key
        return Plan(worktrees: worktrees, workspace: workspace, sessions: sessions)
    }

    private func machine(_ alias: String?) -> String { alias ?? "" }

    /// Copies, stops and removes what `plan` lists. A Worktree already gone is passed over, so
    /// a run again after a failure only retries what is left.
    func perform(_ plan: Plan, discard: Bool) async -> [Failure] {
        await copy(plan.sessions)
        for session in plan.agents {
            _ = await runner(session.body.machine).follower.stop(session.body.pane ?? "")
        }
        let own = { (worktree: Worktree) in worktree.machine == task.body.machine && worktree.path == task.body.path }
        // herdr closes a Workspace with its last pane, so stopping the Agents may have closed it.
        let open = await Herdr.workspaces(on: runner(task.body.machine))
        let workspace = plan.workspace.flatMap { open[$0] != nil ? $0 : nil }
        var failures: [Failure] = []
        for worktree in plan.removed {
            let runner = runner(worktree.machine)
            guard await Git.linked(worktree.path, on: runner) else { continue }
            // git refuses any worktree with a submodule checked out unless forced, however clean.
            let force = await Git.clean(worktree.path, on: runner) || discard
            let ran = if own(worktree), let workspace {
                await Herdr.removeWorktree(of: workspace, force: force, on: runner)
            } else {
                await Git.removeWorktree(worktree.path, of: worktree.repo, force: force, on: runner)
            }
            if !ran.ok { failures.append(Failure(worktree: worktree, problem: ran.problem)) }
        }
        if !plan.removed.contains(where: own), let workspace {
            await Herdr.closeWorkspace(workspace, on: runner(task.body.machine))
        }
        // A Task closes once its work is merged, which the main checkout should then show.
        var updated = Set<String>()
        for worktree in plan.removed where updated.insert("\(worktree.machine ?? "")|\(worktree.repo)").inserted {
            await Git.update(worktree.repo, on: runner(worktree.machine))
        }
        return failures
    }
}
