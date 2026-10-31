import Foundation

/// What Sesh asks of herdr on any machine, through whichever Runner reaches it.
@MainActor
enum Herdr {
    /// The herdr fork as Sesh installs it, with a server of its own apart from any herdr the
    /// user runs (ADR 0009).
    nonisolated static let command = "~/.sesh/bin/herdr-sesh --session sesh"

    /// Starts herdr-sesh in place of a running server, keeping its panes; installed with it.
    nonisolated static let importer = """
        cat > ~/.sesh/bin/herdr-sesh-import <<'EOF'
        #!/bin/sh
        unset HERDR_SOCKET_PATH HERDR_CLIENT_SOCKET_PATH
        HERDR_SESSION=sesh exec "$HOME/.sesh/bin/herdr-sesh" "$@"
        EOF
        chmod 755 ~/.sesh/bin/herdr-sesh-import
        """

    /// Installs herdr's hooks for each Agent configured on the machine that lacks them. Without
    /// them herdr never learns a Claude or Codex session id, and the helper cannot find its Transcript.
    nonisolated static let integrations = """
        hooks=$(\(command) integration status 2>/dev/null)
        for agent in claude codex pi; do
            if [ -d ~/.$agent ] && printf '%s\\n' "$hooks" | grep -q "^$agent: not installed"; then
                \(command) integration install $agent >/dev/null 2>&1
            fi
        done
        true
        """

