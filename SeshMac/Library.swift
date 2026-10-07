import Foundation

/// Every Vault this Mac has opened, and what is open in each Task.
@MainActor
final class Library: ObservableObject {
    @Published private(set) var vaults: [Vault] = []
    @Published var selection: String? { didSet { keep(); markSeen() } }
    /// The open Tabs of each Task by its id; the Journal is never in here, as it never closes.
    @Published var tabs: [String: [TabItem]] = [:] { didSet { keep() } }
    @Published var current: [String: TabItem] = [:] { didSet { keep(); markSeen() } }
    /// Agent sessions being started, by Task, shown as Tabs until their Agent is ready.
    @Published var starting: [String: [Starting]] = [:]

    struct Starting: Identifiable, Hashable {
        let id = UUID()
        let agent: Agent
        let machine: String
    }

    /// Opens a Tab for the new session at once, and swaps in its Conversation once the Agent
    /// can take a prompt; starting one takes tens of seconds.
    func start(_ agent: Agent, for task: Record, on machine: Machine, problem: @escaping (String) -> Void) {
        guard let vault = vault(of: task.id) else { return }
        let pending = Starting(agent: agent, machine: machine.title)
        starting[task.id, default: []].append(pending)
        Task {
            let result = await TaskActions.startSession(agent, for: task, on: machine, in: vault)
            starting[task.id]?.removeAll { $0 == pending }
            switch result {
            case .success(let session): open(.session(session.id), in: task.id)
            case .failure(let failure): problem(failure.message)
            }
        }
    }

    private struct Kept: Codable {
        var selection: String?
        var tabs: [String: [TabItem]]
        var current: [String: TabItem]
    }

    /// What was open survives a restart; Terminals do not, as their shells ended with the app.
    private func keep() {
        let tabs = tabs.mapValues { $0.filter { if case .terminal = $0 { false } else { true } } }
        let current = current.filter { if case .terminal = $0.value { false } else { true } }
        UserDefaults.standard.set(try? JSONEncoder().encode(Kept(selection: selection, tabs: tabs, current: current)), forKey: "tabs")
    }

    private static let key = "vaults"
    /// Kept while the app runs, so switching Tabs neither loses the place nor refetches.
    private var conversations: [String: Conversation] = [:]

    /// The live pane while its machine has it; otherwise the Vault's copy of the last Transcript.
    func conversation(for session: Record) async -> Conversation {
        if let known = conversations[session.id] { return known }
        let machine = TaskActions.machine(session.body.machine)
        let agent = session.body.agent.flatMap(Agent.init)
        let live = await machine.run("herdr pane get \(quote(session.body.pane ?? ""))")
        let conversation: Conversation
        if !live.ok, let vault = vault(of: session.id), let last = session.body.transcripts?.last, let agent {
            let copy = "\(vault.folder)/transcripts/\(session.id)/\((last as NSString).lastPathComponent)"
            conversation = Conversation(source: .file(path: copy, agent: agent), agent: agent, runner: vault.machine)
        } else {
            conversation = Conversation(pane: session.body.pane ?? "", agent: agent, runner: machine)
            conversation.openPath = { [weak self, weak conversation] path in
                guard let self, let vault = self.vault(of: session.id), let current = vault.records[session.id] else { return }
                let wrote = (conversation?.items ?? []).compactMap { item -> String? in
                if case .entry(let entry) = item { entry.file } else { nil }
            }
            Task { await TaskActions.open(path, from: current, wrote: wrote, in: vault, library: self) }
            }
        }
        if let cached = conversations[session.id] { return cached }
        conversation.bookmark = { [weak self] entry in self?.bookmark(entry, in: session.id) }
        if let folder = session.body.path {
            Task { [weak conversation] in
                let remote = await machine.run("git -C \(quote(folder)) remote get-url origin")
                if remote.ok { conversation?.repo = Repo(remote: remote.out) }
            }
        }
        conversation.openSubagent = { [weak self, weak conversation] call, title in
            guard let self, let transcript = conversation?.transcript else { return }
            Task { await self.openSubagent(call, title: title, of: session, transcript: transcript) }
        }
        conversations[session.id] = conversation
        if let item = pendingFocus.removeValue(forKey: session.id) { Task { await conversation.reveal(item) } }
        return conversation
    }

