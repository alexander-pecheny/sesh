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

    /// Removes the linked worktree at `path` of `repo`, keeping its branch.
    static func removeWorktree(_ path: String, of repo: String, force: Bool, on runner: Runner) async -> Ran {
        await runner.run("git -C \(quote(repo)) worktree remove \(quote(path))\(force ? " --force" : "")")
    }
}
