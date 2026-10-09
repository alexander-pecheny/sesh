import Foundation
#if os(macOS)
import AppKit
#else
import UIKit
#endif

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

    /// A Task from its title alone, selected at once; its branch is named in the background.
    @discardableResult
    func newTask(_ title: String, in vault: Vault, parent: String?) -> Record {
        let task = vault.create(.task, .init(
            title: title, parent: parent, position: Tree.next(in: parent, of: vault), edited: Int64(Date().timeIntervalSince1970 * 1000)))
        TaskActions.nameBranch(of: task, in: vault)
        selection = task.id
        return task
    }

    /// Opens the new session's Tab as soon as its pane exists; what the user writes before the
    /// Agent can take a prompt waits in its queue.
    func start(_ agent: Agent, for task: Record, on machine: Machine, repo: String? = nil, problem: @escaping (String) -> Void) {
        guard let vault = vault(of: task.id) else { return }
        let pending = Starting(agent: agent, machine: machine.title)
        starting[task.id, default: []].append(pending)
        Task {
            var shown: Record?
            let result = await TaskActions.startSession(agent, for: task, on: machine, repo: repo, in: vault) { session in
                shown = session
                starting[task.id]?.removeAll { $0 == pending }
                launching.insert(session.id)
                open(.session(session.id), in: task.id)
            }
            starting[task.id]?.removeAll { $0 == pending }
            guard let shown else {
                if case .failure(let failure) = result { problem(failure.message) }
                return
            }
            launching.remove(shown.id)
            let conversation = conversations[shown.id]
            conversation?.starting = false
            switch result {
            case .success: await conversation?.sendQueued()
            case .failure(let failure):
                close(.session(shown.id), in: task.id)
                problem(failure.message)
            }
        }
    }

    /// Sessions whose pane exists but whose Agent cannot take a prompt yet.
    private var launching: Set<String> = []

    private struct Kept: Codable {
        var selection: String?
        var tabs: [String: [TabItem]]
        var current: [String: TabItem]
    }

    /// What was open survives a restart, Terminals included: their shells run in herdr.
    private func keep() {
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
        let cache = Self.cache(of: session.id)
        // A session kept on the device opens from what it last showed, at once; whether its Agent
        // still runs is found out meanwhile.
        let kept = FileManager.default.fileExists(atPath: cache.path) && !stopped.contains(session.id) && !ended(session)
        let running: Bool
        if kept {
            running = true
            Task { [weak self] in
                guard let self, !(await self.runs(session, on: machine)) else { return }
                stopped.insert(session.id)
                reload(session.id)
            }
        } else {
            running = await runs(session, on: machine)
        }
        let conversation: Conversation
        if !running, let vault = vault(of: session.id), let last = session.body.transcripts?.last, let agent {
            let copy = "\(vault.folder)/transcripts/\(session.id)/\((last as NSString).lastPathComponent)"
            conversation = Conversation(source: .file(path: copy, agent: agent), agent: agent, runner: vault.machine)
            conversation.prepare = { _ = await vault.machine.prepare() }
        } else {
            conversation = Conversation(pane: session.body.pane ?? "", agent: agent, runner: machine)
            conversation.prepare = { _ = await machine.prepare() }
            conversation.openPath = { [weak self, weak conversation] path in
                guard let self, let vault = self.vault(of: session.id), let current = vault.records[session.id] else { return }
                let wrote = (conversation?.items ?? []).compactMap { item -> String? in
                if case .entry(let entry) = item { entry.file } else { nil }
            }
            Task { await TaskActions.open(path, from: current, wrote: wrote, in: vault, library: self) }
            }
        }
        if let cached = conversations[session.id] { return cached }
        if case .pane = conversation.source { conversation.cache = cache }
        conversation.starting = launching.contains(session.id)
        conversation.bookmark = { [weak self] entry in self?.bookmark(entry, in: session.id) }
        conversation.copyLink = { [weak self] entry in self?.copyLink(entry, in: session.id) }
        conversation.draft = drafts[session.id] ?? ""
        conversation.saveDraft = { [weak self] text in
            guard let self else { return }
            drafts[session.id] = text.isEmpty ? nil : text
            UserDefaults.standard.set(drafts, forKey: "drafts")
        }
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

    /// Sessions found stopped since the app started, which open from their Vault copy.
    private var stopped: Set<String> = []

    private static func cache(of session: String) -> URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appending(path: "Sesh/conversations/\(session).json")
    }

    /// Whether the session's pane still runs its Agent; a pane whose Agent exited is a bare shell.
    private func runs(_ session: Record, on machine: Machine) async -> Bool {
        _ = await machine.prepare()
        let pane = await machine.run("herdr pane get \(quote(session.body.pane ?? ""))")
        struct Got: Decodable { struct Result: Decodable { struct Pane: Decodable { let agent: String? }; let pane: Pane }; let result: Result }
        return (try? JSONDecoder().decode(Got.self, from: Data(pane.out.utf8)))?.result.pane.agent != nil
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
    private var followers: [String: Task<Void, Never>] = [:]

    /// One session's summary as its machine's follower keeps it (ADR 0012).
    private struct Summary: Decodable {
        let t: String
        let session: String?
        let state: String?
        let agent: String?
        let cwd: String?
        let name: String?
        let done: UInt64?
        let last: String?
        let background: [Conversation.Background]?
    }

    /// Every session's summary, by machine and pane.
    private var summaries: [String: [String: Summary]] = [:]
    private var publishing = false

    /// Every Vault's Host and the Mac, each followed by one stream of session summaries.
    func watchUnfiled() {
        guard watching == nil else { return }
        watching = Task { [weak self] in
            while !Task.isCancelled, let self {
                let machines = Machine.here + self.vaults.compactMap { $0.place.alias == nil ? nil : $0.machine }
                for machine in machines where self.followers[machine.id] == nil {
                    self.followers[machine.id] = Task { [weak self] in await self?.follow(machine) }
                }
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    private func follow(_ machine: Machine) async {
        while !Task.isCancelled {
            _ = await machine.prepare()
            _ = await machine.stream("\(Helper.path) attach --sessions") { [weak self] in self?.summarize($0, on: machine.id) }
            listed.remove(machine.id)
            try? await Task.sleep(for: .seconds(2))
        }
    }

    private func summarize(_ text: String, on machine: String) {
        guard let summary = try? JSONDecoder().decode(Summary.self, from: Data(text.utf8)) else { return }
        switch summary.t {
        case "hello": listed.insert(machine)
        case "session":
            guard let pane = summary.session else { return }
            summaries[machine, default: [:]][pane] = summary
        default: return
        }
        // A working session's status line ticks every second; marks need no more than this.
        guard !publishing else { return }
        publishing = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
            self?.publishing = false
            self?.publish()
        }
    }

    /// The summaries as the marks, the Unfiled lists and the Tasks' order read them.
    private func publish() {
        let adopted = Set(vaults.flatMap { $0.all(.session) }.map(key))
        var states: [String: Live] = [:], found: [String: [Unfiled]] = [:], counts: [String: Int] = [:], over: Set<String> = []
        for (machine, sessions) in summaries {
            for (pane, summary) in sessions {
                guard let state = summary.state, state != "ended" else { continue }
                let key = "\(machine):\(pane)"
                states[key] = Live(status: state == "background" ? "working" : state, done: summary.done ?? 0)
                counts[key] = summary.background?.count ?? 0
                if state == "background" { over.insert(key) }
                if let last = summary.last.flatMap(Self.time), active[key] != last { active[key] = last }
                guard !adopted.contains(key), let agent = summary.agent.flatMap(Agent.init), let cwd = summary.cwd else { continue }
                found[machine, default: []].append(Unfiled(
                    machine: machine.isEmpty ? nil : machine, pane: pane, agent: agent, cwd: cwd,
                    name: summary.name ?? (cwd as NSString).lastPathComponent))
            }
        }
        if unfiled != found { unfiled = found }
        if live != states { live = states }
        if busy != counts { busy = counts }
        turnOver = over
        markSeen()
        sendQueued()
    }

    /// A queued message waits for the Agent's turn to end, which only an open Conversation
    /// watches; one closed by a switch of Task is sent from here.
    private func sendQueued() {
        for (id, conversation) in conversations where !conversation.waiting.isEmpty {
            guard let session = vault(of: id)?.records[id], let state = live[key(session)] else { continue }
            if state.status != "working" || turnOver.contains(key(session)) { Task { await conversation.sendQueued() } }
        }
    }

    // MARK: Marks

    /// What herdr last said of a pane: its status, and how many turns its Agent has finished.
    struct Live: Equatable {
        let status: String
        let done: UInt64
    }

    enum Mark: Int, Comparable {
        case seen, finished, background, working, waiting
        static func < (a: Mark, b: Mark) -> Bool { a.rawValue < b.rawValue }
    }

    @Published private(set) var live: [String: Live] = [:]
    /// How much each idle Claude pane left running in the background, by `machine:pane`.
    @Published private(set) var busy: [String: Int] = [:]
    /// Claude panes whose turn is over, so herdr's "working" there is only background work.
    private var turnOver: Set<String> = []

    /// When each pane's Transcript last had a message, in milliseconds since 1970.
    @Published private(set) var active: [String: Double] = UserDefaults.standard.dictionary(forKey: "active") as? [String: Double] ?? [:] {
        didSet { UserDefaults.standard.set(active, forKey: "active") }
    }

    /// When a Task last changed: the last message in any of its Agent sessions' Transcripts;
    /// a Task without one goes by its Entries and Documents. A folder goes by its newest Task.
    func changed(_ record: Record, in vault: Vault) -> Double {
        if record.kind == .folder {
            return Tree.children(of: record.id, in: vault).map { changed($0, in: vault) }.max() ?? 0
        }
        let parts = vault.records.values.filter { !$0.deleted && $0.body.task == record.id }
        let messages = parts.filter { $0.kind == .session }.compactMap { active[key($0)] }
        if let last = messages.max() { return last }
        let notes = parts.map { Double(max($0.body.at ?? 0, $0.body.edited ?? 0)) }
        return max(Double(record.body.edited ?? 0), notes.max() ?? 0)
    }

    /// Every Task and folder by how recently it changed, for a list to keep while the user
    /// aims at it: a Task moving up as its Agent writes would move under the finger or pointer.
    func order() -> [String: Int] {
        let all = vaults.flatMap { vault in
            (vault.all(.task) + vault.all(.folder)).map { ($0.id, changed($0, in: vault)) }
        }
        return Dictionary(all.sorted { $0.1 > $1.1 }.enumerated().map { ($1.0, $0) }, uniquingKeysWith: { a, _ in a })
    }

    /// The folders and Tasks directly inside `parent` in a kept `order`, those new to it first;
    /// with no order kept, the most recently changed first.
    func children(of parent: String?, in vault: Vault, order: [String: Int]) -> [Record] {
        Tree.children(of: parent, in: vault)
            .map { ($0, changed($0, in: vault)) }
            .sorted { $0.1 > $1.1 }
            .enumerated()
            .sorted { (order[$0.element.0.id] ?? -1, $0.offset) < (order[$1.element.0.id] ?? -1, $1.offset) }
            .map(\.element.0)
    }

    private static let iso: ISO8601DateFormatter = {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return parser
    }()

    private static func time(_ text: String) -> Double? {
        (iso.date(from: text) ?? ISO8601DateFormatter().date(from: text)).map { $0.timeIntervalSince1970 * 1000 }
    }
    /// The finished-turn count the user has seen, by `machine:pane`, so a turn finished while
    /// they looked elsewhere, or while the app was closed, still shows.
    private var seen: [String: UInt64] = UserDefaults.standard.dictionary(forKey: "seen") as? [String: UInt64] ?? [:]

    private func key(_ session: Record) -> String { "\(session.body.machine ?? ""):\(session.body.pane ?? "")" }

    /// An Agent waiting on the user outranks one working, which outranks one finished unseen.
    func mark(of session: Record) -> Mark? {
        guard let state = live[key(session)] else { return nil }
        if state.status == "blocked" { return .waiting }
        let background = busy[key(session), default: 0] > 0
        if state.status == "working" { return background && turnOver.contains(key(session)) ? .background : .working }
        if background { return .background }
        return state.done > seen[key(session), default: state.done] ? .finished : .seen
    }

    /// One mark for each of the Task's running Agents, the most pressing first.
    func marks(ofTask task: String) -> [Mark] {
        (vault(of: task)?.children(.session, task: task).compactMap(mark(of:)) ?? []).sorted(by: >)
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

    /// Interrupts an Unfiled Agent and closes its pane.
    func stop(_ item: Unfiled) async {
        let machine = TaskActions.machine(item.machine)
        _ = await machine.run("herdr agent send-keys \(quote(item.pane)) ctrl+c ctrl+c; sleep 1; herdr pane close \(quote(item.pane))")
        unfiled[item.machine ?? ""]?.removeAll { $0 == item }
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

    /// Attached views of herdr panes, by Terminal record or by Agent session; an attach is a
    /// view only, and the pane keeps running in herdr when it goes.
    let terminals = Terminals()
    /// Which Agent sessions show their own terminal rather than their Conversation.
    @Published var terminalFace: Set<String> = []

    // MARK: Ending and resuming

    /// The machines whose Agents the last poll listed, by alias, "" for the Mac.
    @Published private(set) var listed: Set<String> = []
    /// Agent sessions being started again, which herdr lists only once their Agent is up.
    @Published private(set) var resuming: Set<String> = []
    /// Bumped when a session's Conversation must be loaded afresh, from its pane or its copy.
    @Published private(set) var reloads: [String: Int] = [:]

    /// Whether a session's Agent has stopped: its machine answered and no longer lists it.
    func ended(_ session: Record) -> Bool {
        !resuming.contains(session.id) && listed.contains(session.body.machine ?? "") && live[key(session)] == nil
    }

    /// Stops a session's Agent and closes its pane, its Transcripts copied first; the session,
    /// its Conversation and its Tab stay.
    func end(_ session: Record) async {
        await vault(of: session.id)?.copier.copy([session])
        if let pane = session.body.pane {
            let machine = TaskActions.machine(session.body.machine)
            _ = await machine.run("herdr agent send-keys \(quote(pane)) ctrl+c ctrl+c; sleep 1; herdr pane close \(quote(pane))")
        }
        live[key(session)] = nil
        reload(session.id)
    }

    /// A session the user asked to end while its Agent was busy, until they confirm.
    @Published var ending: Record?
    /// Why the last resume failed, until the user has read it.
    @Published var resumeProblem: String?
    /// What a closed Task's cleanup on its machines could not do.
    @Published var cleanupProblem: String?

    /// Ends an idle Agent at once; a busy one waits for the user to confirm.
    func askToEnd(_ session: Record) {
        if let mark = mark(of: session), mark >= .background { ending = session } else { Task { await end(session) } }
    }

    func resume(_ session: Record) async {
        guard let vault = vault(of: session.id), let task = session.body.task.flatMap({ vault.records[$0] }) else { return }
        resuming.insert(session.id)
        defer { resuming.remove(session.id) }
        if let problem = await TaskActions.resume(vault.records[session.id] ?? session, of: task, in: vault) {
            resumeProblem = problem
            return
        }
        stopped.remove(session.id)
        reload(session.id)
    }

    func reload(_ id: String) {
        conversations[id] = nil
        terminalFace.remove(id)
        reloads[id, default: 0] += 1
    }

    /// A shell in a new herdr pane of the Task's Workspace, recorded so it reopens after a
    /// restart: in the Worktree when the machine has it, else at home.
    func openTerminal(in task: Record, on machine: Machine) async -> String? {
        guard let vault = vault(of: task.id) else { return nil }
        if let problem = await machine.prepare() { return problem }
        switch await TaskActions.pane(for: task, on: machine, label: "Terminal") {
        case .failure(let failure): return failure.message
        case .success(let (opened, folder)):
            let terminal = vault.create(.tab, .init(
                title: "Terminal", task: task.id, position: Date().timeIntervalSince1970,
                machine: machine.alias, path: folder, pane: opened.pane, kind: "terminal"))
            open(.terminal(terminal.id), in: task.id)
            return nil
        }
    }

    /// Closing a Terminal Tab ends its shell; the record goes with it.
    private func closeTerminal(_ id: String) {
        terminals.forget(id)
        guard let vault = vault(of: id), let record = vault.records[id] else { return }
        if let pane = record.body.pane {
            let machine = TaskActions.machine(record.body.machine)
            Task { _ = await machine.run("herdr pane close \(quote(pane))") }
        }
        vault.delete(record)
    }

    // MARK: Bookmarks and links

    private var drafts = UserDefaults.standard.dictionary(forKey: "drafts") as? [String: String] ?? [:]

    /// Items a link asked for in Conversations not open yet.
    private var pendingFocus: [String: String] = [:]

    static func link(vault: String, session: String, item: String) -> URL? {
        URL(string: "sesh://\(vault)/\(session)/\(item)")
    }

    func copyLink(_ entry: Conversation.Entry, in session: String) {
        guard let vault = vault(of: session), let url = Self.link(vault: vault.name, session: session, item: entry.id) else { return }
        #if os(macOS)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(url.absoluteString, forType: .string)
        #else
        UIPasteboard.general.url = url
        #endif
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
    /// of another Task. Whether it was taken.
    @discardableResult
    func receive(_ dropped: String, into task: String) -> Bool {
        if dropped.hasPrefix(Self.unfiledPrefix) {
            let id = String(dropped.dropFirst(Self.unfiledPrefix.count))
            guard let item = unfiled.values.joined().first(where: { $0.id == id }) else { return false }
            adopt(item, into: task)
            return true
        }
        return move(dropped, to: task)
    }

    /// Moves an Agent session or Document to another Task of the same Vault, Tab and all.
    @discardableResult
    func move(_ id: String, to task: String) -> Bool {
        guard let vault = vault(of: id), var record = vault.records[id], record.kind == .session || record.kind == .document,
              let from = record.body.task, from != task, vault.records[task] != nil else { return false }
        record.body.task = task
        vault.write(record)
        let tab: TabItem = record.kind == .session ? .session(id) : .document(id)
        remove(tab, from: from)
        open(tab, in: task)
        return true
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

    /// Moves to the selected Task's next or previous Tab, round from the last to the Journal.
    func cycle(by step: Int) {
        guard let task = selection else { return }
        let all = [TabItem.journal] + (tabs[task] ?? [])
        let index = all.firstIndex(of: current[task] ?? .journal) ?? 0
        current[task] = all[(index + step + all.count) % all.count]
    }

    /// A Tab whose closing would lose something, held until the user confirms.
    struct Closing: Identifiable {
        let tab: TabItem
        let task: String
        let loss: String
        var id: TabItem { tab }
    }

    @Published var closing: Closing?

    /// Closes a Tab; one holding unsaved edits or a shell asks first, unless `confirmed`.
    func close(_ tab: TabItem, in task: String, confirmed: Bool = false) {
        if !confirmed, let loss = loss(closing: tab) { return closing = Closing(tab: tab, task: task, loss: loss) }
        switch tab {
        case .terminal(let id): closeTerminal(id)
        case .document(let id): documents[id] = nil
        default: break
        }
        remove(tab, from: task)
    }

    private func loss(closing tab: TabItem) -> String? {
        switch tab {
        case .terminal: "The shell in this Terminal stops, and anything running in it."
        case .document(let id) where documents[id]?.dirty == true:
            "Your edits to \(vault(of: id)?.records[id]?.body.title ?? "this Document") are not saved."
        default: nil
        }
    }

    /// Takes a Tab off the strip, the one after it chosen in its place, as a browser does.
    private func remove(_ tab: TabItem, from task: String) {
        guard let index = tabs[task]?.firstIndex(of: tab) else { return }
        tabs[task]?.remove(at: index)
        guard current[task] == tab else { return }
        let left = tabs[task] ?? []
        current[task] = left.isEmpty ? .journal : left[min(index, left.count - 1)]
    }

    /// Documents being read or edited, kept while the app runs so a switch of Tab keeps the edits.
    private var documents: [String: DocumentText] = [:]

    func document(_ id: String, in vault: Vault) -> DocumentText {
        if let known = documents[id] { return known }
        let document = DocumentText(vault: vault, id: id)
        documents[id] = document
        return document
    }
}

/// One Tab of a Task. Sessions and Documents are records; a Terminal lives only while open.
enum TabItem: Hashable, Identifiable, Codable {
    case journal
    case session(String)
    case document(String)
    /// A shell in a herdr pane, recorded in the Vault so it reopens after a restart.
    case terminal(String)
    /// A Claude subagent's own Transcript, read-only, opened from its parent's Conversation.
    case subagent(session: String, path: String, title: String)

    var id: Self { self }
}
