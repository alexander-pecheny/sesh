import PhotosUI
import SwiftUI

/// The coding agents Projects can start and show. Anything else herdr finds stays hidden.
enum Agent: String, CaseIterable, Identifiable {
    case claude, codex, pi

    var id: String { rawValue }
    var title: String { self == .pi ? "pi" : rawValue.capitalized }

    /// Permissions are always bypassed: she cannot be at the terminal to approve them.
    func flags(_ name: String) -> String {
        switch self {
        case .claude: "--remote-control \(quote(name)) --dangerously-skip-permissions"
        case .codex: "--dangerously-bypass-approvals-and-sandbox"
        case .pi: ""
        }
    }

    var install: String {
        switch self {
        case .claude: "curl -fsSL https://claude.ai/install.sh | bash"
        case .codex: "npm install -g @openai/codex"
        case .pi: "npm install -g @mariozechner/pi-coding-agent"
        }
    }
}

/// A Projects Tab: one Link to the Host, on which it lists folders and drives herdr to run
/// Agent sessions. Core callbacks arrive on tokio threads and hop to main in `LinkBridge`.
@MainActor
final class Projects: ObservableObject, Identifiable {
    struct Ran {
        let status: Int32
        let out: String
        let err: String
        var ok: Bool { status == 0 }

        /// herdr prints its errors as JSON on stderr; anything else is shown as it came.
        var problem: String {
            let herdr = try? JSONDecoder().decode(HerdrError.self, from: Data(err.utf8))
            let text = herdr?.error.message ?? err.trimmingCharacters(in: .whitespacesAndNewlines)
            return text.isEmpty ? "the command failed with status \(status)" : text
        }
    }

    struct AgentSession: Identifiable, Equatable {
        let id: String
        let name: String
        let agent: Agent
        let pane: String
        let state: String
        let folder: String
        let cwd: String
        let branch: String?
        let workspace: Workspace
        let activity: UInt64
    }

    struct Workspace: Hashable, Identifiable {
        let id: String
        let label: String
        let number: Int
        var repo: String?
        var linked = false
    }

    /// A row of herdr's sidebar: a lone Workspace, or a repo's main checkout heading its branch copies.
    struct WorkspaceGroup: Identifiable {
        let head: Workspace
        let children: [Workspace]
        var id: String { head.id }
    }

    /// herdr's own rule: a repo groups once it has two Workspaces and one is the main checkout.
    static func groups(_ spaces: [Workspace]) -> [WorkspaceGroup] {
        let members = Dictionary(grouping: spaces.filter { $0.repo != nil }, by: { $0.repo! })
        var emitted = Set<String>()
        return spaces.compactMap { space in
            guard let repo = space.repo, let group = members[repo], group.count > 1,
                  let head = group.first(where: { !$0.linked })
            else { return WorkspaceGroup(head: space, children: []) }
            guard emitted.insert(repo).inserted else { return nil }
            return WorkspaceGroup(head: head, children: group.filter { $0 != head })
        }
    }

    /// herdr's state_change_seq is one counter across all agents and moves only on a real change.
    var homeGroups: [WorkspaceGroup] {
        let latest = Dictionary(sessions.map { ($0.workspace.id, $0.activity) }, uniquingKeysWith: max)
        func key(_ group: WorkspaceGroup) -> (Int, UInt64, String, Int) {
            let seq = ([group.head] + group.children).compactMap { latest[$0.id] }.max()
            return (seq == nil ? 1 : 0, UInt64.max - (seq ?? 0), group.head.label.lowercased(), group.head.number)
        }
        return Self.groups(workspaces).sorted { key($0) < key($1) }
    }

    struct Failure: Error {
        let message: String
    }

    struct Listing {
        let git: Bool
        let folders: [String]
    }

    enum Place: Hashable {
        case folder(String)
        case conversation(pane: String, fresh: Bool)
    }

    enum Stage: Equatable {
        case connecting, authenticating, checking, ready, failed
    }

