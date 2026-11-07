import Foundation

/// Where an ended Agent session, or a Transcript found by search, starts again.
enum Resumption: Hashable {
    case here
    /// A copy under a new session id on the same machine, leaving the original as it was.
    case fork
    case on(Machine)
}

/// A Transcript put where its Agent looks for it, as the helper reports it.
struct Placed: Decodable {
    let session: String
    let path: String
    let cwd: String
}

extension TaskActions {
    /// The Claude config folder a Transcript lives in, which a resumed Claude must be started
    /// with: `~/.claude-personal` for `~/.claude-personal/projects/<project>/<id>.jsonl`.
    static func claudeConfig(of transcript: String) -> String? { Move.account(of: transcript) }

    /// Puts the Transcript at `path` on `source` where its Agent looks for it on `target`, in the
    /// same folder there or the same place under its home. A fork takes a new session id.
    static func place(_ path: String, agent: Agent, from source: Machine, on target: Machine,
                      fork: Bool, config: String?) async -> Result<Placed, Mux.Failure> {
        if let problem = await target.prepare() { return .failure(Mux.Failure(problem)) }
        var file = shellPath(path)
        var inbox: String?
        if source != target {
            let bytes: Data
            switch await source.get(path) {
            case .success(let data): bytes = data
            case .failure(let failure): return .failure(failure)
            }
            let folder = ".sesh/inbox/\(UUID().uuidString.lowercased())"
            let staged = "\(folder)/\((path as NSString).lastPathComponent)"
            let put = await target.put(bytes, to: staged)
            guard put.ok else { return .failure(Mux.Failure(put.problem)) }
            (file, inbox) = (shellPath("~/" + staged), shellPath("~/" + folder))
        }
        let options = (agent == .claude ? config.map { " --config \(shellPath($0))" } ?? "" : "") + (fork ? " --fork" : "")
        let placed = await target.run("\(Helper.path) transcripts place \(file) --agent \(agent.rawValue)\(options)")
        if let inbox { _ = await target.run("rm -rf \(inbox)") }
        guard placed.ok, let result = try? JSONDecoder().decode(Placed.self, from: Data(placed.out.utf8)) else {
            return .failure(Mux.Failure(placed.problem))
        }
        return .success(result)
    }
}
