import Foundation

/// What a Task does on its machines: make its Worktree, find its herdr Workspace, start
/// Agent sessions in it (ADR 0009).
@MainActor
enum TaskActions {
    static func machine(_ alias: String?) -> Machine { .named(alias) }

    /// The machines a Task can start Tabs on: its Worktree's first, then the Vault's and the Mac.
    static func machines(for task: Record, in vault: Vault) -> [Machine] {
        let own = task.body.path == nil ? [] : [machine(task.body.machine)]
        let all = own + (vault.place.alias == nil ? Machine.here : [vault.machine] + Machine.here)
        var seen = Set<Machine>()
        return all.filter { seen.insert($0).inserted }
    }

    /// The folder Claude keeps its account in for a Vault's sessions on `machine`, as set on
    /// this device; nil for Claude's own default.
    static func claudeConfig(_ vault: Vault, on machine: Machine) -> String? {
        (UserDefaults.standard.dictionary(forKey: "claudeConfig") as? [String: String])?["\(vault.place.name)|\(machine.id)"]
    }

    static func setClaudeConfig(_ config: String, _ vault: Vault, on machine: Machine) {
        var all = UserDefaults.standard.dictionary(forKey: "claudeConfig") as? [String: String] ?? [:]
        all["\(vault.place.name)|\(machine.id)"] = config.isEmpty ? nil : config
        UserDefaults.standard.set(all, forKey: "claudeConfig")
    }

    /// The branch a title suggests, under the user's name as herdr's own branches are.
    static func branch(for title: String, user: String) -> String {
        let slug = Names.slug(title)
        return "\(user)/\(slug.isEmpty ? "task" : slug)"
    }

    /// The Task's Workspace on `machine`, made if herdr has none, and a fresh pane in it.
    static func pane(for task: Record, on machine: Machine, label: String, folder wanted: String? = nil) async -> Result<(Herdr.Opened, String), Herdr.Failure> {
        let title = task.body.title ?? "Task"
        let open = await Herdr.workspaces(on: machine)
        let known = task.body.workspace.flatMap { open[$0] != nil && machine.alias == task.body.machine ? $0 : nil }
        let workspace = known ?? open.first { $0.value == title }?.key
        let folder: String
        if let wanted {
            folder = wanted
        } else if let path = task.body.path, machine.alias == task.body.machine {
            folder = path
        } else {
            folder = (await machine.run("printf %s \"$HOME\"")).out
        }
        guard let workspace else { return await Herdr.workspace(folder, label: title, on: machine).map { ($0, folder) } }
        return await Herdr.tab(in: workspace, folder: folder, label: label, on: machine).map { ($0, folder) }
    }

    /// Starts `agent` for the Task, in its Worktree of `repo` when one is given, and adopts the
    /// new Agent session from birth. The session is made, and `made` told, once its pane is;
    /// an Agent that then fails to start takes its session with it.
    static func startSession(_ agent: Agent, for task: Record, on machine: Machine, repo: String?, in vault: Vault,
                             made: (Record) -> Void) async -> Result<Record, Herdr.Failure> {
        if let problem = await machine.prepare() { return .failure(Herdr.Failure(problem)) }
        var folder: String?, fresh: String?
        if let repo {
            switch await worktree(for: task, repo: repo, on: machine, in: vault) {
            case .failure(let failure): return .failure(failure)
            case .success(let made): (folder, fresh) = made
            }
        }
        let current = vault.records[task.id] ?? task
        let pane: String
        if let fresh {
            pane = fresh
        } else {
            switch await self.pane(for: current, on: machine, label: agent.title, folder: folder) {
            case .failure(let failure): return .failure(failure)
            case .success(let value): (pane, folder) = (value.0.pane, value.1)
            }
        }
        let count = vault.children(.session, task: task.id).filter { $0.body.agent == agent.rawValue }.count
        var body = Record.Body(
            title: count == 0 ? agent.title : "\(agent.title) \(count + 1)", task: task.id,
            position: Double(Date().timeIntervalSince1970), machine: machine.alias, path: folder, pane: pane, agent: agent.rawValue)
        body.repo = repo
        let session = vault.create(.session, body)
        made(session)
        let name = Names.slug("\(current.body.title ?? "task")-\(agent.rawValue)")
        if let problem = await Herdr.launch(agent, name: name, pane: pane, config: claudeConfig(vault, on: machine), on: machine) {
            vault.delete(vault.records[session.id] ?? session)
            return .failure(Herdr.Failure(problem))
        }
        return .success(vault.records[session.id] ?? session)
    }