    @Published private(set) var stage = Stage.connecting
    @Published private(set) var message = ""
    @Published var hostKeyQuestion: SeshSession.HostKeyQuestion?
    @Published var authQuestion: SeshSession.AuthQuestion?
    @Published var savePassword = false
    @Published private(set) var home = ""
    @Published private(set) var missing: [String] = []
    /// What `uname -sm` printed on a Host Sesh carries no transcript helper for.
    @Published private(set) var unsupported: String?
    @Published private(set) var sessions: [AgentSession] = []
    @Published private(set) var workspaces: [Workspace] = []
    @Published var route: [Place] = []
    @Published private(set) var uploading: SeshSession.Progress?
    @Published var uploadError: String?

    let id = UUID()
    let host: Host
    private let store: Store
    private var handle: OpaquePointer?
    private var bridge = LinkBridge()
    private var waiting: [UInt32: CheckedContinuation<Ran, Never>] = [:]
    private var streams: [UInt32: (buffer: Data, line: (String) -> Void)] = [:]
    private var linking: [CheckedContinuation<Bool, Never>] = []
    private var linked = false
    private var paused = false
    private var links: [String: URL] = [:]
    private var uploaded: (([String], String?) -> Void)?

    init(host: Host, store: Store) {
        self.host = host
        self.store = store
        savePassword = Keychain.read("password.\(host.id)") != nil
        connect()
    }

    var name: String { host.title }

    var status: String {
        switch stage {
        case .connecting: "connecting"
        case .authenticating: "authenticating"
        case .checking: "checking the Host"
        case .ready: linked ? "Projects" : "reconnecting"
        case .failed: message.isEmpty ? "disconnected" : message
        }
    }

    var canBranch: Bool { !missing.contains("git") }

    var needsHerdr: Bool { missing.contains("herdr") }

    /// Where Sesh's transcript helper lives on every Host (ADR 0006).
    static let helper = "~/.sesh/bin/sesh-transcript"
    private static let helperVersion = Bundle.main.url(forResource: "version", withExtension: nil, subdirectory: "helpers")
        .flatMap { try? String(contentsOf: $0, encoding: .utf8) }?
        .trimmingCharacters(in: .whitespacesAndNewlines)

    /// Also called on the way to the background: iOS would freeze the socket, and a command
    /// sent on it after waking would wait minutes for TCP to admit it is dead.
    func close() {
        paused = true
        drop("the connection was closed")
    }

    /// Once the Host is checked, losing the connection costs nothing visible: the screens
    /// stay put and the next command dials again before it runs.
    func resume() {
        paused = false
        if handle == nil { connect() }
    }

    private func drop(_ problem: String) {
        if let handle {
            sesh_session_close(handle)
            sesh_session_free(handle)
        }
        handle = nil
        linked = false
        finishAll(problem)
        let pending = linking
        linking = []
        pending.forEach { $0.resume(returning: false) }
    }

    private func connect() {
        if stage != .ready {
            stage = .connecting
            message = ""
        }
        bridge.owner = nil
        bridge = LinkBridge()
        bridge.owner = self

        let key = store.key(host.keyID)
        let strings = CStrings()
        var config = sesh_link_config_t(
            host: strings.make(host.address), port: UInt16(host.port), user: strings.make(host.user),
            password: strings.make(Keychain.read("password.\(host.id)")),
            key_pem: strings.make(key.flatMap { Keychain.read("key.\($0.id)") }),
            key_passphrase: strings.make(key.flatMap { Keychain.read("passphrase.\($0.id)") }),
            known_hosts_path: strings.make(store.knownHostsPath),
            extra_flags: strings.make(host.transport == .ssh ? host.sshFlags : host.moshFlags),
            transport: host.transport == .ssh ? SESH_TRANSPORT_SSH : SESH_TRANSPORT_MOSH)
        handle = sesh_link_connect(&config, LinkBridge.callbacks, Unmanaged.passRetained(bridge).toOpaque())
    }

    // MARK: Commands on the Host

    func run(_ command: String) async -> Ran { await call { sesh_session_run($0, command) } }

    /// Runs `command` until it exits or the calling task is cancelled, handing each line of
    /// its stdout to `line` as it arrives.
    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran {
        await call(line: line) { sesh_session_stream($0, command) }
    }

