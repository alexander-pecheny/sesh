import Foundation

/// Commands the Mac runs on itself, through its login shell's environment.
@MainActor
final class Local {
    private var running: Set<Process> = []
    private var tethers: [ObjectIdentifier: Pipe] = [:]

    /// An interactive shell may greet first; everything before this line is its greeting.
    nonisolated static let mark = "--sesh-output--"

    /// The login shell's environment as `export` lines, read once, so PATH has what the user
    /// installed without a login shell for every command.
    private var environment: Task<String?, Never>?

    func stop() { running.forEach { $0.terminate() } }

    private func loginExports() async -> String? {
        if environment == nil {
            environment = Task {
                let ran = await launch("env -0", line: nil, input: nil, opened: nil, exports: nil)
                if !ran.ok { environment = nil }
                return ran.ok ? exports(fromEnv: ran.out) : nil
            }
        }
        return await environment?.value
    }

    /// A stream runs until the app's end of its stdin closes, which happens however the app
    /// ends, so no command outlives the app that reads it.
    private static func tethered(_ command: String) -> String {
        "{ \(command)\n} & child=$!; { cat >/dev/null; kill $child 2>/dev/null; } >/dev/null 2>&1 & wait $child"
    }

    func run(_ command: String, line: ((String) -> Void)?, input: Data?,
             opened: ((@escaping (String) -> Void) -> Void)?) async -> Ran {
        let exports = await loginExports()
        // The tether reads stdin itself, so a stream that is written to goes without one.
        let command = line == nil || opened != nil ? command : Self.tethered(command)
        return await launch(command, line: line, input: input, opened: opened, exports: exports)
    }

    private func launch(_ command: String, line: ((String) -> Void)?, input: Data?,
                        opened: ((@escaping (String) -> Void) -> Void)?, exports: String?) async -> Ran {
        let process = Process()
        let marked = "echo \(Self.mark); " + command
        process.executableURL = URL(filePath: "/bin/zsh")
        process.arguments = exports.map { ["-c", $0 + marked] } ?? ["-lic", marked]
        process.standardInput = FileHandle.nullDevice
        let out = Pipe(), err = Pipe()
        process.standardOutput = out
        process.standardError = err
        // Data to write, or, for a stream, an input the app never writes and only closes.
        let feed = input != nil || line != nil ? Pipe() : nil
        if let feed { process.standardInput = feed }
        let lines = Greeted(line)
        let collected = Collector()
        // Output is read only by its handlers, to its end, and taken once the process has also
        // exited: a second reader at exit raced them and could put a chunk out of order.
        let finished = DispatchGroup()
        func drain(_ pipe: Pipe, into add: @escaping (Data) -> Void) {
            finished.enter()
            pipe.fileHandleForReading.readabilityHandler = { handle in
                let chunk = handle.availableData
                guard chunk.isEmpty else { return add(chunk) }
                handle.readabilityHandler = nil
                finished.leave()
            }
        }
        // The main queue, unlike a Task, keeps the chunks in order.
        drain(out) { chunk in
            if line == nil { collected.out.append(chunk) } else { DispatchQueue.main.async { MainActor.assumeIsolated { lines.feed(chunk) } } }
        }
        drain(err) { collected.err.append($0) }
        finished.enter()
        return await withTaskCancellationHandler {
            await withCheckedContinuation { (done: CheckedContinuation<Ran, Never>) in
                process.terminationHandler = { _ in finished.leave() }
                finished.notify(queue: .main) { MainActor.assumeIsolated {
                    self.running.remove(process)
                    try? self.tethers.removeValue(forKey: ObjectIdentifier(process))?.fileHandleForWriting.close()
                    lines.flush()
                    let out = String(decoding: collected.out.data, as: UTF8.self)
                    let marked = out.range(of: Self.mark + "\n")
                    done.resume(returning: Ran(
                        status: process.terminationStatus,
                        out: marked.map { String(out[$0.upperBound...]) } ?? out,
                        err: String(decoding: collected.err.data, as: UTF8.self)))
                } }
                do {
                    try process.run()
                    running.insert(process)
                    if Task.isCancelled { process.terminate() }
                    if let feed, let input {
                        // The throwing write: the old one raises an Objective-C exception when the
                        // command has already gone, and one escaping a Task breaks Swift's executor
                        // bookkeeping until the next click crashes.
                        try? feed.fileHandleForWriting.write(contentsOf: input)
                        try? feed.fileHandleForWriting.close()
                    }
                    if let feed, input == nil { tethers[ObjectIdentifier(process)] = feed }
                    if let feed, let opened {
                        // A write after the command has gone fails rather than killing the app.
                        _ = fcntl(feed.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1)
                        opened { text in try? feed.fileHandleForWriting.write(contentsOf: Data((text + "\n").utf8)) }
                    }
                } catch {
                    done.resume(returning: Ran(status: -1, out: "", err: error.localizedDescription))
                }
            }
        } onCancel: {
            // Terminating a process not yet launched raises; one cancelled first ends after launch.
            if process.isRunning { process.terminate() }
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

/// A stream's whole lines, those of the shell's greeting left out.
@MainActor
private final class Greeted {
    private let line: ((String) -> Void)?
    private var lines = LineSplitter()
    private(set) var greeted = false

    init(_ line: ((String) -> Void)?) { self.line = line }

    func feed(_ chunk: Data) {
        guard let line else { return }
        for text in lines.feed(chunk) {
            if greeted { line(text) } else { greeted = text == Local.mark }
        }
    }

    func flush() {
        guard let line, greeted, let rest = lines.flush() else { return }
        line(rest)
    }
}
