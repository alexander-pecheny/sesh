import Foundation

/// What Sesh asks of herdr on any machine, through whichever Runner reaches it.
@MainActor
enum Herdr {
    /// Shell lines that print `missing` where herdr is not installed, and start its server
    /// where it is down.
    nonisolated static let wake = """
        command -v herdr >/dev/null 2>&1 || echo missing
        if command -v herdr >/dev/null 2>&1 && ! herdr workspace list >/dev/null 2>&1; then
            nohup herdr server </dev/null >/dev/null 2>&1 &
            for i in 1 2 3 4 5; do sleep 1; herdr workspace list >/dev/null 2>&1 && break; done
        fi
        """

    /// Menus an Agent may open before its first prompt, and the keys that get past them.
    private static let menus = [
        (prompt: "Yes, I trust this folder", keys: "down enter"),
        (prompt: "Do you trust the contents of this directory", keys: "enter"),
        (prompt: "Skip until next version", keys: "down enter"),
        (prompt: "Continue without trusting", keys: "down down enter"),
    ]

    /// Starts `agent` in a fresh `pane` and waits until it can take a prompt. On failure the
    /// pane is closed, so no half-started Agent lingers, and the reason comes back.
    static func launch(_ agent: Agent, name wanted: String, pane: String, resuming transcript: String? = nil,
                       config: String? = nil, on runner: Runner) async -> String? {
        let name = Names.free(wanted, taken: await agentNames(on: runner))
        if agent == .claude, let config, !config.isEmpty {
            _ = await runner.run("herdr pane run \(quote(pane)) \(quote("export CLAUDE_CONFIG_DIR=\(shellPath(config))"))")
        }
        var ran = await runner.run(
            "herdr agent start \(quote(name)) --kind \(agent.rawValue) --pane \(quote(pane)) --timeout 60000"
                + " -- \(agent.flags(name, resuming: transcript))")
        let notReady = !ran.ok && (ran.err + ran.out).contains("agent_not_ready")
        if ran.ok || notReady {
            // The user picked this folder and pressed Start, which answers the trust question;
            // updates are skipped and new hooks left for the user to trust.
            var answered = false
            for _ in menus.indices {
                let screen = await runner.run("herdr pane read \(quote(pane)) --source visible")
                guard let menu = menus.first(where: { screen.out.contains($0.prompt) }) else { break }
                _ = await runner.run("herdr agent send-keys \(quote(pane)) \(menu.keys)")
                answered = true
                for _ in 0..<20 {
                    try? await Task.sleep(for: .milliseconds(100))
                    if !(await runner.run("herdr pane read \(quote(pane)) --source visible")).out.contains(menu.prompt) { break }
                }
            }
            if answered || notReady {
                ran = await runner.run("herdr agent wait \(quote(pane)) --until idle --until done --timeout 30000")
            }
        }
        guard !ran.ok else { return nil }
        let screen = await runner.run("herdr pane read \(quote(pane)) --source recent-unwrapped --lines 40")
        _ = await runner.run("herdr pane close \(quote(pane))")
        let tail = screen.out.split(separator: "\n").filter { !$0.allSatisfy(\.isWhitespace) }.suffix(12)
        return ([ran.problem] + tail.map(String.init)).joined(separator: "\n")
    }

    struct Opened {
        let workspace: String
        let pane: String
        var path: String?
        var branch: String?
    }

    private struct Response: Decodable {
        struct Result: Decodable {
            struct Workspace: Decodable { let workspace_id: String }
            struct Pane: Decodable { let pane_id: String }
            struct Worktree: Decodable {
                let path: String
                let branch: String?
            }
            let workspace: Workspace?
            let root_pane: Pane?
            let worktree: Worktree?
        }
        let result: Result
    }

    private static func opened(_ ran: Ran) -> Result<Opened, Failure> {
        guard ran.ok else { return .failure(Failure(ran.problem)) }
        guard let result = try? JSONDecoder().decode(Response.self, from: Data(ran.out.utf8)).result,
              let pane = result.root_pane?.pane_id
        else { return .failure(Failure("herdr said something Sesh cannot read")) }
        return .success(Opened(
            workspace: result.workspace?.workspace_id ?? "", pane: pane,
            path: result.worktree?.path, branch: result.worktree?.branch))
    }

    struct Failure: Error {
        let message: String
        init(_ message: String) { self.message = message }
    }