    private func call(line: ((String) -> Void)? = nil, start: (OpaquePointer) -> UInt32) async -> Ran {
        guard !paused else { return Ran(status: -1, out: "", err: "not connected") }
        if !linked {
            if handle == nil { connect() }
            guard await withCheckedContinuation({ linking.append($0) }) else {
                return Ran(status: -1, out: "", err: message.isEmpty ? "not connected" : message)
            }
        }
        guard let handle, !Task.isCancelled else { return Ran(status: -1, out: "", err: "not connected") }
        let id = start(handle)
        guard id != 0 else { return Ran(status: -1, out: "", err: "not connected") }
        if let line { streams[id] = (Data(), line) }
        return await withTaskCancellationHandler {
            await withCheckedContinuation { waiting[id] = $0 }
        } onCancel: {
            Task { @MainActor [weak self] in
                guard let self, let handle = self.handle else { return }
                sesh_session_cancel(handle, id)
            }
        }
    }

    private func check() async {
        stage = .checking
        let ran = await run("""
            for t in herdr claude codex pi git; do command -v "$t" >/dev/null 2>&1 || printf '%s ' "$t"; done
            echo; printf '%s\\n' "$HOME"
            uname -sm
            \(Self.helper) --version 2>/dev/null || echo
            if command -v herdr >/dev/null 2>&1 && ! herdr workspace list >/dev/null 2>&1; then
                nohup herdr server </dev/null >/dev/null 2>&1 &
                for i in 1 2 3 4 5; do sleep 1; herdr workspace list >/dev/null 2>&1 && break; done
            fi
            """)
        guard stage == .checking else { return }
        let lines = ran.out.components(separatedBy: "\n")
        guard ran.ok, lines.count > 3 else { return fail(ran.problem) }
        missing = lines[0].split(separator: " ").map(String.init)
        home = lines[1]
        if !needsHerdr, let problem = await install(platform: lines[2], found: lines[3]) { return fail(problem) }
        guard stage == .checking else { return }
        stage = .ready
        await refresh()
    }

    /// Copies the helper built for this Host's platform unless the same version is there.
    private func install(platform: String, found: String) async -> String? {
        let name = "sesh-transcript-" + platform.lowercased().replacingOccurrences(of: " ", with: "-")
        guard let bundled = Bundle.main.url(forResource: name, withExtension: "gz", subdirectory: "helpers") else {
            unsupported = platform
            return nil
        }
        guard found != Self.helperVersion else { return nil }
        let remote = String(Self.helper.dropFirst(2))
        var ran = await call { sesh_session_put($0, bundled.path, remote + ".gz") }
        if ran.ok { ran = await run("gunzip -f \(Self.helper).gz && chmod 755 \(Self.helper)") }
        return ran.ok ? nil : "Sesh could not copy its helper to the Host: \(ran.problem)"
    }

    func refresh() async {
        guard stage == .ready, !needsHerdr, unsupported == nil else { return }
        let ran = await run("herdr agent list && herdr workspace list")
        let lines = ran.out.split(separator: "\n").map { Data($0.utf8) }
        guard ran.ok, lines.count == 2,
              let agents = try? JSONDecoder().decode(Herdr<AgentList>.self, from: lines[0]).result.agents,
              let workspaces = try? JSONDecoder().decode(Herdr<WorkspaceList>.self, from: lines[1]).result.workspaces
        else { return }
        self.workspaces = workspaces.map(Workspace.init).sorted { $0.number < $1.number }
        let byId = Dictionary(workspaces.map { ($0.workspace_id, $0) }, uniquingKeysWith: { first, _ in first })
        sessions = agents.compactMap { agent in
            guard let kind = Agent(rawValue: agent.agent) else { return nil }
            let space = byId[agent.workspace_id]
            let worktree = space?.worktree.flatMap { $0.is_linked_worktree ? $0 : nil }
            return AgentSession(
                id: agent.pane_id,
                name: agent.name ?? (agent.cwd as NSString).lastPathComponent,
                agent: kind,
                pane: agent.pane_id,
                state: agent.agent_status,
                folder: worktree?.repo_root ?? agent.cwd,
                cwd: agent.cwd,
                branch: worktree.map { ($0.checkout_path as NSString).lastPathComponent },
                workspace: space.map(Workspace.init)
                    ?? Workspace(id: agent.workspace_id, label: agent.workspace_id, number: .max),
                activity: agent.state_change_seq)
        }
        .sorted { ($0.workspace.number, $0.name) < ($1.workspace.number, $1.name) }
    }

