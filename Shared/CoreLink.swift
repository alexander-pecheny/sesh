import Foundation

struct HostKeyQuestion: Identifiable {
    let id = UUID()
    let fingerprint: String
    let previous: String?
}

struct AuthQuestion: Identifiable {
    let id: UInt32
    let title: String
    let instruction: String
    let prompts: [(text: String, echo: Bool)]
}

/// One connection to a Host through the core, shared by everything run there: Vaults, herdr,
/// the helper, Conversations and Terminals. Core callbacks arrive on tokio threads and hop to
/// main in `LinkBridge`.
@MainActor
class CoreLink: ObservableObject, Identifiable {
    enum Stage: Equatable {
        case connecting, authenticating, ready, failed
    }

    @Published private(set) var stage = Stage.connecting
    @Published private(set) var message = ""
    @Published var hostKeyQuestion: HostKeyQuestion? { didSet { Asking.shared.update() } }
    @Published var authQuestion: AuthQuestion? { didSet { Asking.shared.update() } }

    let id = UUID()
    let title: String
    private(set) var handle: OpaquePointer?
    private let dial: (sesh_callbacks_t, UnsafeMutableRawPointer) -> OpaquePointer?
    private var bridge = LinkBridge()
    private var waiting: [UInt32: CheckedContinuation<Ran, Never>] = [:]
    private var streams: [UInt32: (lines: LineSplitter, line: (String) -> Void)] = [:]
    private var linking: [CheckedContinuation<Bool, Never>] = []
    private(set) var linked = false
    private var paused = false

    /// `dial` opens the connection with the callbacks and userdata it is handed.
    init(title: String, dial: @escaping (sesh_callbacks_t, UnsafeMutableRawPointer) -> OpaquePointer?) {
        self.title = title
        self.dial = dial
    }

    /// Also called on the way to the background or to sleep: a frozen socket would make a
    /// command sent after waking wait minutes for TCP to admit it is dead.
    func close() {
        paused = true
        drop("the connection was closed")
    }

    /// Losing the connection costs nothing visible: the next command dials again before it runs.
    func resume() {
        paused = false
    }

    private func drop(_ problem: String) {
        if let handle {
            sesh_session_close(handle)
            sesh_session_free(handle)
        }
        handle = nil
        linked = false
        finishAll(problem)
        let pending = linking
        linking = []
        pending.forEach { $0.resume(returning: false) }
    }

    private func connect() {
        stage = .connecting
        message = ""
        bridge.owner = nil
        bridge = LinkBridge()
        bridge.owner = self
        handle = dial(LinkBridge.callbacks, Unmanaged.passRetained(bridge).toOpaque())
    }

    // MARK: Commands on the Host

    func run(_ command: String) async -> Ran { await call { sesh_session_run($0, command) } }

    /// Runs `command` until it exits or the calling task is cancelled, handing each line of
    /// its stdout to `line` as it arrives.
    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran {
        await call(line: line) { sesh_session_stream($0, command) }
    }

    /// Writes `data` to `path` on the Host over SFTP. Sesh's own files land in place; any other
    /// file is staged and copied over the old one, which keeps its mode.
    func put(_ data: Data, to path: String) async -> Ran {
        let file = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
        do { try data.write(to: file) } catch { return Ran(status: -1, out: "", err: error.localizedDescription) }
        defer { try? FileManager.default.removeItem(at: file) }
        if path.hasPrefix("~/.sesh/") {
            return await call { sesh_session_put($0, file.path, String(path.dropFirst(2))) }
        }
        let staged = ".sesh/inbox/\(UUID().uuidString.lowercased())"
        let sent = await call { sesh_session_put($0, file.path, staged) }
        guard sent.ok else { return sent }
        let target = shellPath(path)
        return await run("mkdir -p \"$(dirname \(target))\" && cat ~/\(staged) > \(target); s=$?; rm -f ~/\(staged); exit $s")
    }

