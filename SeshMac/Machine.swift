import Foundation

/// A machine the Mac runs commands on: itself through a login shell, or a Host through the
/// owner's own ssh and `~/.ssh/config`, one shared connection per Host.
@MainActor
final class Machine: Runner, Identifiable, Hashable {
    /// The ssh alias, or nil for the Mac itself.
    let alias: String?
    private var running: Set<Process> = []

    nonisolated static func == (a: Machine, b: Machine) -> Bool { a.alias == b.alias }
    nonisolated func hash(into hasher: inout Hasher) { hasher.combine(alias) }

    private init(alias: String?) { self.alias = alias }

    nonisolated var id: String { alias ?? "" }
    var title: String { alias ?? "This Mac" }

    static let mac = Machine(alias: nil)
    private static var known: [String: Machine] = [:]

    /// One object per Host for the app's life: Conversations hold their machine weakly.
    static func named(_ alias: String?) -> Machine {
        guard let alias else { return mac }
        if let machine = known[alias] { return machine }
        let machine = Machine(alias: alias)
        known[alias] = machine
        return machine
    }
    /// An interactive shell may greet first; everything before this line is its greeting.
    nonisolated static let mark = "--sesh-output--"

    private func process(_ command: String) -> Process {
        let process = Process()
        if let alias {
            let control = FileManager.default.homeDirectoryForCurrentUser.appending(path: ".sesh/ssh").path
            try? FileManager.default.createDirectory(atPath: control, withIntermediateDirectories: true)
            process.executableURL = URL(filePath: "/usr/bin/ssh")
            process.arguments = [
                "-o", "BatchMode=yes", "-o", "ControlMaster=auto", "-o", "ControlPath=\(control)/%C",
                "-o", "ControlPersist=600", "-o", "ServerAliveInterval=15", alias,
                // A login shell, as the phone's Link uses, so PATH has what the user installed.
                "exec \"$SHELL\" -lic \(quote("echo \(Self.mark); " + command))",
            ]
        } else {
            process.executableURL = URL(filePath: "/bin/zsh")
            process.arguments = ["-lic", "echo \(Self.mark); " + command]
        }
        process.standardInput = FileHandle.nullDevice
        return process
    }

    func run(_ command: String) async -> Ran { await stream(command, line: nil) }

    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran {
        await stream(command, line: Optional(line))
    }

    /// `path` quoted for the shell, with a leading `~/` still meaning the home folder.
    nonisolated static func shellPath(_ path: String) -> String {
        path.hasPrefix("~/") ? "\"$HOME\"/" + quote(String(path.dropFirst(2))) : quote(path)
    }

    /// Writes `data` to `path` on the machine, relative to its home folder unless absolute.
    func put(_ data: Data, to path: String) async -> Ran {
        let target = Self.shellPath(path)
        return await stream("mkdir -p \"$(dirname \(target))\" && cat > \(target)", line: nil, input: data)
    }