    func list(_ folder: String) async -> Result<Listing, Failure> {
        let ran = await run("""
            cd -- \(quote(folder)) || exit 1
            if [ -e .git ]; then echo git; else echo plain; fi
            find . -mindepth 1 -maxdepth 1 -type d ! -name '.*'
            """)
        guard ran.ok else { return .failure(Failure(message: ran.problem)) }
        var lines = ran.out.split(separator: "\n").map(String.init)
        let git = lines.first == "git"
        lines.removeFirst(min(1, lines.count))
        let folders = lines.map { String($0.dropFirst(2)) }.sorted { $0.localizedStandardCompare($1) == .orderedAscending }
        return .success(Listing(git: git, folders: folders))
    }

    func makeFolder(_ name: String, in parent: String) async -> String? {
        let ran = await run("mkdir -- \(quote(parent + "/" + name))")
        return ran.ok ? nil : ran.problem
    }

    /// Menus an Agent may open before its first prompt, and the keys that get past them.
    private static let menus = [
        (prompt: "Yes, I trust this folder", keys: "down enter"),
        (prompt: "Do you trust the contents of this directory", keys: "enter"),
        (prompt: "Skip until next version", keys: "down enter"),
        (prompt: "Continue without trusting", keys: "down down enter"),
    ]

    /// Starts an Agent session in `folder`, on a new branch of it, or in `workspace`, and returns
    /// its pane. The pane is closed on failure so no half-started Agent lingers.
    func start(
        _ name: String, agent: Agent, in folder: String, branch: Bool, workspace: String? = nil
    ) async -> Result<String, Failure> {
        let pane: String
        switch await paneFor(name, in: folder, branch: branch, workspace: workspace) {
        case .failure(let failure): return .failure(failure)
        case .success(let id): pane = id
        }
        var ran = await run(
            "herdr agent start \(quote(name)) --kind \(agent.rawValue) --pane \(quote(pane)) --timeout 60000"
                + " -- \(agent.flags(name))")
        let notReady = !ran.ok && (ran.err + ran.out).contains("agent_not_ready")
        if ran.ok || notReady {
            // She picked this folder and pressed Start, which answers the trust question;
            // updates are skipped and new hooks left for her to trust.
            var answered = false
            for _ in Self.menus.indices {
                let screen = await run("herdr pane read \(quote(pane)) --source visible")
                guard let keys = Self.menus.first(where: { screen.out.contains($0.prompt) })?.keys else { break }
                _ = await run("herdr agent send-keys \(quote(pane)) \(keys)")
                answered = true
                try? await Task.sleep(for: .seconds(2))
            }
            if answered || notReady {
                ran = await run("herdr agent wait \(quote(pane)) --until idle --until done --timeout 30000")
            }
        }
        guard ran.ok else {
            let screen = await run("herdr pane read \(quote(pane)) --source recent-unwrapped --lines 40")
            _ = await run("herdr pane close \(quote(pane))")
            let tail = screen.out.split(separator: "\n").filter { !$0.allSatisfy(\.isWhitespace) }.suffix(12)
            return .failure(Failure(message: ([ran.problem] + tail.map(String.init)).joined(separator: "\n")))
        }
        if agent == .claude { _ = await link(for: pane) }
        await refresh()
        return .success(pane)
    }

