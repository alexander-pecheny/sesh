import Foundation

/// Where a Vault moved, as the marker beside its database on the old Host says (ADR 0024).
struct Moved: Codable, Equatable {
    /// The new Host's ssh alias on the Mac that moved the Vault.
    var alias: String?
    var hostname: String?
    var user: String?
    var port: Int?
    /// False while the move runs; a device waits until it is true, then follows.
    var done: Bool?

    var place: String {
        let address = [user, hostname].compactMap { $0 }.joined(separator: "@")
        return [alias, address.isEmpty || address == alias ? nil : "(\(address))"].compactMap { $0 }.joined(separator: " ")
    }

    /// What `ssh -G alias` says of the Host it reaches.
    static func resolved(_ config: String, alias: String) -> Moved {
        var moved = Moved(alias: alias)
        for line in config.split(separator: "\n") {
            let parts = line.split(separator: " ", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { continue }
            switch parts[0].lowercased() {
            case "hostname": moved.hostname = parts[1]
            case "user": moved.user = parts[1]
            case "port": moved.port = Int(parts[1])
            default: break
            }
        }
        return moved
    }

    /// Whether `other`, as this device resolves the same alias, reaches the Host the marker names.
    func reached(by other: Moved) -> Bool {
        let same = { (a: String?, b: String?) in a == nil || b == nil || a?.caseInsensitiveCompare(b ?? "") == .orderedSame }
        return same(hostname, other.hostname) && same(user, other.user) && (port == nil || other.port == nil || port == other.port)
    }

    /// The phone's Host for the marker: the one at its address as its user, else the one named
    /// after its alias.
    func host<H>(among hosts: [H], name: (H) -> String, address: (H) -> String, user: (H) -> String) -> H? {
        let same = { (a: String, b: String?) in b.map { a.caseInsensitiveCompare($0) == .orderedSame } ?? false }
        return hosts.first { same(address($0), hostname) && (self.user == nil || user($0) == self.user) }
            ?? hosts.first { same(name($0), alias) }
    }
}

/// Moving a Vault from one Host to another, as far as it follows from the Vault's records: what
/// moves, and how each record reads on the new Host. The machines' side is in VaultMove.
struct Move {
    let from: String
    let to: String
    let oldHome: String
    let newHome: String

    /// A Worktree on the old Host, carried with its branch and uncommitted work.
    struct Folder: Hashable {
        let repo: String
        let path: String
    }

    /// Where a session's Transcript went on the new Host, as the helper placed it.
    struct Spot: Equatable {
        let path: String
        let cwd: String
    }

    struct Plan {
        /// The open Tasks, whose work on the old Host moves with the Vault.
        var tasks: [Record] = []
        /// Their Agent sessions on the old Host, which come back ended on the new one.
        var sessions: [Record] = []
        /// Every repository those use there, each cloned on the new Host if it lacks it.
        var repos: [String] = []
        var worktrees: [Folder] = []
        /// Their Terminals on the old Host, which close: a shell cannot move.
        var terminals: [Record] = []
        /// The Claude account folders the sessions used, as paths on the old Host.
        var accounts: [String] = []
        var agents: Set<Agent> = []
    }

    /// What moves: the open Tasks' records on the old Host. Archived Tasks keep theirs, and a
    /// session in one resumes elsewhere from the Vault's copy as any ended session does.
    static func plan(_ records: [Record], from alias: String) -> Plan {
        var plan = Plan()
        let live = records.filter { !$0.deleted }.sorted { $0.id < $1.id }
        plan.tasks = live.filter { $0.kind == .task && $0.body.archived != true }
        let open = Set(plan.tasks.map(\.id))
        let here = { (record: Record) in record.body.machine == alias && record.body.task.map(open.contains) == true }
        plan.sessions = live.filter { $0.kind == .session && here($0) }
        plan.terminals = live.filter { $0.kind == .tab && $0.body.kind == "terminal" && here($0) }
        let own = plan.tasks.filter { $0.body.machine == alias }.compactMap { task in
            task.body.path.map { Folder(repo: task.body.repo ?? $0, path: $0) }
        }
        let others = plan.sessions.compactMap { session in
            session.body.repo.flatMap { repo in session.body.path.map { Folder(repo: repo, path: $0) } }
        }
        var seen = Set<String>(), repos = Set<String>()
        for folder in own + others {
            if repos.insert(folder.repo).inserted { plan.repos.append(folder.repo) }
            if folder.path != folder.repo, seen.insert(folder.path).inserted { plan.worktrees.append(folder) }
        }
        var accounts = Set<String>()
        for session in plan.sessions {
            guard let agent = session.body.agent.flatMap(Agent.init) else { continue }
            plan.agents.insert(agent)
            if agent == .claude, let account = session.body.transcripts?.last.flatMap(account(of:)), accounts.insert(account).inserted {
                plan.accounts.append(account)
            }
        }
        return plan
    }