    private func stream(_ command: String, line: ((String) -> Void)?, input: Data? = nil) async -> Ran {
        let process = process(command)
        let out = Pipe(), err = Pipe()
        process.standardOutput = out
        process.standardError = err
        let feed = input.map { _ in Pipe() }
        if let feed { process.standardInput = feed }
        let lines = LineSplitter(line)
        let collected = Collector()
        out.fileHandleForReading.readabilityHandler = { handle in
            let chunk = handle.availableData
            guard !chunk.isEmpty else { return }
            // The main queue, unlike a Task, keeps the chunks in order.
            if line == nil { collected.out.append(chunk) } else { DispatchQueue.main.async { MainActor.assumeIsolated { lines.feed(chunk) } } }
        }
        err.fileHandleForReading.readabilityHandler = { handle in collected.err.append(handle.availableData) }
        return await withTaskCancellationHandler {
            await withCheckedContinuation { (done: CheckedContinuation<Ran, Never>) in
                process.terminationHandler = { process in
                    out.fileHandleForReading.readabilityHandler = nil
                    err.fileHandleForReading.readabilityHandler = nil
                    let rest = out.fileHandleForReading.readDataToEndOfFile()
                    let errRest = err.fileHandleForReading.readDataToEndOfFile()
                    DispatchQueue.main.async { MainActor.assumeIsolated {
                        self.running.remove(process)
                        if line == nil { collected.out.append(rest) } else { lines.feed(rest); lines.flush() }
                        collected.err.append(errRest)
                        let out = String(decoding: collected.out.data, as: UTF8.self)
                        done.resume(returning: Ran(
                            status: process.terminationStatus,
                            out: out.range(of: Self.mark + "\n").map { String(out[$0.upperBound...]) } ?? out,
                            err: String(decoding: collected.err.data, as: UTF8.self)))
                    } }
                }
                do {
                    try process.run()
                    running.insert(process)
                    if let feed, let input {
                        feed.fileHandleForWriting.write(input)
                        try? feed.fileHandleForWriting.close()
                    }
                } catch {
                    done.resume(returning: Ran(status: -1, out: "", err: error.localizedDescription))
                }
            }
        } onCancel: {
            process.terminate()
        }
    }
}

/// Bytes from a reading thread, gathered under a lock.
private final class Collector: @unchecked Sendable {
    final class Buffer: @unchecked Sendable {
        private let lock = NSLock()
        private var bytes = Data()
        func append(_ chunk: Data) { lock.withLock { bytes.append(chunk) } }
        var data: Data { lock.withLock { bytes } }
    }
    let out = Buffer()
    let err = Buffer()
}

/// Turns chunks into whole lines, on the main actor.
@MainActor
private final class LineSplitter {
    private let line: ((String) -> Void)?
    private var buffer = Data()
    private var greeted = false

    init(_ line: ((String) -> Void)?) { self.line = line }

    func feed(_ chunk: Data) {
        guard let line else { return }
        buffer.append(chunk)
        while let end = buffer.firstIndex(of: UInt8(ascii: "\n")) {
            let text = String(decoding: buffer[buffer.startIndex..<end], as: UTF8.self)
            buffer.removeSubrange(buffer.startIndex...end)
            if greeted { line(text) } else { greeted = text == Machine.mark }
        }
    }

    func flush() {
        guard let line, greeted, !buffer.isEmpty else { return }
        line(String(decoding: buffer, as: UTF8.self))
        buffer.removeAll()
    }
}

extension Machine {
    /// Puts the pinned transcript helper in place unless that version is already there.
    func prepare() async -> String? {
        let ran = await run("uname -sm; \(Helper.path) --version 2>/dev/null || echo")
        let lines = ran.out.components(separatedBy: "\n")
        guard ran.ok, lines.count > 1 else { return ran.problem }
        if let local = Self.localHelpers { return await install(from: local, platform: lines[0], found: lines[1]) }
        guard let published = Helper.published, lines[1] != published.version else { return nil }
        guard let download = Helper.download(Helper.name(lines[0])) else {
            return "Sesh has no helper for \(lines[0]) on \(title)."
        }
        let installed = await run(download)
        return installed.ok ? nil : "Sesh could not install its helper on \(title): \(installed.problem)"
    }

    /// `-helpers DIR` points at `build/helpers`, so a helper not yet released can be tried.
    private static let localHelpers = UserDefaults.standard.string(forKey: "helpers").map { URL(filePath: $0) }

    private func install(from folder: URL, platform: String, found: String) async -> String? {
        let version = (try? String(contentsOf: folder.appending(path: "version"), encoding: .utf8))?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard found != version else { return nil }
        guard let data = try? Data(contentsOf: folder.appending(path: "sesh-transcript-\(Helper.name(platform)).gz")) else {
            return "There is no local helper for \(platform)."
        }
        let put = await put(data, to: "~/.sesh/bin/sesh-transcript.gz")
        guard put.ok else { return put.problem }
        let unpacked = await run(Helper.unpack)
        return unpacked.ok ? nil : unpacked.problem
    }
}