    private func paneFor(
        _ name: String, in folder: String, branch: Bool, workspace chosen: String?
    ) async -> Result<String, Failure> {
        if branch {
            let ran = await run(
                "herdr worktree create --cwd \(quote(folder)) --branch \(quote(name)) --no-focus")
            return rootPane(ran)
        }
        var workspace = chosen
        var folder = folder
        if workspace == nil || folder.isEmpty {
            let panes = (try? JSONDecoder().decode(Herdr<PaneList>.self, from: Data(await run("herdr pane list").out.utf8)))?
                .result.panes ?? []
            if let workspace {
                folder = panes.first { $0.workspace_id == workspace }?.cwd ?? home
            } else {
                workspace = panes.first { $0.cwd == folder }?.workspace_id
            }
        }
        let ran: Ran
        if let workspace {
            ran = await run(
                "herdr tab create --workspace \(quote(workspace)) --cwd \(quote(folder)) --label \(quote(name)) --no-focus")
        } else {
            let label = (folder as NSString).lastPathComponent
            ran = await run("herdr workspace create --cwd \(quote(folder)) --label \(quote(label)) --no-focus")
        }
        return rootPane(ran)
    }

    private func rootPane(_ ran: Ran) -> Result<String, Failure> {
        guard ran.ok else { return .failure(Failure(message: ran.problem)) }
        guard let created = try? JSONDecoder().decode(Herdr<Created>.self, from: Data(ran.out.utf8)) else {
            return .failure(Failure(message: "herdr said something Sesh cannot read"))
        }
        return .success(created.result.root_pane.pane_id)
    }

    /// The link `--remote-control` prints when it starts, which opens the Claude app on
    /// exactly this conversation.
    func link(for pane: String) async -> URL? {
        if let known = links[pane] { return known }
        let ran = await run(
            "herdr agent read \(quote(pane)) --source recent-unwrapped --lines 1000"
                + " | grep -o 'https://claude.ai/code/session_[A-Za-z0-9_]*' | tail -n 1")
        let found = URL(string: ran.out.trimmingCharacters(in: .whitespacesAndNewlines))
        if let found, found.host != nil { links[pane] = found }
        return links[pane]
    }

    func stop(_ session: AgentSession) async {
        let pane = quote(session.pane)
        _ = await run("herdr agent send-keys \(pane) ctrl+c ctrl+c; sleep 1; herdr pane close \(pane)")
        links[session.pane] = nil
        await refresh()
    }

    /// A name herdr accepts as an agent name and git as a branch: the folder's, then two words.
    func suggestName(for folder: String) -> String {
        let taken = Set(sessions.map(\.name))
        let base = String(Self.slug((folder as NSString).lastPathComponent).prefix(14))
        for _ in 0..<50 {
            let words = [Self.adjectives.randomElement()!, Self.nouns.randomElement()!]
            let name = Self.slug(([base] + words).filter { !$0.isEmpty }.joined(separator: "-"))
            if !taken.contains(name) { return name }
        }
        return Self.slug(base + "-\(Int.random(in: 100...999))")
    }

    static func slug(_ text: String) -> String {
        var slug = ""
        for character in text.lowercased() {
            let keep = character.isASCII && (character.isLetter || character.isNumber || character == "_")
            if keep { slug.append(character) } else if !slug.isEmpty, slug.last != "-" { slug.append("-") }
        }
        while slug.last == "-" { slug.removeLast() }
        if let first = slug.first, !first.isLetter { slug = "c-" + slug }
        return String(slug.prefix(32))
    }

    private static let adjectives = [
        "calm", "bright", "quiet", "swift", "gentle", "bold", "warm", "clear", "brave", "lucky",
        "sunny", "misty", "tidy", "lively", "mellow", "crisp", "rosy", "silver", "amber", "cosy",
    ]
    private static let nouns = [
        "stone", "river", "fern", "harbour", "meadow", "comet", "maple", "pebble", "otter", "lantern",
        "willow", "orchid", "summit", "breeze", "canyon", "ember", "finch", "grove", "lagoon", "moss",
    ]

    // MARK: Uploads

    var canUpload: Bool { linked && uploading == nil }

