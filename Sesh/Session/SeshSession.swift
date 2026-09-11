import Foundation
import GhosttyKit

/// Owns one sesh-core Session and wires it to a libghostty surface. Core callbacks arrive
/// on tokio threads; `Bridge` is the only thing they touch, and it hops to main.
@MainActor
final class SeshSession: ObservableObject {
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

    enum Stage: Equatable {
        case connecting, authenticating, connected, closed, failed
    }

    @Published private(set) var stage = Stage.connecting
    @Published private(set) var message = ""
    @Published var hostKeyQuestion: HostKeyQuestion?
    @Published var authQuestion: AuthQuestion?
    @Published var savePassword = false

    let host: Host
    let terminal: Ghostty.TerminalView

    private var handle: OpaquePointer?
    private let bridge = Bridge()
    private var lastAnswer: String?

    init(host: Host, store: Store, app: ghostty_app_t) {
        self.host = host
        terminal = Ghostty.TerminalView(app: app)
        bridge.terminal = terminal
        bridge.owner = self

        let key = store.key(host.keyID)
        let strings = CStrings()
        var config = sesh_ssh_config_t()
        config.host = strings.make(host.address)
        config.port = UInt16(host.port)
        config.user = strings.make(host.user)
        config.password = strings.make(Keychain.read("password.\(host.id)"))
        config.key_pem = strings.make(key.flatMap { Keychain.read("key.\($0.id)") })
        config.key_passphrase = strings.make(key.flatMap { Keychain.read("passphrase.\($0.id)") })
        config.known_hosts_path = strings.make(store.knownHostsPath)
        config.term = strings.make("xterm-256color")
        config.remote_command = strings.make(host.remoteCommand)
        config.extra_flags = strings.make(host.sshFlags)
        config.cols = 80
        config.rows = 24
        config.agent_forwarding = host.agentForwarding

        handle = sesh_ssh_connect(&config, Bridge.callbacks, Unmanaged.passRetained(bridge).toOpaque())

        terminal.onWrite = { [weak self] data in self?.send(data) }
        terminal.onResize = { [weak self] cols, rows in
            guard let handle = self?.handle else { return }
            sesh_session_resize(handle, cols, rows)
        }
    }

    deinit {
        guard let handle else { return }
        sesh_session_close(handle)
        sesh_session_free(handle)
    }

    func close() {
        guard let handle else { return }
        sesh_session_close(handle)
    }

    func send(_ data: Data) {
        guard let handle else { return }
        data.withUnsafeBytes { sesh_session_write(handle, $0.baseAddress?.assumingMemoryBound(to: UInt8.self), UInt($0.count)) }
    }

    func answerHostKey(_ accept: Bool) {
        hostKeyQuestion = nil
        guard let handle else { return }
        sesh_session_answer_host_key(handle, accept)
    }

    func answerPrompt(_ question: AuthQuestion, _ answers: [String]?) {
        authQuestion = nil
        guard let handle else { return }
        guard let answers else {
            sesh_session_answer_prompt(handle, question.id, nil, 0)
            return
        }
        if savePassword, let first = answers.first, question.prompts.count == 1 {
            Keychain.write(first, to: "password.\(host.id)")
        }
        let strings = CStrings()
        let pointers = answers.map { strings.make($0) }
        pointers.withUnsafeBufferPointer { sesh_session_answer_prompt(handle, question.id, $0.baseAddress, UInt($0.count)) }
    }

    fileprivate func apply(_ state: UInt32, _ message: String) {
        stage =
            switch state {
            case 0: .connecting
            case 1: .authenticating
            case 2: .connected
            case 3: .closed
            default: .failed
            }
        if !message.isEmpty { self.message = message }
    }
}

private final class CStrings {
    private var allocated: [UnsafeMutablePointer<CChar>] = []

    func make(_ value: String?) -> UnsafePointer<CChar>? {
        guard let value, let copy = strdup(value) else { return nil }
        allocated.append(copy)
        return UnsafePointer(copy)
    }

    deinit { allocated.forEach { free($0) } }
}

private final class Bridge {
    weak var owner: SeshSession?
    var terminal: Ghostty.TerminalView?

    private let lock = NSLock()
    private var pending = Data()
    private var draining = false

    static let callbacks = sesh_callbacks_t(
        on_output: { userdata, bytes, len in
            guard let userdata, let bytes, len > 0 else { return }
            Bridge.of(userdata).output(Data(bytes: bytes, count: Int(len)))
        },
        on_state: { userdata, state, message in
            guard let userdata else { return }
            let text = message.map { String(cString: $0) } ?? ""
            let bridge = Bridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.apply(state.rawValue, text) }
        },
        on_host_key: { userdata, fingerprint, previous in
            guard let userdata, let fingerprint else { return }
            let question = SeshSession.HostKeyQuestion(
                fingerprint: String(cString: fingerprint),
                previous: previous.map { String(cString: $0) })
            let bridge = Bridge.of(userdata)
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
            let bridge = Bridge.of(userdata)
            DispatchQueue.main.async { bridge.owner?.authQuestion = question }
        },
        on_release: { userdata in
            guard let userdata else { return }
            Unmanaged<Bridge>.fromOpaque(userdata).release()
        })

    private static func of(_ userdata: UnsafeMutableRawPointer) -> Bridge {
        Unmanaged<Bridge>.fromOpaque(userdata).takeUnretainedValue()
    }

    /// Remote output can outrun the main queue, so it is coalesced into one hop per runloop.
    func output(_ data: Data) {
        lock.lock()
        pending.append(data)
        let schedule = !draining
        draining = true
        lock.unlock()
        guard schedule else { return }
        DispatchQueue.main.async { [self] in
            lock.lock()
            let chunk = pending
            pending.removeAll(keepingCapacity: true)
            draining = false
            lock.unlock()
            terminal?.process(chunk)
        }
    }
}

enum Flags {
    static func validate(_ transport: Host.Transport, _ text: String) -> String? {
        var error: UnsafeMutablePointer<CChar>?
        let kind = transport == .ssh ? SESH_TRANSPORT_SSH : SESH_TRANSPORT_MOSH
        if sesh_parse_flags(kind, text, &error) { return nil }
        defer { sesh_string_free(error) }
        return error.map { String(cString: $0) } ?? "invalid flags"
    }
}

enum PrivateKey {
    static func publicKey(_ pem: String, passphrase: String?) -> (line: String?, error: String?) {
        var error: UnsafeMutablePointer<CChar>?
        let line = sesh_public_key(pem, passphrase ?? "", &error)
        defer {
            sesh_string_free(line)
            sesh_string_free(error)
        }
        guard let line else { return (nil, error.map { String(cString: $0) } ?? "unreadable key") }
        return (String(cString: line), nil)
    }
}