    init() {
        let places = UserDefaults.standard.data(forKey: Self.key)
            .flatMap { try? JSONDecoder().decode([Vault.Place].self, from: $0) } ?? []
        vaults = places.map(Vault.init)
        vaults.forEach { $0.start() }
        watchUnfiled()
        if let kept = UserDefaults.standard.data(forKey: "tabs").flatMap({ try? JSONDecoder().decode(Kept.self, from: $0) }) {
            (selection, tabs, current) = (kept.selection, kept.tabs, kept.current)
        }
    }

    func add(_ place: Vault.Place) {
        guard !vaults.contains(where: { $0.name == place.name }) else { return }
        let vault = Vault(place)
        vaults.append(vault)
        vault.start()
        UserDefaults.standard.set(try? JSONEncoder().encode(vaults.map(\.place)), forKey: Self.key)
    }

    func vault(of task: String) -> Vault? { vaults.first { $0.records[task] != nil } }

    // MARK: Unfiled

    /// An Agent session on one of the machines that no Task has adopted.
    struct Unfiled: Identifiable, Hashable {
        let machine: String?
        let pane: String
        let agent: Agent
        let cwd: String
        let name: String
        var id: String { "\(machine ?? ""):\(pane)" }
    }

    @Published private(set) var unfiled: [String: [Unfiled]] = [:]
    private var watching: Task<Void, Never>?