    /// A new branch of `repo` in its own worktree, as a herdr Workspace labelled `label`.
    static func worktree(_ repo: String, branch: String, label: String, on runner: Runner) async -> Result<Opened, Failure> {
        opened(await runner.run(
            "herdr worktree create --cwd \(quote(repo)) --branch \(quote(branch)) --label \(quote(label)) --no-focus"))
    }

    static func workspace(_ folder: String, label: String, on runner: Runner) async -> Result<Opened, Failure> {
        opened(await runner.run("herdr workspace create --cwd \(quote(folder)) --label \(quote(label)) --no-focus"))
    }

    /// Another pane in an existing Workspace, for a second Agent session or a Terminal.
    static func tab(in workspace: String, folder: String, label: String, on runner: Runner) async -> Result<Opened, Failure> {
        let ran = await runner.run(
            "herdr tab create --workspace \(quote(workspace)) --cwd \(quote(folder)) --label \(quote(label)) --no-focus")
        return opened(ran).map { Opened(workspace: workspace, pane: $0.pane) }
    }

    /// What herdr says of a pane.
    struct Pane: Decodable {
        struct Session: Decodable { let path: String? }
        /// The id `herdr terminal attach` takes.
        let terminal_id: String
        /// The Agent running in it; a pane whose Agent exited is a bare shell.
        let agent: String?
        let agent_session: Session?
    }

    enum Lookup {
        case found(Pane)
        /// herdr answered that it has no such pane.
        case gone
        /// The machine or herdr did not answer, which says nothing of the pane.
        case unreachable(String)
    }

    static func look(for pane: String, on runner: Runner) async -> Lookup {
        struct Got: Decodable {
            struct Result: Decodable { let pane: Pane }
            let result: Result
        }
        let ran = await runner.run("herdr pane get \(quote(pane))")
        if let got = try? JSONDecoder().decode(Got.self, from: Data(ran.out.utf8)) { return .found(got.result.pane) }
        return (ran.err + ran.out).contains("pane_not_found") ? .gone : .unreachable(ran.problem)
    }

    /// The command that shows a pane's terminal, taken over from any other device showing it.
    static func attach(_ terminal: String) -> String { "herdr terminal attach \(quote(terminal)) --takeover" }

    /// Ends a Terminal's shell.
    static func close(_ pane: String, on runner: Runner) async {
        _ = await runner.run("herdr pane close \(quote(pane))")
    }

    /// The link `--remote-control` prints when it starts, which opens the Claude app on
    /// exactly this conversation.
    static func claudeLink(for pane: String, on runner: Runner) async -> URL? {
        let ran = await runner.run(
            "herdr agent read \(quote(pane)) --source recent-unwrapped --lines 1000"
                + " | grep -o 'https://claude.ai/code/session_[A-Za-z0-9_]*' | tail -n 1")
        return URL(string: ran.out.trimmingCharacters(in: .whitespacesAndNewlines)).flatMap { $0.host == nil ? nil : $0 }
    }

    static func removeWorktree(of workspace: String, force: Bool, on runner: Runner) async -> Ran {
        await runner.run("herdr worktree remove --workspace \(quote(workspace))\(force ? " --force" : "")")
    }

    static func closeWorkspace(_ workspace: String, on runner: Runner) async {
        _ = await runner.run("herdr workspace close \(quote(workspace))")
    }

    /// The names herdr's running Agents go by.
    static func agentNames(on runner: Runner) async -> Set<String> {
        struct List: Decodable {
            struct Result: Decodable {
                struct Agent: Decodable { let name: String? }
                let agents: [Agent]
            }
            let result: Result
        }
        let ran = await runner.run("herdr agent list")
        let list = try? JSONDecoder().decode(List.self, from: Data(ran.out.utf8))
        return Set(list?.result.agents.compactMap(\.name) ?? [])
    }

    /// The Workspaces herdr has open right now, by id, with their labels.
    static func workspaces(on runner: Runner) async -> [String: String] {
        struct List: Decodable {
            struct Result: Decodable {
                struct Workspace: Decodable {
                    let workspace_id: String
                    let label: String
                }
                let workspaces: [Workspace]
            }
            let result: Result
        }
        let ran = await runner.run("herdr workspace list")
        let list = try? JSONDecoder().decode(List.self, from: Data(ran.out.utf8))
        return Dictionary(list?.result.workspaces.map { ($0.workspace_id, $0.label) } ?? [], uniquingKeysWith: { a, _ in a })
    }
}
