import PhotosUI
import SwiftUI

/// One Link to a Host, shared by everything the phone runs there: Vaults, herdr, the helper,
/// Conversations. It has no terminal. Core callbacks arrive on tokio threads and hop to main
/// in `LinkBridge`.
@MainActor
final class HostLink: ObservableObject, Identifiable {
    enum Stage: Equatable {
        case connecting, authenticating, ready, failed
    }

    @Published private(set) var stage = Stage.connecting
    @Published private(set) var message = ""
    @Published var hostKeyQuestion: SeshSession.HostKeyQuestion? { didSet { Asking.shared.update() } }
    @Published var authQuestion: SeshSession.AuthQuestion? { didSet { Asking.shared.update() } }
    @Published var savePassword = false
    @Published private(set) var uploading: SeshSession.Progress?
    @Published var uploadError: String?

    let id = UUID()
    let host: Host
    private let store: Store
    private var handle: OpaquePointer?
    private var bridge = LinkBridge()
    private var waiting: [UInt32: CheckedContinuation<Ran, Never>] = [:]
    private var streams: [UInt32: (lines: LineSplitter, line: (String) -> Void)] = [:]
    private var linking: [CheckedContinuation<Bool, Never>] = []
    private var linked = false
    private var paused = false
    private var uploaded: (([String], String?) -> Void)?
    /// Kept with the Link, so a Host edited to point elsewhere is prepared afresh.
    private(set) lazy var installer = HelperInstaller(
        title: host.title, run: { [unowned self] in await run($0) }, put: { [unowned self] in await put($0, to: $1) })

    init(host: Host, store: Store) {
        self.host = host
        self.store = store
        savePassword = Keychain.read("password.\(host.id)") != nil
    }

    var title: String { host.title }

    /// Also called on the way to the background: iOS would freeze the socket, and a command
    /// sent on it after waking would wait minutes for TCP to admit it is dead.
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

        let key = store.key(host.keyID)
        let strings = CStrings()
        var config = sesh_link_config_t(
            host: strings.make(host.address), port: UInt16(host.port), user: strings.make(host.user),
            password: strings.make(Keychain.read("password.\(host.id)")),
            key_pem: strings.make(key.flatMap { Keychain.read("key.\($0.id)") }),
            key_passphrase: strings.make(key.flatMap { Keychain.read("passphrase.\($0.id)") }),
            known_hosts_path: strings.make(store.knownHostsPath),
            extra_flags: strings.make(host.transport == .ssh ? host.sshFlags : host.moshFlags),
            transport: host.transport == .ssh ? SESH_TRANSPORT_SSH : SESH_TRANSPORT_MOSH)
        handle = sesh_link_connect(&config, LinkBridge.callbacks, Unmanaged.passRetained(bridge).toOpaque())
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

    private func call(line: ((String) -> Void)? = nil, start: (OpaquePointer) -> UInt32) async -> Ran {
        guard !paused else { return Ran(status: -1, out: "", err: "not connected") }
        if !linked {
            if handle == nil { connect() }
            guard await withCheckedContinuation({ linking.append($0) }) else {
                return Ran(status: -1, out: "", err: message.isEmpty ? "not connected" : message)
            }
        }
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

    // MARK: Uploads

    var canUpload: Bool { linked && uploading == nil }

    /// As in a Terminal Tab, but over the Link: the paths reach `insert` once all have landed.
    func upload(_ results: [PHPickerResult], insert: @escaping (String) -> Void) {
        guard canUpload, !results.isEmpty else { return }
        uploading = SeshSession.Progress(id: 0, done: 0, total: 1)
        Task {
            let files: [PreparedUpload]
            do {
                files = try await Uploads.prepare(results, compress: store.compressUploads)
            } catch {
                uploading = nil
                uploadError = error.localizedDescription
                return
            }
            guard uploading != nil, let handle, let id = Uploads.start(files, on: handle) else {
                if uploading != nil { uploadError = "the Upload could not be started" }
                uploading = nil
                return Uploads.discard(files)
            }
            uploading = SeshSession.Progress(id: id, done: 0, total: 1)
            uploaded = { [weak self] paths, error in
                Uploads.discard(files)
                self?.uploading = nil
                self?.uploadError = error
                if !paths.isEmpty { insert(Uploads.text(for: paths)) }
            }
        }
    }

    /// Frees the button at once: a stalled transfer may never report that it stopped.
    func cancelUpload() {
        guard let uploading else { return }
        if uploading.id != 0, let handle { sesh_session_cancel_upload(handle, uploading.id) }
        self.uploading = nil
        let finish = uploaded
        uploaded = nil
        finish?([], nil)
    }

    fileprivate func uploadProgressed(_ id: UInt32, _ done: UInt64, _ total: UInt64) {
        guard uploading?.id == id else { return }
        uploading = SeshSession.Progress(id: id, done: done, total: total)
    }

    fileprivate func uploadDone(_ id: UInt32, _ paths: [String], _ error: String?) {
        guard uploading?.id == id, let finish = uploaded else { return }
        uploaded = nil
        finish(paths, error)
    }

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
        if result.status == -1, result.err.hasPrefix("the connection to the Host is gone") { drop(result.err) }
    }

    fileprivate func chunk(_ id: UInt32, _ data: Data) {
        guard var stream = streams[id] else { return }
        let lines = stream.lines.feed(data)
        streams[id] = stream
        lines.forEach(stream.line)
    }

    fileprivate func apply(_ state: UInt32, _ message: String) {
        switch state {
        case SESH_STATE_PASSWORD_REJECTED.rawValue:
            Keychain.write(nil, to: "password.\(host.id)")
            savePassword = false
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

    func answerPrompt(_ question: SeshSession.AuthQuestion, _ answers: [String]?) {
        authQuestion = nil
        guard let handle else { return }
        guard let answers else { return sesh_session_answer_prompt(handle, question.id, nil, 0) }
        if savePassword, let first = answers.first, question.prompts.count == 1 {
            Keychain.write(first, to: "password.\(host.id)")
        }
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
    @Published private(set) var link: HostLink?
    var links: () -> [HostLink] = { [] }

    func update() {
        link = links().first { $0.hostKeyQuestion != nil || $0.authQuestion != nil }
    }
}

private final class LinkBridge {
    weak var owner: HostLink?

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
            let question = SeshSession.HostKeyQuestion(
                fingerprint: String(cString: fingerprint),
                previous: previous.map { String(cString: $0) })
            let bridge = LinkBridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.hostKeyQuestion = question }
        },
        on_auth_prompt: { userdata, id, name, instruction, prompts, echoes, count in
            guard let userdata, let prompts, let echoes else { return }
            let question = SeshSession.AuthQuestion(
                id: id,
                title: name.map { String(cString: $0) } ?? "Authentication",
                instruction: instruction.map { String(cString: $0) } ?? "",
                prompts: (0..<Int(count)).map {
                    (prompts[$0].map { String(cString: $0) } ?? "", echoes[$0])
                })
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
            let result = Ran(
                status: status,
                out: out.map { String(cString: $0) } ?? "",
                err: err.map { String(cString: $0) } ?? "")
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