    /// Every Vault's Host and the Mac, looked at again every few seconds.
    func watchUnfiled() {
        guard watching == nil else { return }
        watching = Task { [weak self] in
            while !Task.isCancelled, let self {
                await self.refreshUnfiled()
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    func refreshUnfiled() async {
        struct List: Decodable {
            struct Result: Decodable {
                struct Agent: Decodable {
                    let agent: String
                    let cwd: String
                    let name: String?
                    let pane_id: String
                    let agent_status: String
                    let completion_seq: UInt64?
                }
                let agents: [Agent]
            }
            let result: Result
        }
        let machines = [Machine.mac] + vaults.compactMap { $0.place.alias == nil ? nil : $0.machine }
        let adopted = Set(vaults.flatMap { $0.all(.session) }.map { "\($0.body.machine ?? ""):\($0.body.pane ?? "")" })
        var found: [String: [Unfiled]] = [:]
        var states: [String: Live] = [:]
        for machine in Set(machines) {
            let ran = await machine.run("herdr agent list")
            guard let list = try? JSONDecoder().decode(List.self, from: Data(ran.out.utf8)) else { continue }
            for agent in list.result.agents {
                let key = "\(machine.alias ?? ""):\(agent.pane_id)"
                let state = Live(status: agent.agent_status, done: agent.completion_seq ?? 0)
                if state.status == "working" || (live[key].map { state.done > $0.done } ?? false) { active[key] = Date().timeIntervalSince1970 * 1000 }
                states[key] = state
            }
            found[machine.id] = list.result.agents.compactMap { agent in
                guard let kind = Agent(rawValue: agent.agent) else { return nil }
                let item = Unfiled(machine: machine.alias, pane: agent.pane_id, agent: kind, cwd: agent.cwd,
                                   name: agent.name ?? (agent.cwd as NSString).lastPathComponent)
                return adopted.contains(item.id) ? nil : item
            }
        }
        unfiled = found
        live = states
        markSeen()
    }

    // MARK: Marks

    /// What herdr last said of a pane: its status, and how many turns its Agent has finished.
    struct Live: Equatable {
        let status: String
        let done: UInt64
    }

    enum Mark: Int, Comparable {
        case seen, finished, working, waiting
        static func < (a: Mark, b: Mark) -> Bool { a.rawValue < b.rawValue }
    }

    @Published private(set) var live: [String: Live] = [:]
    /// When each pane's Agent last worked or finished a turn, in milliseconds since 1970.
    @Published private(set) var active: [String: Double] = UserDefaults.standard.dictionary(forKey: "active") as? [String: Double] ?? [:] {
        didSet { UserDefaults.standard.set(active, forKey: "active") }
    }

    /// The latest moment anything happened in a Task, or in any Task inside a folder: an Entry,
    /// a Document edited, a session started, an Agent working or finishing.
    func changed(_ record: Record, in vault: Vault) -> Double {
        if record.kind == .folder {
            return Tree.children(of: record.id, in: vault).map { changed($0, in: vault) }.max() ?? 0
        }
        let own = Double(record.body.edited ?? 0)
        let parts = vault.records.values.filter { !$0.deleted && $0.body.task == record.id }.map { part -> Double in
            switch part.kind {
            case .session: max((part.body.position ?? 0) * 1000, active[key(part)] ?? 0)
            default: Double(max(part.body.at ?? 0, part.body.edited ?? 0))
            }
        }
        return max(own, parts.max() ?? 0)
    }
    /// The finished-turn count the user has seen, by `machine:pane`, so a turn finished while
    /// they looked elsewhere, or while the app was closed, still shows.
    private var seen: [String: UInt64] = UserDefaults.standard.dictionary(forKey: "seen") as? [String: UInt64] ?? [:]

    private func key(_ session: Record) -> String { "\(session.body.machine ?? ""):\(session.body.pane ?? "")" }

    /// An Agent waiting on the user outranks one working, which outranks one finished unseen.
    func mark(of session: Record) -> Mark? {
        guard let state = live[key(session)] else { return nil }
        if state.status == "blocked" { return .waiting }
        if state.status == "working" { return .working }
        return state.done > seen[key(session), default: state.done] ? .finished : .seen
    }

    func mark(ofTask task: String) -> Mark? {
        vault(of: task)?.children(.session, task: task).compactMap(mark(of:)).max()
    }

    /// The open Agent session counts as seen, and so does every pane met for the first time.
    func markSeen() {
        var changed = false
        for (key, state) in live where seen[key] == nil {
            seen[key] = state.done
            changed = true
        }
        if let task = selection, case .session(let id)? = current[task], let session = vault(of: id)?.records[id],
           let state = live[key(session)], seen[key(session)] != state.done {
            seen[key(session)] = state.done
            changed = true
        }
        guard changed else { return }
        UserDefaults.standard.set(seen.mapValues { NSNumber(value: $0) }, forKey: "seen")
        objectWillChange.send()
    }

    /// Makes an Unfiled Agent session part of `task`; it never goes back.
    func adopt(_ item: Unfiled, into task: String) {
        guard let vault = vault(of: task) else { return }
        let session = vault.create(.session, .init(
            title: "\(item.agent.title) in \((item.cwd as NSString).lastPathComponent)", task: task,
            position: Double(Date().timeIntervalSince1970), machine: item.machine, path: item.cwd, pane: item.pane,
            agent: item.agent.rawValue))
        unfiled[item.machine ?? ""]?.removeAll { $0 == item }
        open(.session(session.id), in: task)
    }

    static let unfiledPrefix = "unfiled:"

    // MARK: Subagents

    /// Claude keeps a subagent's Transcript beside its parent's, under `subagents`, with a
    /// `.meta.json` naming the tool call that started it.
    private func openSubagent(_ call: String, title: String, of session: Record, transcript: String) async {
        guard let task = session.body.task else { return }
        let folder = (transcript as NSString).deletingPathExtension + "/subagents"
        let machine = TaskActions.machine(session.body.machine)
        let ran = await machine.run("grep -l \(quote("\"toolUseId\":\"\(call)\"")) \(quote(folder))/*.meta.json | head -n 1")
        let meta = ran.out.trimmingCharacters(in: .whitespacesAndNewlines)
        guard ran.ok, meta.hasSuffix(".meta.json") else {
            conversations[session.id]?.problem = "Claude has not written a Transcript for this subagent yet."
            return
        }
        open(.subagent(session: session.id, path: String(meta.dropLast(".meta.json".count)) + ".jsonl", title: title), in: task)
    }

    func subagentConversation(path: String, of session: Record) -> Conversation {
        if let known = subagents[path] { return known }
        let conversation = Conversation(source: .file(path: path, agent: .claude), agent: .claude,
                                        runner: TaskActions.machine(session.body.machine))
        conversation.openPath = { [weak self, weak conversation] file in
            guard let self, let vault = self.vault(of: session.id), let current = vault.records[session.id] else { return }
            let wrote = (conversation?.items ?? []).compactMap { item -> String? in
                if case .entry(let entry) = item { entry.file } else { nil }
            }
            Task { await TaskActions.open(file, from: current, wrote: wrote, in: vault, library: self) }
        }
        conversation.openSubagent = { [weak self, weak conversation] call, title in
            guard let self, let transcript = conversation?.transcript else { return }
            Task { await self.openSubagent(call, title: title, of: session, transcript: transcript) }
        }
        subagents[path] = conversation
        return conversation
    }

    private var subagents: [String: Conversation] = [:]

    // MARK: Terminals

    /// Open Terminals by Tab; a Terminal's shell ends when its Tab closes.
    private var terminals: [UUID: Ghostty.TerminalSurface] = [:]

    /// A plain shell for the Task: in its Worktree when the machine has it, else at home.
    func openTerminal(in task: Record, on machine: Machine) {
        let folder = machine.alias == task.body.machine ? task.body.path : nil
        let command: String?
        if let alias = machine.alias {
            let start = folder.map { "cd \(quote($0)) && exec \"$SHELL\" -l" } ?? "exec \"$SHELL\" -l"
            // Its own connection: the shared one may already carry as many channels as sshd allows.
            command = "/usr/bin/ssh -t -o ControlPath=none \(alias) \(quote(start))"
        } else {
            command = nil
        }
        let id = UUID()
        let surface = Ghostty.TerminalSurface(command: command, folder: machine.alias == nil ? folder : nil)
        surface.onClose = { [weak self] in self?.close(.terminal(id), in: task.id) }
        terminals[id] = surface
        open(.terminal(id), in: task.id)
    }

    func terminal(_ id: UUID) -> Ghostty.TerminalSurface? { terminals[id] }

    // MARK: Bookmarks and links

    /// Items a link asked for in Conversations not open yet.
    private var pendingFocus: [String: String] = [:]

    static func link(vault: String, session: String, item: String) -> URL? {
        URL(string: "sesh://\(vault)/\(session)/\(item)")
    }

    /// An Entry in the session's Task quoting the item, with a link back to it.
    func bookmark(_ entry: Conversation.Entry, in session: String) {
        guard let vault = vault(of: session), let record = vault.records[session], let task = record.body.task,
              let url = Self.link(vault: vault.name, session: session, item: entry.id) else { return }
        let source = (entry.text?.isEmpty == false ? entry.text : nil) ?? entry.summary
        let lines = source.split(separator: "\n", omittingEmptySubsequences: true).prefix(3)
        let quote = lines.map { "> " + $0 }.joined(separator: "\n")
        let name = record.body.title ?? "Agent session"
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        _ = vault.create(.entry, .init(text: "\(quote)\n\n[\(name)](\(url.absoluteString))", task: task, edited: now, at: now))
    }

    /// Opens what a `sesh://` link names: an item of an Agent session, in whichever Task has it.
    func follow(_ url: URL) -> Bool {
        guard url.scheme == "sesh", let name = url.host(), let vault = vaults.first(where: { $0.name == name }) else { return false }
        let parts = url.pathComponents.filter { $0 != "/" }
        guard let session = parts.first, let record = vault.records[session], let task = record.body.task else { return false }
        open(.session(session), in: task)
        if let item = parts.dropFirst().first {
            if let conversation = conversations[session] { Task { await conversation.reveal(item) } } else { pendingFocus[session] = item }
        }
        return true
    }

    /// Something dropped on a Task: an Unfiled Agent session, or an Agent session or Document
    /// of another Task.
    func receive(_ dropped: String, into task: String) {
        if dropped.hasPrefix(Self.unfiledPrefix) {
            let id = String(dropped.dropFirst(Self.unfiledPrefix.count))
            if let item = unfiled.values.joined().first(where: { $0.id == id }) { adopt(item, into: task) }
        } else {
            move(dropped, to: task)
        }
    }

    /// Moves an Agent session or Document to another Task of the same Vault, Tab and all.
    func move(_ id: String, to task: String) {
        guard let vault = vault(of: id), var record = vault.records[id], let from = record.body.task, from != task,
              vault.records[task] != nil else { return }
        record.body.task = task
        vault.write(record)
        let tab: TabItem = record.kind == .session ? .session(id) : .document(id)
        close(tab, in: from)
        open(tab, in: task)
    }

    func open(_ tab: TabItem, in task: String) {
        if tab != .journal, !(tabs[task] ?? []).contains(tab) { tabs[task, default: []].append(tab) }
        current[task] = tab
        selection = task
    }

    /// The selected Task's current Tab, unless it is the Journal, which never closes.
    func closeCurrent() {
        guard let task = selection, let tab = current[task], tab != .journal else { return }
        close(tab, in: task)
    }

    func close(_ tab: TabItem, in task: String) {
        if case .terminal(let id) = tab { terminals[id] = nil }
        tabs[task]?.removeAll { $0 == tab }
        if current[task] == tab { current[task] = tabs[task]?.last ?? .journal }
    }
}

/// One Tab of a Task. Sessions and Documents are records; a Terminal lives only while open.
enum TabItem: Hashable, Identifiable, Codable {
    case journal
    case session(String)
    case document(String)
    case terminal(UUID)
    /// A Claude subagent's own Transcript, read-only, opened from its parent's Conversation.
    case subagent(session: String, path: String, title: String)

    var id: Self { self }
}
