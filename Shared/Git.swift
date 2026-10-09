import Foundation

/// What Sesh asks of git on any machine, through whichever Runner reaches it.
@MainActor
enum Git {
    /// Whether `path` is a linked worktree, whose `.git` is a file rather than a repository's own.
    static func linked(_ path: String, on runner: Runner) async -> Bool {
        await runner.run("[ -f \(quote(path))/.git ]").ok
    }

    /// What `git status` says is uncommitted at `path`; empty when clean or unreadable.
    static func uncommitted(_ path: String, on runner: Runner) async -> String {
        let ran = await runner.run("git -C \(quote(path)) status --porcelain")
        return ran.ok ? ran.out.trimmingCharacters(in: .whitespacesAndNewlines) : ""
    }

    /// The local branches of `repo`; none when it is not a repository yet.
    static func branches(of repo: String, on runner: Runner) async -> Set<String> {
        let ran = await runner.run("git -C \(shellPath(repo)) for-each-ref --format='%(refname:short)' refs/heads")
        return ran.ok ? Set(ran.out.split(separator: "\n").map(String.init)) : []
    }

    /// A new linked worktree of `repo` at `path` on a new `branch`, from what `repo` has checked out.
    static func addWorktree(_ path: String, branch: String, of repo: String, on runner: Runner) async -> Ran {
        await runner.run("git -C \(quote(repo)) worktree add -b \(quote(branch)) \(quote(path))")
    }

    /// Removes the linked worktree at `path` of `repo`, keeping its branch.
    static func removeWorktree(_ path: String, of repo: String, force: Bool, on runner: Runner) async -> Ran {
        await runner.run("git -C \(quote(repo)) worktree remove \(quote(path))\(force ? " --force" : "")")
    }
}
