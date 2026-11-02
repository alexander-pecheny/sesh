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
    /// Bytes read from a machine per command while a Transcript moves to another.
    private static let chunk = 4 << 20

    /// The Claude config folder a Transcript lives in, which a resumed Claude must be started
    /// with: `~/.claude-personal` for `~/.claude-personal/projects/<project>/<id>.jsonl`.
    static func claudeConfig(of transcript: String) -> String? {
        transcript.range(of: "/projects/", options: .backwards).map { String(transcript[..<$0.lowerBound]) }
    }

    /// Puts the Transcript at `path` on `source` where its Agent looks for it on `target`, in the
    /// same folder there or the same place under its home. A fork takes a new session id.
    static func place(_ path: String, agent: Agent, from source: Machine, on target: Machine,
                      fork: Bool, config: String?) async -> Result<Placed, Mux.Failure> {
        if let problem = await target.prepare() { return .failure(Mux.Failure(problem)) }
        var file = shellPath(path)
        var inbox: String?
        if source != target {
            var bytes = Data()
            while true {
                let read = await source.run("tail -c +\(bytes.count + 1) \(shellPath(path)) | head -c \(chunk) | base64 | tr -d '\\n'")
                guard read.ok, let piece = Data(base64Encoded: read.out) else { return .failure(Mux.Failure(read.problem)) }
                if piece.isEmpty { break }
                bytes += piece
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
