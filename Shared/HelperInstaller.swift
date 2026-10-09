import CryptoKit
import Foundation

/// Puts the pinned transcript helper on a machine (ADR 0006) unless that version is there, and
/// starts herdr's server there if it is down; once a run, however the machine is reached.
@MainActor
final class HelperInstaller {
    /// What a machine's answer to `probe` calls for.
    enum Plan: Equatable {
        case ready
        case failed(String)
        /// The helper built for this platform, from the folder `-helpers DIR` names.
        case local(platform: String)
        /// The published helper of this name, fetched by the machine or else through this device.
        case download(String)
    }

    static let probe = Herdr.wake + "\nuname -sm\n\(Helper.path) --version 2>/dev/null || echo"

    static func plan(_ probed: Ran, on title: String, version: String? = Helper.version, local: Bool = Helper.local != nil,
                     published: Helper.Published? = Helper.published) -> Plan {
        let lines = probed.out.components(separatedBy: "\n")
        guard probed.ok, lines.count > 1 else { return .failed(probed.problem) }
        guard lines[0] != "missing" else { return .failed("\(title) has no herdr, which Tasks run their Agents in.") }
        let (platform, found) = (lines[0], lines[1])
        if found == version { return .ready }
        if local { return .local(platform: platform) }
        guard published?.sha256[Helper.name(platform)] != nil else { return .failed("Sesh has no helper for \(platform) on \(title).") }
        return .download(Helper.name(platform))
    }

    private let title: String
    private let run: (String) async -> Ran
    private let put: (Data, String) async -> Ran
    private var preparing: Task<String?, Never>?

    /// `put` writes bytes to a path on the machine, which is all that differs between machines.
    init(title: String, run: @escaping (String) async -> Ran, put: @escaping (Data, String) async -> Ran) {
        self.title = title
        self.run = run
        self.put = put
    }

    /// Why the machine is not ready, or nil once it is; a failure is tried again next time.
    func prepare() async -> String? {
        if let preparing { return await preparing.value }
        let task = Task { await install() }
        preparing = task
        let problem = await task.value
        if problem != nil { preparing = nil }
        return problem
    }

    private func install() async -> String? {
        switch Self.plan(await run(Self.probe), on: title) {
        case .ready: return nil
        case .failed(let problem): return problem
        case .local(let platform):
            guard let data = Helper.local.flatMap({ try? Data(contentsOf: $0.appending(path: "sesh-transcript-\(Helper.name(platform)).gz")) })
            else { return "There is no local helper for \(platform)." }
            let placed = await place(data)
            return placed.ok ? nil : placed.problem
        case .download(let name):
            var ran = await run(Helper.download(name) ?? "false")
            if !ran.ok, let data = await Self.fetch(name) { ran = await place(data) }
            return ran.ok ? nil : "Sesh could not install its helper on \(title): \(ran.problem)"
        }
    }

    private func place(_ data: Data) async -> Ran {
        let sent = await put(data, "~/.sesh/bin/\(Helper.file).gz")
        return sent.ok ? await run(Helper.unpack) : sent
    }

    /// The published build through this device, for a machine that cannot fetch it itself; only
    /// the exact bytes this build pinned.
    private static func fetch(_ name: String) async -> Data? {
        guard let published = Helper.published, let sha = published.sha256[name],
              let source = URL(string: published.url + "sesh-transcript-\(name).gz"),
              let (data, _) = try? await URLSession.shared.data(from: source)
        else { return nil }
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() == sha ? data : nil
    }
}