    /// As `stream`, but over a mosh pipe that `server` starts on the Host: once running it needs
    /// nothing of the Link, so it roams, outlives the Link's drop and picks up on waking. Behind
    /// a proxy the core runs it on the connection instead. `opened` is handed a way to write
    /// lines to the command's stdin.
    func pipe(_ command: String, server: String, opened: ((@escaping (String) -> Void) -> Void)?,
              line: @escaping (String) -> Void) async -> Ran {
        if let problem = await up() { return Ran(status: -1, out: "", err: problem) }
        guard let handle, !Task.isCancelled else { return Ran(status: -1, out: "", err: "not connected") }
        let bridge = PipeBridge(line: line)
        let ran = await withTaskCancellationHandler {
            await withCheckedContinuation { ended in
                bridge.ended = ended
                bridge.pipe = sesh_session_pipe(handle, server, command, PipeBridge.callbacks, Unmanaged.passRetained(bridge).toOpaque())
                opened?(bridge.write)
            }
        } onCancel: {
            Task { @MainActor in bridge.close() }
        }
        if ran.status == -1, ran.err.hasPrefix(Self.gone) { drop(ran.err) }
        return ran
    }

    /// A terminal running `command` on the connection, or the reason there is none.
    func terminal(_ command: String, server: String?, cols: UInt16, rows: UInt16, events: TerminalEvents) async -> Result<CoreTerminal, Failure> {
        if let problem = await up() { return .failure(Failure(message: problem)) }
        guard let handle else { return .failure(Failure(message: "not connected")) }
        return .success(CoreTerminal(link: handle, command: command, server: server, cols: cols, rows: rows, events: events))
    }

    struct Failure: Error { let message: String }

    /// What the core says when the connection died under a command.
    nonisolated static let gone = "the connection to the Host is gone"

    /// Why the Link is not up, after dialling it if need be; nil once it is.
    private func up() async -> String? {
        guard !paused else { return "not connected" }
        if !linked {
            if handle == nil { connect() }
            guard await withCheckedContinuation({ linking.append($0) }) else { return message.isEmpty ? "not connected" : message }
        }
        return nil
    }

    /// A command the dead connection kept from running is run once more on a fresh one.
    private func call(line: ((String) -> Void)? = nil, start: (OpaquePointer) -> UInt32) async -> Ran {
        let ran = await attempt(line: line, start: start)
        guard line == nil, ran.status == -1, ran.err.hasPrefix(Self.gone), !Task.isCancelled else { return ran }
        return await attempt(line: line, start: start)
    }

    private func attempt(line: ((String) -> Void)?, start: (OpaquePointer) -> UInt32) async -> Ran {
        if let problem = await up() { return Ran(status: -1, out: "", err: problem) }
        guard let handle, !Task.isCancelled else { return Ran(status: -1, out: "", err: "not connected") }
        let id = start(handle)
        guard id != 0 else { return Ran(status: -1, out: "", err: "not connected") }
        if let line { streams[id] = (LineSplitter(), line) }
        return await withTaskCancellationHandler {
            await withCheckedContinuation { waiting[id] = $0 }
        } onCancel: {
            Task { @MainActor [weak self] in
                guard let self, let handle = self.handle else { return }
                sesh_session_cancel(handle, id)
            }
        }
    }

    // MARK: For subclasses

    /// The saved password was refused.
    func passwordRejected() {}
    func uploadProgressed(_ id: UInt32, _ done: UInt64, _ total: UInt64) {}
    func uploadDone(_ id: UInt32, _ paths: [String], _ error: String?) {}

    // MARK: From the core

    private func fail(_ problem: String) {
        stage = .failed
        message = problem
        drop(problem)
    }

    private func finishAll(_ problem: String) {
        let pending = waiting
        waiting = [:]
        streams = [:]
        pending.values.forEach { $0.resume(returning: Ran(status: -1, out: "", err: problem)) }
    }

    fileprivate func ran(_ id: UInt32, _ result: Ran) {
        if var stream = streams.removeValue(forKey: id), let rest = stream.lines.flush() { stream.line(rest) }
        waiting.removeValue(forKey: id)?.resume(returning: result)
        if result.status == -1, result.err.hasPrefix(Self.gone) { drop(result.err) }
    }

    fileprivate func chunk(_ id: UInt32, _ data: Data) {
        guard var stream = streams[id] else { return }
        let lines = stream.lines.feed(data)
        streams[id] = stream
        lines.forEach(stream.line)
    }

    fileprivate func apply(_ state: UInt32, _ message: String) {
        switch state {
        case SESH_STATE_PASSWORD_REJECTED.rawValue: passwordRejected()
        case SESH_STATE_CONNECTING.rawValue: stage = .connecting
        case SESH_STATE_AUTHENTICATING.rawValue: stage = .authenticating
        case SESH_STATE_CONNECTED.rawValue:
            stage = .ready
            linked = true
            let pending = linking
            linking = []
            pending.forEach { $0.resume(returning: true) }
        default: fail(message.isEmpty ? "disconnected" : message)
        }
    }

