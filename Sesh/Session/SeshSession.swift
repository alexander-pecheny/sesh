import Combine
import PhotosUI
import UIKit
import GhosttyKit

/// One terminal on a Host's Link, wired to a libghostty surface. Its Uploads and its questions
/// for the user belong to the Link.
@MainActor
final class SeshSession: ObservableObject, Identifiable, TerminalEvents {
    enum Stage: Equatable {
        case connecting, connected, closed, failed
    }

    @Published private(set) var stage = Stage.connecting
    @Published private(set) var message = ""
    @Published var selection: CGPoint?
    @Published var editing = false
    @Published var picking = false
    @Published private(set) var title: String?

    let id = UUID()
    let link: HostLink
    /// What runs on the Host: a Terminal Tab's attach to its sesh-mux pane.
    let command: String
    /// The mosh-server Sesh put on the Host, which the Host's own PATH need not have.
    let server: String?
    let input = InputState()
    let terminal: Ghostty.TerminalView

    private var remote: CoreTerminal?
    private var opening = false
    private var relay: AnyCancellable?

    init(link: HostLink, app: ghostty_app_t, command: String, server: String?) {
        self.link = link
        self.command = command
        self.server = server
        terminal = Ghostty.TerminalView(app: app, input: input, fontSize: Store.shared.fontSize)
        relay = link.objectWillChange.sink { [weak self] in self?.objectWillChange.send() }

        terminal.onWrite = { [weak self] data in self?.remote?.write(data) }
        terminal.onResize = { [weak self] cols, rows in self?.resize(cols, rows) }
        terminal.onSelection = { [weak self] anchor in self?.selection = anchor }
        terminal.onEditor = { [weak self] in self?.editing = true }
        terminal.onUpload = { [weak self] in self?.picking = true }
        terminal.onTitle = { [weak self] title in self?.title = title }
    }

    var host: Host { link.host }

    var name: String { title.map { $0.isEmpty ? host.title : $0 } ?? host.title }

    var ended: Bool { stage == .closed || stage == .failed }

    var reason: String {
        guard stage == .failed else { return "the remote shell exited" }
        return message.isEmpty ? "the Session failed" : message
    }

    /// A fresh terminal in the same Tab, on the same surface, so the scrollback survives.
    func reconnect() {
        remote?.close()
        remote = nil
        message = ""
        stage = .connecting
        let size = terminal.gridSize
        open(size.0, size.1)
    }

    /// The first layout is also the first honest grid size, so opening waits for it and the
    /// remote never sees the 80x24 placeholder.
    private func resize(_ cols: UInt16, _ rows: UInt16) {
        guard let remote else { return open(cols, rows) }
        remote.resize(cols: cols, rows: rows)
    }

    private func open(_ cols: UInt16, _ rows: UInt16) {
        guard !opening else { return }
        opening = true
        Task {
            defer { opening = false }
            switch await link.terminal(command, server: server, cols: cols, rows: rows, events: self) {
            case .success(let opened):
                remote = opened
                stage = .connected
            case .failure(let failure):
                message = failure.message
                stage = .failed
            }
        }
    }

    func output(_ data: Data) { terminal.process(data) }

    func ended(_ problem: String?) {
        remote = nil
        if let problem { message = problem }
        stage = problem == nil ? .closed : .failed
    }

    func copySelection() {
        guard let text = terminal.selection()?.text else { return }
        UIPasteboard.general.string = text
        selection = nil
    }

    // MARK: Uploads, over the Link

    var uploading: HostLink.Progress? { link.uploading }
    var canUpload: Bool { stage == .connected && link.canUpload }
    var uploadError: String? {
        get { link.uploadError }
        set { link.uploadError = newValue }
    }

    func upload(_ results: [PHPickerResult], insert: @escaping (String) -> Void) { link.upload(results, insert: insert) }

    func cancelUpload() { link.cancelUpload() }

    func sendDraft(_ text: String, enter: Bool) {
        terminal.paste(text)
        if enter { terminal.send(keycode: Keycode.enter, mods: []) }
    }

    func close() {
        remote?.close()
        remote = nil
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
