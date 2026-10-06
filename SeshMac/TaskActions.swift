import Foundation

/// What a Task does on its machines: make its Worktree, find its herdr Workspace, start
/// Agent sessions in it (ADR 0009).
@MainActor
enum TaskActions {
    static func machine(_ alias: String?) -> Machine { alias.map { Machine(alias: $0) } ?? .mac }

    /// The machines a Task can start Tabs on: its Worktree's alone, or the Vault's and the Mac.
    static func machines(for task: Record, in vault: Vault) -> [Machine] {
        if task.body.path != nil { return [machine(task.body.machine)] }
        return vault.place.alias == nil ? [.mac] : [vault.machine, .mac]
    }

    /// The branch a title suggests, under the user's name as herdr's own branches are.
    static func branch(for title: String) -> String { "\(NSUserName())/\(Names.slug(title))" }

    /// Makes the Worktree a new Task asked for and records it on the Task.
    static func makeWorktree(for task: Record, repo: String, branch: String, on machine: Machine, in vault: Vault) async -> String? {
        if let problem = await machine.prepare() { return problem }
        switch await Herdr.worktree(repo, branch: branch, label: task.body.title ?? branch, on: machine) {
        case .failure(let failure): return failure.message
        case .success(let opened):
            var record = vault.records[task.id] ?? task
            record.body.machine = machine.alias
            record.body.repo = repo
            record.body.path = opened.path
            record.body.branch = opened.branch ?? branch
            record.body.workspace = opened.workspace
            vault.write(record)
            return nil
        }
    }

    /// The Task's Workspace on `machine`, made if herdr has none, and a fresh pane in it.
    private static func pane(for task: Record, on machine: Machine, label: String) async -> Result<Herdr.Opened, Herdr.Failure> {
        let title = task.body.title ?? "Task"
        let open = await Herdr.workspaces(on: machine)
        let known = task.body.workspace.flatMap { open[$0] != nil && machine.alias == task.body.machine ? $0 : nil }
        let workspace = known ?? open.first { $0.value == title }?.key
        let folder: String
        if let path = task.body.path, machine.alias == task.body.machine {
            folder = path
        } else {
            folder = (await machine.run("printf %s \"$HOME\"")).out
        }
        guard let workspace else { return await Herdr.workspace(folder, label: title, on: machine) }
        return await Herdr.tab(in: workspace, folder: folder, label: label, on: machine)
    }

    /// Starts `agent` for the Task and adopts the new Agent session from birth.
    static func startSession(_ agent: Agent, for task: Record, on machine: Machine, in vault: Vault) async -> Result<Record, Herdr.Failure> {
        if let problem = await machine.prepare() { return .failure(Herdr.Failure(problem)) }
        let name = Names.slug("\(task.body.title ?? "task")-\(agent.rawValue)")
        let opened: Herdr.Opened
        switch await pane(for: task, on: machine, label: agent.title) {
        case .failure(let failure): return .failure(failure)
        case .success(let value): opened = value
        }
        if let problem = await Herdr.launch(agent, name: name, pane: opened.pane, on: machine) {
            return .failure(Herdr.Failure(problem))
        }
        let count = vault.children(.session, task: task.id).filter { $0.body.agent == agent.rawValue }.count
        let session = vault.create(.session, .init(
            title: count == 0 ? agent.title : "\(agent.title) \(count + 1)", task: task.id,
            position: Double(Date().timeIntervalSince1970), machine: machine.alias, pane: opened.pane, agent: agent.rawValue))
        return .success(session)
    }
}