    func answerHostKey(_ accept: Bool) {
        hostKeyQuestion = nil
        guard let handle else { return }
        sesh_session_answer_host_key(handle, accept)
    }

    func answerPrompt(_ question: AuthQuestion, _ answers: [String]?) {
        authQuestion = nil
        guard let handle else { return }
        guard let answers else { return sesh_session_answer_prompt(handle, question.id, nil, 0) }
        let strings = CStrings()
        let pointers = answers.map { strings.make($0) }
        pointers.withUnsafeBufferPointer {
            sesh_session_answer_prompt(handle, question.id, $0.baseAddress, UInt($0.count))
        }
    }

    deinit {
        guard let handle else { return }
        sesh_session_close(handle)
        sesh_session_free(handle)
    }
}

/// The Link that has a question for the user, if any, so one place on screen can ask it.
@MainActor
final class Asking: ObservableObject {
    static let shared = Asking()
    @Published private(set) var link: CoreLink?
    var links: () -> [CoreLink] = { [] }

    func update() {
        link = links().first { $0.hostKeyQuestion != nil || $0.authQuestion != nil }
    }
}

/// C strings that live as long as this object, for a call that borrows them.
final class CStrings {
    private var allocated: [UnsafeMutablePointer<CChar>] = []

    func make(_ value: String?) -> UnsafePointer<CChar>? {
        guard let value, let copy = strdup(value) else { return nil }
        allocated.append(copy)
        return UnsafePointer(copy)
    }

    deinit { allocated.forEach { free($0) } }
}

private final class LinkBridge {
    weak var owner: CoreLink?

    static let callbacks = sesh_callbacks_t(
        on_output: nil,
        on_state: { userdata, state, message in
            guard let userdata else { return }
            let text = message.map { String(cString: $0) } ?? ""
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.apply(state.rawValue, text) }
        },
        on_host_key: { userdata, fingerprint, previous in
            guard let userdata, let fingerprint else { return }
            let question = HostKeyQuestion(fingerprint: String(cString: fingerprint), previous: previous.map { String(cString: $0) })
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.hostKeyQuestion = question }
        },
        on_auth_prompt: { userdata, id, name, instruction, prompts, echoes, count in
            guard let userdata, let prompts, let echoes else { return }
            let question = AuthQuestion(
                id: id,
                title: name.map { String(cString: $0) } ?? "Authentication",
                instruction: instruction.map { String(cString: $0) } ?? "",
                prompts: (0..<Int(count)).map { (prompts[$0].map { String(cString: $0) } ?? "", echoes[$0]) })
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.authQuestion = question }
        },
        on_upload_progress: { userdata, id, done, total in
            guard let userdata else { return }
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.uploadProgressed(id, done, total) }
        },
        on_upload_done: { userdata, id, paths, count, error in
            guard let userdata else { return }
            let remote = (0..<Int(count)).compactMap { paths?[$0].map { String(cString: $0) } }
            let message = error.map { String(cString: $0) }
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.uploadDone(id, remote, message) }
        },
        on_ran: { userdata, id, status, out, err in
            guard let userdata else { return }
            let result = Ran(status: status, out: out.map { String(cString: $0) } ?? "", err: err.map { String(cString: $0) } ?? "")
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.ran(id, result) }
        },
        on_chunk: { userdata, id, bytes, len in
            guard let userdata, let bytes else { return }
            let data = Data(bytes: bytes, count: Int(len))
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.chunk(id, data) }
        },
        on_release: { userdata in
            guard let userdata else { return }
            Unmanaged<LinkBridge>.fromOpaque(userdata).release()
        })

    private static func of(_ userdata: UnsafeMutableRawPointer) -> LinkBridge {
        Unmanaged<LinkBridge>.fromOpaque(userdata).takeUnretainedValue()
    }
}

/// One mosh pipe's way back from the core, which calls on its own threads.
@MainActor
private final class PipeBridge {
    var pipe: OpaquePointer?
    var ended: CheckedContinuation<Ran, Never>?
    private var lines = LineSplitter()
    private let line: (String) -> Void
    private var closed = false

    init(line: @escaping (String) -> Void) { self.line = line }

    func write(_ text: String) {
        guard let pipe, !closed else { return }
        let bytes = Array((text + "\n").utf8)
        sesh_pipe_write(pipe, bytes, UInt(bytes.count))
    }