    /// The Claude account folder a Transcript sits in, `~/.claude-work` for
    /// `~/.claude-work/projects/<project>/<id>.jsonl`.
    static func account(of transcript: String) -> String? {
        transcript.range(of: "/projects/", options: .backwards).map { String(transcript[..<$0.lowerBound]) }
    }

    /// `path` on the new Host: the same place under its home when it was under the old one.
    func rehome(_ path: String) -> String {
        guard path == oldHome || path.hasPrefix(oldHome + "/") else { return path }
        return newHome + path.dropFirst(oldHome.count)
    }

    /// A Claude account folder as this device keeps it, written from ~ so it holds on any Host.
    static func portable(_ folder: String) -> String {
        let parts = folder.split(separator: "/", omittingEmptySubsequences: false)
        guard parts.count >= 3, parts[0].isEmpty, ["home", "Users"].contains(parts[1]) else { return folder }
        return (["~"] + parts.dropFirst(3)).joined(separator: "/")
    }

    /// Every record that reads differently once the Vault is on the new Host. A session takes the
    /// Transcript placed for it and comes back ended; a Terminal goes, and its Task's strip with it.
    func rewrite(_ records: [Record], placed: [String: Spot]) -> [Record] {
        let plan = Self.plan(records, from: from)
        var changed: [Record] = []
        for var task in plan.tasks where task.body.machine == from {
            task.body.machine = to
            task.body.path = task.body.path.map(rehome)
            task.body.repo = task.body.repo.map(rehome)
            task.body.workspace = nil
            changed.append(task)
        }
        for var session in plan.sessions {
            let spot = placed[session.id]
            session.body.machine = to
            session.body.path = spot?.cwd ?? session.body.path.map(rehome)
            session.body.repo = session.body.repo.map(rehome)
            session.body.pane = nil
            session.body.name = nil
            var transcripts = (session.body.transcripts ?? []).map(rehome)
            if let spot {
                if !transcripts.isEmpty { transcripts.removeLast() }
                transcripts.append(spot.path)
            }
            session.body.transcripts = transcripts.isEmpty ? nil : transcripts
            changed.append(session)
        }
        let open = Set(plan.tasks.map(\.id))
        for var document in records where document.kind == .document && !document.deleted && document.body.machine == from
            && document.body.task.map(open.contains) == true {
            document.body.machine = to
            document.body.path = document.body.path.map(rehome)
            changed.append(document)
        }
        let gone = Set(plan.terminals.map(\.id))
        for var terminal in plan.terminals {
            terminal.deleted = true
            changed.append(terminal)
        }
        for var strip in records where strip.kind == .tab && strip.body.kind == "strip" && !strip.deleted {
            let tabs = strip.body.tabs ?? []
            let kept = tabs.filter { if case .terminal(let id) = $0 { !gone.contains(id) } else { true } }
            guard kept != tabs else { continue }
            strip.body.tabs = kept
            changed.append(strip)
        }
        return changed
    }

    /// git that fails at once rather than ask for a password no one is there to type.
    static let quiet = "GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -o BatchMode=yes -o ConnectTimeout=10'"

