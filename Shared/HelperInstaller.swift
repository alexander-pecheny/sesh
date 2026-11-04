import CryptoKit
import Foundation

/// Puts the pinned transcript helper, mosh-server and sesh-mux on a machine (ADR 0006, 0009)
/// unless those versions are there, and starts Sesh's sesh-mux server there if it is down; once a
/// run, however the machine is reached.
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

    static let probe = "uname -sm\n\(Helper.path) --version 2>/dev/null || echo"
        + "\ntest -x ~/.sesh/bin/\(Helper.moshFile ?? "-") && echo mosh || echo"
        + "\n\(Mux.command) --version 2>/dev/null || echo"
        + "\ncat ~/.sesh/bin/sesh-mux.version 2>/dev/null || echo"

    /// Whether this build's sesh-mux should replace the one whose `--version` printed `found`.
    /// It never replaces a newer build another device put there (ADR 0023); a pin from before
    /// builds were stamped goes by the version the last install noted.
    static func replacesMux(found: String, noted: String, build: UInt64? = Helper.muxBuild,
                            version: String? = Helper.muxVersion) -> Bool {
        guard let version else { return false }
        guard let build else { return noted != version }
        return found.isEmpty || (found.components(separatedBy: " build ").last.flatMap { UInt64($0) } ?? 0) < build
    }

    static func plan(_ probed: Ran, on title: String, version: String? = Helper.version, local: Bool = Helper.local != nil,
                     published: Helper.Published? = Helper.published) -> Plan {
        let lines = probed.out.components(separatedBy: "\n")
        guard probed.ok, lines.count > 1 else { return .failed(probed.problem) }
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
        let probed = await run(Self.probe)
        let plan = Self.plan(probed, on: title)
        if let problem = await install(plan) { return problem }
        if plan != .ready { _ = await run(Helper.link) }
        let lines = probed.out.components(separatedBy: "\n")
        guard lines.count > 4 else { return nil }
        let name = Helper.name(lines[0])
        if let file = Helper.moshFile, lines[2] != "mosh",
           let problem = await install("mosh-server", as: file, for: name, from: Helper.mosh) { return problem }
        let replaces = Self.replacesMux(found: lines[3], noted: lines[4])
        if replaces, let version = Helper.muxVersion {
            if let problem = await install("sesh-mux", as: "sesh-mux", for: name, from: Helper.mux) { return problem }
            let noted = await run(Mux.importer + "\nprintf '%s\\n' \(quote(version)) > ~/.sesh/bin/sesh-mux.version")
            guard noted.ok else { return "Sesh could not install sesh-mux on \(title): \(noted.problem)" }
        }
        let woke = await run(Mux.wake(upgrade: replaces && !lines[3].isEmpty) + "\n" + Mux.integrations)
        return woke.ok ? nil : "Sesh could not start sesh-mux on \(title): \(woke.problem)"
    }

    /// Puts `program`'s build for platform `name` in `~/.sesh/bin` as `file`: the local one under
    /// `-helpers DIR`, or else the one `published` pins, fetched by the machine or through this device.
    private func install(_ program: String, as file: String, for name: String, from published: Helper.Published?) async -> String? {
        var ran: Ran
        if let local = Helper.local {
            guard let data = try? Data(contentsOf: local.appending(path: "\(program)-\(name).gz")) else { return "There is no local \(program) for \(name)." }
            ran = await place(data, as: file)
        } else if let published {
            ran = await run(Helper.download(program, as: file, from: published, for: name) ?? "false")
            if !ran.ok, let data = await Self.fetch(program, for: name, from: published) { ran = await place(data, as: file) }
        } else {
            return nil
        }
        return ran.ok ? nil : "Sesh could not install \(program) on \(title): \(ran.problem)"
    }

    private func install(_ plan: Plan) async -> String? {
        switch plan {
        case .ready: return nil
        case .failed(let problem): return problem
        case .local(let platform):
            guard let data = Helper.local.flatMap({ try? Data(contentsOf: $0.appending(path: "sesh-transcript-\(Helper.name(platform)).gz")) })
            else { return "There is no local helper for \(platform)." }
            let placed = await place(data, as: Helper.file)
            return placed.ok ? nil : placed.problem
        case .download(let name):
            var ran = await run(Helper.download(name) ?? "false")
            if !ran.ok, let published = Helper.published, let data = await Self.fetch("sesh-transcript", for: name, from: published) {
                ran = await place(data, as: Helper.file)
            }
            return ran.ok ? nil : "Sesh could not install its helper on \(title): \(ran.problem)"
        }
    }

    private func place(_ data: Data, as file: String) async -> Ran {
        let sent = await put(data, "~/.sesh/bin/\(file).gz")
        return sent.ok ? await run(Helper.unpack(file)) : sent
    }

    /// The published build through this device, for a machine that cannot fetch it itself; only
    /// the exact bytes this build pinned.
    private static func fetch(_ program: String, for name: String, from published: Helper.Published) async -> Data? {
        guard let sha = published.sha256[name], let source = URL(string: published.url + "\(program)-\(name).gz"),
              let (data, _) = try? await URLSession.shared.data(from: source)
        else { return nil }
        return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined() == sha ? data : nil
    }
}