    func close() {
        closed = true
        if let pipe { sesh_pipe_close(pipe) }
    }

    private func output(_ data: Data) { lines.feed(data).forEach(line) }

    private func end(_ ran: Ran) {
        if let rest = lines.flush() { line(rest) }
        if let pipe { sesh_pipe_free(pipe) }
        pipe = nil
        ended?.resume(returning: ran)
        ended = nil
    }

    static let callbacks = sesh_pipe_callbacks_t(
        on_output: { userdata, bytes, len in
            guard let userdata, let bytes else { return }
            let data = Data(bytes: bytes, count: Int(len))
            let bridge = of(userdata)
            DispatchQueue.main.async { bridge.output(data) }
        },
        on_end: { userdata, status, err in
            guard let userdata else { return }
            let ran = Ran(status: status, out: "", err: err.map { String(cString: $0) } ?? "")
            let bridge = of(userdata)
            DispatchQueue.main.async { bridge.end(ran) }
        },
        on_release: { userdata in
            guard let userdata else { return }
            Unmanaged<PipeBridge>.fromOpaque(userdata).release()
        })

    private nonisolated static func of(_ userdata: UnsafeMutableRawPointer) -> PipeBridge {
        Unmanaged<PipeBridge>.fromOpaque(userdata).takeUnretainedValue()
    }
}

/// What a terminal on a Link hands back: its output, and its end.
@MainActor
protocol TerminalEvents: AnyObject {
    func output(_ data: Data)
    /// The terminal ended, with why when it failed.
    func ended(_ problem: String?)
}

/// A terminal on a Link's connection, over mosh where the Host is reached directly.
@MainActor
final class CoreTerminal {
    private var handle: OpaquePointer?

    fileprivate init(link: OpaquePointer, command: String, server: String?, cols: UInt16, rows: UInt16, events: TerminalEvents) {
        let bridge = TerminalBridge(events: events)
        handle = sesh_session_terminal(link, server, command, cols, rows, TerminalBridge.callbacks, Unmanaged.passRetained(bridge).toOpaque())
    }

    func write(_ data: Data) {
        guard let handle, !data.isEmpty else { return }
        data.withUnsafeBytes { sesh_session_write(handle, $0.baseAddress?.assumingMemoryBound(to: UInt8.self), UInt($0.count)) }
    }

    func resize(cols: UInt16, rows: UInt16) {
        if let handle { sesh_session_resize(handle, cols, rows) }
    }

    func close() {
        guard let handle else { return }
        sesh_session_close(handle)
        sesh_session_free(handle)
        self.handle = nil
    }

    deinit {
        guard let handle else { return }
        sesh_session_close(handle)
        sesh_session_free(handle)
    }
}

private final class TerminalBridge {
    weak var events: TerminalEvents?

    init(events: TerminalEvents) { self.events = events }

    static let callbacks = sesh_callbacks_t(
        on_output: { userdata, bytes, len in
            guard let userdata, let bytes else { return }
            let data = Data(bytes: bytes, count: Int(len))
            let bridge = of(userdata)
            DispatchQueue.main.async { MainActor.assumeIsolated { bridge.events?.output(data) } }
        },
        on_state: { userdata, state, message in
            guard let userdata, state == SESH_STATE_CLOSED || state == SESH_STATE_FAILED else { return }
            let problem = state == SESH_STATE_FAILED ? (message.map { String(cString: $0) } ?? "the terminal failed") : nil
            let bridge = of(userdata)
            DispatchQueue.main.async { MainActor.assumeIsolated { bridge.events?.ended(problem) } }
        },
        on_host_key: nil, on_auth_prompt: nil, on_upload_progress: nil, on_upload_done: nil, on_ran: nil, on_chunk: nil,
        on_release: { userdata in
            guard let userdata else { return }
            Unmanaged<TerminalBridge>.fromOpaque(userdata).release()
        })

    private static func of(_ userdata: UnsafeMutableRawPointer) -> TerminalBridge {
        Unmanaged<TerminalBridge>.fromOpaque(userdata).takeUnretainedValue()
    }
}

/// This device as the core tags the mosh-servers it starts, so a later run of the app stops
/// those an earlier one left behind.
enum Device {
    static func name() {
        let key = "device"
        let id = UserDefaults.standard.string(forKey: key) ?? UUID().uuidString
        UserDefaults.standard.set(id, forKey: key)
        sesh_set_device(id)
    }
}