    /// Shell lines that start Sesh's herdr server where it is down, or fail saying why they
    /// could not. Where it has never run, the user's own herdr server becomes it, panes and
    /// all, so Agents started before herdr-sesh carry on; `upgrade` hands a running one over to
    /// the herdr-sesh just installed the same way.
    nonisolated static func wake(upgrade: Bool) -> String {
        let handoff = "server live-handoff --import-exe ~/.sesh/bin/herdr-sesh-import"
        let user = "env -u HERDR_SOCKET_PATH -u HERDR_SESSION herdr"
        return """
            if \(command) workspace list >/dev/null 2>&1; then
                \(upgrade ? "\(command) \(handoff) >/dev/null 2>&1" : ":")
            else
                if [ ! -e ~/.sesh/herdr-moved ] && \(user) workspace list >/dev/null 2>&1; then
                    if moved=$(\(user) \(handoff) 2>&1); then
                        # Left in place, the old layout would come back in the user's herdr, its
                        # Agents resumed a second time.
                        for i in 1 2 3 4 5; do \(user) workspace list >/dev/null 2>&1 || break; sleep 1; done
                        layout=${XDG_CONFIG_HOME:-$HOME/.config}/herdr/session.json
                        [ ! -e "$layout" ] || mv "$layout" "$layout.moved-to-sesh"
                    else
                        failed=$moved
                    fi
                fi
                if [ -z "${failed-}" ]; then
                    touch ~/.sesh/herdr-moved
                    if ! \(command) workspace list >/dev/null 2>&1; then
                        nohup \(command) server </dev/null >/dev/null 2>&1 &
                    fi
                    for i in 1 2 3 4 5; do \(command) workspace list >/dev/null 2>&1 && break; sleep 1; done
                fi
                \(command) workspace list >/dev/null 2>&1 || { echo "${failed:-herdr-sesh did not start}" >&2; exit 1; }
            fi
            """
    }

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
                       config: String? = nil, remote: Bool = true, on runner: Runner) async -> String? {
        let name = Names.free(wanted, taken: await agentNames(on: runner))
        if agent == .claude, let config, !config.isEmpty {
            let dir = shellPath(config)
            _ = await runner.run("[ -e \(dir)/hooks/herdr-agent-state.sh ] || CLAUDE_CONFIG_DIR=\(dir) \(command) integration install claude")
            _ = await runner.run("\(command) pane run \(quote(pane)) \(quote("export CLAUDE_CONFIG_DIR=\(shellPath(config))"))")
        }
        var ran = await runner.run(
            "\(command) agent start \(quote(name)) --kind \(agent.rawValue) --pane \(quote(pane)) --timeout 60000"
                + " -- \(agent.flags(name, resuming: transcript, remote: remote))")
        let notReady = !ran.ok && (ran.err + ran.out).contains("agent_not_ready")
        if ran.ok || notReady {
            // The user picked this folder and pressed Start, which answers the trust question;
            // updates are skipped and new hooks left for the user to trust.
            var answered = false
            for _ in menus.indices {
                let screen = await runner.run("\(command) pane read \(quote(pane)) --source visible")
                guard let menu = menus.first(where: { screen.out.contains($0.prompt) }) else { break }
                _ = await runner.run("\(command) agent send-keys \(quote(pane)) \(menu.keys)")
                answered = true
                for _ in 0..<20 {
                    try? await Task.sleep(for: .milliseconds(100))
                    if !(await runner.run("\(command) pane read \(quote(pane)) --source visible")).out.contains(menu.prompt) { break }
                }
            }
            // A menu Sesh does not know waits for the user, who answers it in the Conversation.
            if notReady && !answered { return nil }
            if answered {
                ran = await runner.run("\(command) agent wait \(quote(pane)) --until idle --until done --timeout 30000")
            }
        }
        guard !ran.ok else { return nil }
        let screen = await runner.run("\(command) pane read \(quote(pane)) --source recent-unwrapped --lines 40")
        _ = await runner.run("\(command) pane close \(quote(pane))")
        let tail = screen.out.split(separator: "\n").filter { !$0.allSatisfy(\.isWhitespace) }.suffix(12)
        return ([ran.problem] + tail.map(String.init)).joined(separator: "\n")
    }

    struct Opened {
        let workspace: String
        let pane: String
        var path: String?
        var branch: String?
        /// herdr already had a Workspace on this worktree, whose pane may hold an Agent.
        var reused = false
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
            let already_open: Bool?
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
            path: result.worktree?.path, branch: result.worktree?.branch, reused: result.already_open == true))
    }

    struct Failure: Error {
        let message: String
        init(_ message: String) { self.message = message }
    }

    /// `branch` of `repo` in its own worktree, as a herdr Workspace labelled `label`: the worktree
    /// the branch already has, or else a new one.
    static func worktree(_ repo: String, branch: String, label: String, on runner: Runner) async -> Result<Opened, Failure> {
        let args = "--cwd \(quote(repo)) --branch \(quote(branch)) --label \(quote(label)) --no-focus"
        guard case .success(let found) = opened(await runner.run("\(command) worktree open \(args)")) else {
            return opened(await runner.run("\(command) worktree create \(args)"))
        }
        guard found.reused, let path = found.path else { return .success(found) }
        return await tab(in: found.workspace, folder: path, label: label, on: runner).map {
            Opened(workspace: found.workspace, pane: $0.pane, path: path, branch: found.branch)
        }
    }

    static func workspace(_ folder: String, label: String, on runner: Runner) async -> Result<Opened, Failure> {
        opened(await runner.run("\(command) workspace create --cwd \(quote(folder)) --label \(quote(label)) --no-focus"))
    }

    /// Another pane in an existing Workspace, for a second Agent session or a Terminal.
    static func tab(in workspace: String, folder: String, label: String, on runner: Runner) async -> Result<Opened, Failure> {
        let ran = await runner.run(
            "\(command) tab create --workspace \(quote(workspace)) --cwd \(quote(folder)) --label \(quote(label)) --no-focus")
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
        let ran = await runner.run("\(command) pane get \(quote(pane))")
        if let got = try? JSONDecoder().decode(Got.self, from: Data(ran.out.utf8)) { return .found(got.result.pane) }
        return (ran.err + ran.out).contains("pane_not_found") ? .gone : .unreachable(ran.problem)
    }

    /// The command that shows a pane's terminal, taken over from any other device showing it.
    static func attach(_ terminal: String) -> String { "\(command) terminal attach \(quote(terminal)) --takeover" }

    /// Ends a Terminal's shell.
    static func close(_ pane: String, on runner: Runner) async {
        _ = await runner.run("\(command) pane close \(quote(pane))")
    }

    /// The link `--remote-control` prints when it starts, which opens the Claude app on
    /// exactly this conversation.
    static func claudeLink(for pane: String, on runner: Runner) async -> URL? {
        let ran = await runner.run(
            "\(command) agent read \(quote(pane)) --source recent-unwrapped --lines 1000"
                + " | grep -o 'https://claude.ai/code/session_[A-Za-z0-9_]*' | tail -n 1")
        return URL(string: ran.out.trimmingCharacters(in: .whitespacesAndNewlines)).flatMap { $0.host == nil ? nil : $0 }
    }

    static func removeWorktree(of workspace: String, force: Bool, on runner: Runner) async -> Ran {
        await runner.run("\(command) worktree remove --workspace \(quote(workspace))\(force ? " --force" : "")")
    }

    static func closeWorkspace(_ workspace: String, on runner: Runner) async {
        _ = await runner.run("\(command) workspace close \(quote(workspace))")
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
        let ran = await runner.run("\(command) agent list")
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
        let ran = await runner.run("\(command) workspace list")
        let list = try? JSONDecoder().decode(List.self, from: Data(ran.out.utf8))
        return Dictionary(list?.result.workspaces.map { ($0.workspace_id, $0.label) } ?? [], uniquingKeysWith: { a, _ in a })
    }
}