    /// As in a Terminal Tab, but over the Link: the paths reach `insert` once all have landed.
    func upload(_ results: [PHPickerResult], insert: @escaping (String) -> Void) {
        guard canUpload, !results.isEmpty else { return }
        uploading = SeshSession.Progress(id: 0, done: 0, total: 1)
        Task {
            let files: [PreparedUpload]
            do {
                files = try await Uploads.prepare(results, compress: store.compressUploads)
            } catch {
                uploading = nil
                uploadError = error.localizedDescription
                return
            }
            guard uploading != nil, let handle, let id = Uploads.start(files, on: handle) else {
                if uploading != nil { uploadError = "the Upload could not be started" }
                uploading = nil
                return Uploads.discard(files)
            }
            uploading = SeshSession.Progress(id: id, done: 0, total: 1)
            uploaded = { [weak self] paths, error in
                Uploads.discard(files)
                self?.uploading = nil
                self?.uploadError = error
                if !paths.isEmpty { insert(Uploads.text(for: paths)) }
            }
        }
    }

    func cancelUpload() {
        guard let uploading else { return }
        guard uploading.id != 0, let handle else { return self.uploading = nil }
        sesh_session_cancel_upload(handle, uploading.id)
    }

    fileprivate func uploadProgressed(_ id: UInt32, _ done: UInt64, _ total: UInt64) {
        guard uploading?.id == id else { return }
        uploading = SeshSession.Progress(id: id, done: done, total: total)
    }

    fileprivate func uploadDone(_ id: UInt32, _ paths: [String], _ error: String?) {
        guard uploading?.id == id, let finish = uploaded else { return }
        uploaded = nil
        finish(paths, error)
    }

    // MARK: From the core

    private func fail(_ problem: String) {
        if stage != .ready { stage = .failed }
        message = problem
        drop(problem)
    }

    private func finishAll(_ problem: String) {
        let pending = waiting
        waiting = [:]
        streams = [:]
        pending.values.forEach { $0.resume(returning: Ran(status: -1, out: "", err: problem)) }
    }

    fileprivate func ran(_ id: UInt32, _ result: Ran) {
        if let rest = streams.removeValue(forKey: id), !rest.buffer.isEmpty {
            rest.line(String(decoding: rest.buffer, as: UTF8.self))
        }
        waiting.removeValue(forKey: id)?.resume(returning: result)
        if result.status == -1, result.err.hasPrefix("the connection to the Host is gone") { drop(result.err) }
    }

    fileprivate func chunk(_ id: UInt32, _ data: Data) {
        guard var stream = streams[id] else { return }
        stream.buffer.append(data)
        var lines: [String] = []
        while let end = stream.buffer.firstIndex(of: UInt8(ascii: "\n")) {
            lines.append(String(decoding: stream.buffer[..<end], as: UTF8.self))
            stream.buffer.removeSubrange(...end)
        }
        streams[id] = stream
        lines.forEach(stream.line)
    }

    fileprivate func apply(_ state: UInt32, _ message: String) {
        switch state {
        case SESH_STATE_PASSWORD_REJECTED.rawValue:
            Keychain.write(nil, to: "password.\(host.id)")
            savePassword = false
        case SESH_STATE_CONNECTING.rawValue: if stage != .ready { stage = .connecting }
        case SESH_STATE_AUTHENTICATING.rawValue: if stage != .ready { stage = .authenticating }
        case SESH_STATE_CONNECTED.rawValue:
            linked = true
            let pending = linking
            linking = []
            pending.forEach { $0.resume(returning: true) }
            if stage != .ready { Task { await check() } }
        default: fail(message.isEmpty ? "disconnected" : message)
        }
    }

    func answerHostKey(_ accept: Bool) {
        hostKeyQuestion = nil
        guard let handle else { return }
        sesh_session_answer_host_key(handle, accept)
    }

    func answerPrompt(_ question: SeshSession.AuthQuestion, _ answers: [String]?) {
        authQuestion = nil
        guard let handle else { return }
        guard let answers else { return sesh_session_answer_prompt(handle, question.id, nil, 0) }
        if savePassword, let first = answers.first, question.prompts.count == 1 {
            Keychain.write(first, to: "password.\(host.id)")
        }
        let strings = CStrings()
        let pointers = answers.map { strings.make($0) }
        pointers.withUnsafeBufferPointer {
            sesh_session_answer_prompt(handle, question.id, $0.baseAddress, UInt($0.count))
        }
    }

