import Foundation

/// What Sesh asks of git on any machine, through whichever Runner reaches it.
@MainActor
enum Git {
    /// Whether `path` is a linked worktree, whose `.git` is a file rather than a repository's own.
    static func linked(_ path: String, on runner: Runner) async -> Bool {
        await runner.run("[ -f \(quote(path))/.git ]").ok
    }

    /// What `git status` says is uncommitted in each of `paths` that is a linked worktree, empty
    /// when clean or unreadable, asked in one command.
    static func survey(_ paths: [String], on runner: Runner) async -> [String: String] {
        let script = paths.map {
            "echo --sesh-path--; [ -f \(quote($0))/.git ] && echo linked && git -C \(quote($0)) status --porcelain"
        }.joined(separator: "; ")
        let sections = (await runner.run(script)).out.components(separatedBy: "--sesh-path--\n").dropFirst()
        return Dictionary(zip(paths, sections).compactMap { path, section in
            section.hasPrefix("linked\n") ? (path, section.dropFirst(7).trimmingCharacters(in: .whitespacesAndNewlines)) : nil
        }, uniquingKeysWith: { a, _ in a })
    }

    /// Whether `git status` at `path` answers and finds nothing uncommitted, submodules included.
    static func clean(_ path: String, on runner: Runner) async -> Bool {
        let ran = await runner.run("git -C \(quote(path)) status --porcelain")
        return ran.ok && ran.out.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// The local branches of `repo`; none when it is not a repository yet.
    static func branches(of repo: String, on runner: Runner) async -> Set<String> {
        let ran = await runner.run("git -C \(shellPath(repo)) for-each-ref --format='%(refname:short)' refs/heads")
        return ran.ok ? Set(ran.out.split(separator: "\n").map(String.init)) : []
    }

    /// A new linked worktree of `repo` at `path` on a new `branch`, from `base` or else from what
    /// `repo` has checked out.
    static func addWorktree(_ path: String, branch: String, from base: String?, of repo: String, on runner: Runner) async -> Ran {
        await runner.run("git -C \(quote(repo)) worktree add -b \(quote(branch)) \(quote(path))" + (base.map { " " + quote($0) } ?? ""))
    }

    /// Brings the branch `repo` has checked out up to its upstream, as `git pull --ff-only` would,
    /// so a new Worktree starts from the latest main and the checkout shows what was merged.
    /// git refuses whatever would touch local work; that, or no network, leaves it as it was.
    static func update(_ repo: String, on runner: Runner) async {
        let quiet = "GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND='ssh -o BatchMode=yes -o ConnectTimeout=10'"
        _ = await runner.run("cd \(shellPath(repo)) && git rev-parse -q --verify '@{u}' >/dev/null"
            + " && \(quiet) git fetch -q && git merge -q --ff-only '@{u}'")
    }

    /// Starts git's file monitor in `folder` when the user's config wants one. Started by a
    /// prompt's `git status` instead, it stays in the pane's process group, and herdr then takes
    /// the shell for busy and will not start an Agent there.
    static func watch(_ folder: String, on runner: Runner) async {
        let git = "git -C \(shellPath(folder))"
        _ = await runner.run("[ \"$(\(git) config --type=bool core.fsmonitor)\" != true ] || \(git) fsmonitor--daemon start")
    }

    /// Removes the linked worktree at `path` of `repo`, keeping its branch.
    static func removeWorktree(_ path: String, of repo: String, force: Bool, on runner: Runner) async -> Ran {
        await runner.run("git -C \(shellPath(repo)) worktree remove \(quote(path))\(force ? " --force" : "")")
    }
}