    /// The Task's Worktree of `repo` on `machine`, made on first use with the Task's branch, and
    /// the fresh Workspace's own pane when it was made just now.
    static func worktree(for task: Record, repo given: String, on machine: Machine, in vault: Vault) async -> Result<(String, String?), Herdr.Failure> {
        let repo = given.hasPrefix("~/") ? (await machine.run("printf %s \"$HOME\"")).out + given.dropFirst(1) : given
        let current = vault.records[task.id] ?? task
        if let path = current.body.path, current.body.repo == repo, current.body.machine == machine.alias { return .success((path, nil)) }
        let known = vault.children(.session, task: task.id)
            .first { $0.body.repo == repo && $0.body.machine == machine.alias && $0.body.path != nil && $0.body.path != repo }
        if let path = known?.body.path { return .success((path, nil)) }
        let title = current.body.title ?? "Task"
        let user = user(on: machine)
        let branch: String
        if let named = current.body.branch, !named.hasSuffix("/") {
            branch = named
        } else {
            branch = await suggestBranch(for: title, user: user, in: vault) ?? self.branch(for: title, user: user)
        }
        // A missing or empty folder becomes a new repo. One with no commits yet has no HEAD to
        // branch a worktree from, so it is used as is.
        let folder = quote(repo)
        let born = await machine.run(
            "[ -n \"$(command ls -A \(folder) 2>/dev/null)\" ] || { mkdir -p \(folder) && command git -C \(folder) init -q; }; "
                + "command git -C \(folder) rev-parse --verify -q HEAD").ok
        let made = born
            ? await Herdr.worktree(repo, branch: branch, label: title, on: machine)
            : await Herdr.workspace(repo, label: title, on: machine)
        guard current.body.path == nil else { return made.map { ($0.path ?? repo, $0.pane) } }
        switch made {
        case .failure(let failure): return .failure(failure)
        case .success(let opened):
            var record = vault.records[task.id] ?? current
            record.body.machine = machine.alias
            record.body.repo = repo
            record.body.path = opened.path ?? repo
            if born { record.body.branch = opened.branch ?? branch }
            record.body.workspace = opened.workspace
            vault.write(record)
            return .success((opened.path ?? repo, opened.pane))
        }
    }

    /// The repositories a new session of the Task may want on `machine`, the Task's own last
    /// used first, then the newest used anywhere.
    static func recentRepos(for task: Record, on machine: String?, library: Library) -> [String] {
        let sessions = library.vaults.flatMap { $0.all(.session) }.filter { $0.body.machine == machine }
            .sorted { ($0.body.position ?? 0) > ($1.body.position ?? 0) }
        let mine = sessions.filter { $0.body.task == task.id }.compactMap(\.body.repo)
        let tasks = library.vaults.flatMap { $0.all(.task) }.filter { $0.body.machine == machine }
        let own = task.body.machine == machine ? [task.body.repo].compactMap { $0 } : []
        var seen = Set<String>()
        return (mine + own + sessions.compactMap(\.body.repo) + tasks.compactMap(\.body.repo))
            .filter { seen.insert($0).inserted }
            .prefix(8).map { $0 }
    }

    /// Gives a new Task a branch named by a model from its title, so starting work later asks
    /// for nothing; a slug of the title when no model answers.
    static func nameBranch(of task: Record, in vault: Vault) {
        Task {
            let user = user(on: vault.machine)
            let title = task.body.title ?? "task"
            let named = await suggestBranch(for: title, user: user, in: vault)
            var record = vault.records[task.id] ?? task
            guard record.body.branch?.hasSuffix("/") != false else { return }
            record.body.branch = named ?? branch(for: title, user: user)
            vault.write(record)
        }
    }