    /// Prints the Worktree's commit and branch, and whether it bundled commits no remote has, then
    /// writes its uncommitted and untracked work as a binary diff, leaving git's own index alone.
    static func pack(_ path: String, into out: String) -> String {
        """
        set -e
        cd \(shellPath(path))
        mkdir -p \(shellPath(out))
        git rev-parse HEAD
        branch=$(git symbolic-ref -q --short HEAD || :)
        printf '%s\\n' "$branch"
        ref=HEAD; [ -z "$branch" ] || ref="refs/heads/$branch"
        if git bundle create \(shellPath(out + "/commits.bundle")) "$ref" --not --remotes >/dev/null 2>&1; then echo bundle; else echo none; fi
        index=$(mktemp); rm -f "$index"
        cp "$(git rev-parse --git-path index)" "$index" 2>/dev/null || GIT_INDEX_FILE="$index" git read-tree HEAD
        GIT_INDEX_FILE="$index" git add -A
        GIT_INDEX_FILE="$index" git diff --cached --binary HEAD > \(shellPath(out + "/changes.patch"))
        rm -f "$index"
        """
    }

    /// Makes the Worktree at `path` on `branch` at `head`, from origin or the bundle, and applies
    /// the diff; one that does not apply waits at `keep`.
    static func unpack(repo: String, path: String, head: String, branch: String, bundle: String?, patch: String, keep: String) -> String {
        let ref = branch.isEmpty ? "HEAD" : "refs/heads/\(branch)"
        let local = quote("refs/heads/\(branch)")
        let fetch = bundle.map { "git fetch -q \(shellPath($0)) \(quote("+\(ref):refs/sesh/moved"))" } ?? ":"
        let checkout = branch.isEmpty ? "--detach \(shellPath(path)) \(head)" : "\(shellPath(path)) \(quote(branch))"
        return """
        set -e
        cd \(shellPath(repo))
        \(quiet) git fetch -q origin 2>/dev/null || :
        \(fetch)
        git cat-file -e \(head)^{commit} 2>/dev/null || { echo "Its commit \(head) came neither from origin nor with the bundle." >&2; exit 1; }
        if [ -n \(quote(branch)) ]; then
            if git show-ref -q --verify \(local); then
                tip=$(git rev-parse \(local))
                if [ "$tip" != \(head) ]; then
                    git merge-base --is-ancestor "$tip" \(head) || { echo "Its branch \(branch) is there already with other commits." >&2; exit 1; }
                    git branch -f \(quote(branch)) \(head)
                fi
            else
                git branch \(quote(branch)) \(head)
            fi
        fi
        git update-ref -d refs/sesh/moved 2>/dev/null || :
        mkdir -p "$(dirname \(shellPath(path)))"
        git worktree add -q \(checkout)
        if [ -s \(shellPath(patch)) ] && ! git -C \(shellPath(path)) apply --binary --whitespace=nowarn \(shellPath(patch)); then
            mkdir -p "$(dirname \(shellPath(keep)))" && cp \(shellPath(patch)) \(shellPath(keep))
            echo "The Worktree is there, but its uncommitted changes did not apply; they wait in \(keep)." >&2
            exit 1
        fi
        """
    }

    /// Claude keeps a login in its folder's credentials on Linux and in the Keychain on a Mac,
    /// where only its settings file shows the account.
    static func logins(agents: [Agent], accounts: [String], home: String) -> String {
        var lines = agents.map { "\"${SHELL:-sh}\" -lic 'command -v \($0.rawValue)' </dev/null >/dev/null 2>&1 || echo missing \($0.rawValue)" }
        for account in accounts {
            let settings = account == home + "/.claude" ? home + "/.claude.json" : account + "/.claude.json"
            lines.append("[ -s \(quote(account + "/.credentials.json")) ] || grep -qs oauthAccount \(quote(settings)) || echo out claude \(quote(account))")
        }
        if agents.contains(.codex) { lines.append("[ -s ~/.codex/auth.json ] || echo out codex") }
        if agents.contains(.pi) { lines.append("[ -s ~/.pi/agent/auth.json ] || echo out pi") }
        return lines.joined(separator: "\n")
    }
}