    deinit {
        guard let handle else { return }
        sesh_session_close(handle)
        sesh_session_free(handle)
    }
}

func quote(_ text: String) -> String { "'" + text.replacingOccurrences(of: "'", with: "'\\''") + "'" }

// MARK: What herdr prints

private struct Herdr<Result: Decodable>: Decodable { let result: Result }
private struct HerdrError: Decodable {
    struct Detail: Decodable { let message: String }
    let error: Detail
}
private struct AgentList: Decodable {
    struct Agent: Decodable {
        let agent: String
        let agent_status: String
        let cwd: String
        let name: String?
        let pane_id: String
        let state_change_seq: UInt64
        let workspace_id: String
    }
    let agents: [Agent]
}
private struct WorkspaceList: Decodable {
    struct Worktree: Decodable {
        let checkout_path: String
        let is_linked_worktree: Bool
        let repo_key: String
        let repo_root: String
    }
    struct Workspace: Decodable {
        let workspace_id: String
        let label: String
        let number: Int
        let worktree: Worktree?
    }
    let workspaces: [Workspace]
}
private struct PaneList: Decodable {
    struct Pane: Decodable {
        let cwd: String
        let workspace_id: String
    }
    let panes: [Pane]
}
private struct Created: Decodable {
    struct Pane: Decodable { let pane_id: String }
    let root_pane: Pane
}

private final class LinkBridge {
    weak var owner: Projects?

    static let callbacks = sesh_callbacks_t(
        on_output: nil,
        on_state: { userdata, state, message in
            guard let userdata else { return }
            let text = message.map { String(cString: $0) } ?? ""
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.apply(state.rawValue, text) }
        },
        on_host_key: { userdata, fingerprint, previous in
            guard let userdata, let fingerprint else { return }
            let question = SeshSession.HostKeyQuestion(
                fingerprint: String(cString: fingerprint),
                previous: previous.map { String(cString: $0) })
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.hostKeyQuestion = question }
        },
        on_auth_prompt: { userdata, id, name, instruction, prompts, echoes, count in
            guard let userdata, let prompts, let echoes else { return }
            let question = SeshSession.AuthQuestion(
                id: id,
                title: name.map { String(cString: $0) } ?? "Authentication",
                instruction: instruction.map { String(cString: $0) } ?? "",
                prompts: (0..<Int(count)).map {
                    (prompts[$0].map { String(cString: $0) } ?? "", echoes[$0])
                })
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.authQuestion = question }
        },
        on_upload_progress: { userdata, id, done, total in
            guard let userdata else { return }
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.uploadProgressed(id, done, total) }
        },
        on_upload_done: { userdata, id, paths, count, error in
            guard let userdata else { return }
            let remote = (0..<Int(count)).compactMap { paths?[$0].map { String(cString: $0) } }
            let message = error.map { String(cString: $0) }
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.uploadDone(id, remote, message) }
        },
        on_ran: { userdata, id, status, out, err in
            guard let userdata else { return }
            let result = Projects.Ran(
                status: status,
                out: out.map { String(cString: $0) } ?? "",
                err: err.map { String(cString: $0) } ?? "")
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.ran(id, result) }
        },
        on_chunk: { userdata, id, bytes, len in
            guard let userdata, let bytes else { return }
            let data = Data(bytes: bytes, count: Int(len))
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.chunk(id, data) }
        },
        on_release: { userdata in
            guard let userdata else { return }
            Unmanaged<LinkBridge>.fromOpaque(userdata).release()
        })

    private static func of(_ userdata: UnsafeMutableRawPointer) -> LinkBridge {
        Unmanaged<LinkBridge>.fromOpaque(userdata).takeUnretainedValue()
    }
}

private extension Projects.Workspace {
    init(_ space: WorkspaceList.Workspace) {
        self.init(
            id: space.workspace_id, label: space.label, number: space.number,
            repo: space.worktree?.repo_key, linked: space.worktree?.is_linked_worktree ?? false)
    }
}
