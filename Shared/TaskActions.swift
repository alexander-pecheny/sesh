import Foundation

/// What a Task does on its machines: make its Worktree, find its herdr Workspace, start
/// Agent sessions in it (ADR 0009).
@MainActor
enum TaskActions {
    static func machine(_ alias: String?) -> Machine { .named(alias) }

    /// The machines a Task can start Tabs on: its Worktree's alone, or the Vault's and the Mac.
    static func machines(for task: Record, in vault: Vault) -> [Machine] {
        if task.body.path != nil { return [machine(task.body.machine)] }
        return vault.place.alias == nil ? Machine.here : [vault.machine] + Machine.here
    }

    /// The branch a title suggests, under the user's name as herdr's own branches are.
    static func branch(for title: String, user: String) -> String { "\(user)/\(Names.slug(title))" }

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
    static func pane(for task: Record, on machine: Machine, label: String) async -> Result<(Herdr.Opened, String), Herdr.Failure> {
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
        guard let workspace else { return await Herdr.workspace(folder, label: title, on: machine).map { ($0, folder) } }
        return await Herdr.tab(in: workspace, folder: folder, label: label, on: machine).map { ($0, folder) }
    }

    /// Starts `agent` for the Task and adopts the new Agent session from birth.
    static func startSession(_ agent: Agent, for task: Record, on machine: Machine, in vault: Vault) async -> Result<Record, Herdr.Failure> {
        if let problem = await machine.prepare() { return .failure(Herdr.Failure(problem)) }
        let name = Names.slug("\(task.body.title ?? "task")-\(agent.rawValue)")
        let opened: Herdr.Opened, folder: String
        switch await pane(for: task, on: machine, label: agent.title) {
        case .failure(let failure): return .failure(failure)
        case .success(let value): (opened, folder) = value
        }
        if let problem = await Herdr.launch(agent, name: name, pane: opened.pane, on: machine) {
            return .failure(Herdr.Failure(problem))
        }
        let count = vault.children(.session, task: task.id).filter { $0.body.agent == agent.rawValue }.count
        let session = vault.create(.session, .init(
            title: count == 0 ? agent.title : "\(agent.title) \(count + 1)", task: task.id,
            position: Double(Date().timeIntervalSince1970), machine: machine.alias, path: folder, pane: opened.pane, agent: agent.rawValue))
        return .success(session)
    }

    /// Opens a file in the Task as a Document, reusing the Tab if it is open already. A bare
    /// name is looked for among the files the Agent wrote, then in its folder; the first that
    /// exists on the machine wins.
    static func open(_ path: String, from session: Record, wrote: [String], in vault: Vault, library: Library) async {
        guard let task = session.body.task else { return }
        let folder = session.body.path ?? "~"
        var absolute = path
        if !(path.hasPrefix("/") || path.hasPrefix("~/")) {
            let candidates = wrote.filter { $0.hasSuffix("/" + path) } + [(folder as NSString).appendingPathComponent(path)]
            let tests = candidates.map { "[ -f \(shellPath($0)) ] && printf %s \(quote($0)) && exit" }.joined(separator: "; ")
            let found = await machine(session.body.machine).run(tests).out
            absolute = found.isEmpty ? candidates.last ?? path : found
        }
        let known = vault.children(.document, task: task)
            .first { $0.body.path == absolute && $0.body.machine == session.body.machine }
        let document = known ?? vault.create(.document, .init(
            title: (absolute as NSString).lastPathComponent, task: task, machine: session.body.machine, path: absolute))
        library.open(.document(document.id), in: task)
    }

    /// What `git status` says is uncommitted in the Task's Worktree; empty when clean or absent.
    static func uncommitted(in task: Record) async -> String {
        guard let path = task.body.path else { return "" }
        let ran = await machine(task.body.machine).run("git -C \(quote(path)) status --porcelain")
        return ran.ok ? ran.out.trimmingCharacters(in: .whitespacesAndNewlines) : ""
    }

    /// Ends a Task: its Transcripts copied one last time, its Agents stopped, its Worktree and
    /// Workspace removed with the branch kept, and the Task archived. It stays searchable.
    static func close(_ task: Record, in vault: Vault, library: Library, discard: Bool) async -> String? {
        let sessions = vault.children(.session, task: task.id)
        await vault.copier.copy(sessions)
        for session in sessions {
            guard let pane = session.body.pane else { continue }
            let machine = machine(session.body.machine)
            _ = await machine.run("herdr agent send-keys \(quote(pane)) ctrl+c ctrl+c; sleep 1; herdr pane close \(quote(pane))")
        }
        let home = machine(task.body.machine)
        let open = await Herdr.workspaces(on: home)
        let workspace = task.body.workspace.flatMap { open[$0] != nil ? $0 : nil }
            ?? open.first { $0.value == task.body.title }?.key
        if let path = task.body.path {
            let force = discard ? " --force" : ""
            let ran = if let workspace {
                await home.run("herdr worktree remove --workspace \(quote(workspace))\(force)")
            } else {
                await home.run("git -C \(quote(task.body.repo ?? path)) worktree remove \(quote(path))\(force)")
            }
            guard ran.ok else { return "The Worktree is still there: \(ran.problem)" }
        } else if let workspace {
            _ = await home.run("herdr workspace close \(quote(workspace))")
        }
        var record = vault.records[task.id] ?? task
        record.body.archived = true
        vault.write(record)
        for tab in library.tabs[task.id] ?? [] { library.close(tab, in: task.id) }
        if library.selection == task.id { library.selection = nil }
        return nil
    }
}