    private static func suggestBranch(for title: String, user: String, in vault: Vault) async -> String? {
        #if os(macOS)
        let key = try? String(contentsOfFile: NSHomeDirectory() + "/.openrouter_key", encoding: .utf8)
        #else
        let key = Optional((await vault.machine.run("cat ~/.openrouter_key")).out)
        #endif
        guard let key = key?.trimmingCharacters(in: .whitespacesAndNewlines), !key.isEmpty,
              let url = URL(string: "https://openrouter.ai/api/v1/chat/completions") else { return nil }
        var request = URLRequest(url: url, timeoutInterval: 20)
        request.httpMethod = "POST"
        request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let prompt = "Generate a branch name \(user)/... from natural language short task description: \(title). Reply with only the branch name"
        request.httpBody = try? JSONSerialization.data(withJSONObject: [
            // Left to think, Haiku can spend every token reasoning about a title and answer nothing.
            "model": "anthropic/claude-haiku-5.5", "max_tokens": 60, "reasoning": ["enabled": false],
            "messages": [["role": "user", "content": prompt]],
        ])
        struct Reply: Decodable {
            struct Choice: Decodable { struct Message: Decodable { let content: String }; let message: Message }
            let choices: [Choice]
        }
        guard let (data, _) = try? await URLSession.shared.data(for: request),
              let text = (try? JSONDecoder().decode(Reply.self, from: data))?.choices.first?.message.content else { return nil }
        let branch = text.trimmingCharacters(in: .whitespacesAndNewlines.union(CharacterSet(charactersIn: "`\"'")))
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "/-_."))
        guard branch.hasPrefix(user + "/"), branch.count < 80, branch.unicodeScalars.allSatisfy(allowed.contains) else { return nil }
        return branch
    }

    /// The user herdr's own branches are named for: the Mac's, or the Host's ssh user.
    static func user(on machine: Machine) -> String {
        #if os(macOS)
        return NSUserName()
        #else
        return machine.host?.user ?? "sesh"
        #endif
    }

    /// Starts a session's Agent again on its last Transcript, in a fresh pane in its folder.
    static func resume(_ session: Record, of task: Record, in vault: Vault) async -> String? {
        guard let agent = session.body.agent.flatMap(Agent.init), let transcript = session.body.transcripts?.last else {
            return "Sesh holds no Transcript of this Agent session, so there is nothing to resume."
        }
        let machine = machine(session.body.machine)
        if let problem = await machine.prepare() { return problem }
        let opened: Herdr.Opened
        switch await pane(for: task, on: machine, label: agent.title, folder: session.body.path) {
        case .failure(let failure): return failure.message
        case .success(let value): opened = value.0
        }
        let name = Names.slug("\(task.body.title ?? "task")-\(agent.rawValue)")
        if let problem = await Herdr.launch(
            agent, name: name, pane: opened.pane, resuming: transcript, config: claudeConfig(vault, on: machine), on: machine) { return problem }
        var record = vault.records[session.id] ?? session
        record.body.pane = opened.pane
        vault.write(record)
        return nil
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

    /// Ends a Task at once: archived, still searchable, and its Tabs closed. Its machines are
    /// cleaned after, as `plan` says; what that cannot do waits for the user to retry.
    static func close(_ close: Close, as plan: Close.Plan, in vault: Vault, library: Library, discard: Bool) {
        var record = vault.records[close.task.id] ?? close.task
        record.body.archived = true
        vault.write(record)
        for tab in library.tabs[record.id] ?? [] { library.close(tab, in: record.id, confirmed: true) }
        if library.selection == record.id { library.selection = nil }
        finish(close, plan, discard: discard, library: library)
    }

    private static func finish(_ close: Close, _ plan: Close.Plan, discard: Bool, library: Library) {
        Task {
            let failures = await close.perform(plan, discard: discard)
            guard !failures.isEmpty else { return }
            library.cleanupProblem = Library.CleanupProblem(
                message: "“\(close.task.body.title ?? "Task")” is closed, but not cleaned up.\n\n" + failures.map(\.message).joined(separator: "\n\n"),
                retry: { finish(close, plan, discard: discard, library: library) })
        }
    }
}

extension Close {
    init(_ task: Record, in vault: Vault) {
        self.init(task: task, sessions: vault.children(.session, task: task.id), runner: { TaskActions.machine($0) }, copy: vault.copier.copy)
    }
}
