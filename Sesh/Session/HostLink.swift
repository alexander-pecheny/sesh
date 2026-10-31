import PhotosUI
import SwiftUI

/// The Link to a Host from the phone's list, which keeps its password in the Keychain and
/// carries photo Uploads.
@MainActor
final class HostLink: CoreLink {
    @Published var savePassword = false
    @Published private(set) var uploading: SeshSession.Progress?
    @Published var uploadError: String?

    let host: Host
    private let store: Store
    private var uploaded: (([String], String?) -> Void)?
    /// Kept with the Link, so a Host edited to point elsewhere is prepared afresh.
    private(set) lazy var installer = HelperInstaller(
        title: host.title, run: { [unowned self] in await run($0) }, put: { [unowned self] in await put($0, to: $1) })

    init(host: Host, store: Store) {
        self.host = host
        self.store = store
        super.init(title: host.title) { callbacks, userdata in
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
            return sesh_link_connect(&config, callbacks, userdata)
        }
        savePassword = Keychain.read("password.\(host.id)") != nil
    }

    override func passwordRejected() {
        Keychain.write(nil, to: "password.\(host.id)")
        savePassword = false
    }

    override func answerPrompt(_ question: AuthQuestion, _ answers: [String]?) {
        if savePassword, let first = answers?.first, question.prompts.count == 1 {
            Keychain.write(first, to: "password.\(host.id)")
        }
        super.answerPrompt(question, answers)
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

    override func uploadProgressed(_ id: UInt32, _ done: UInt64, _ total: UInt64) {
        guard uploading?.id == id else { return }
        uploading = SeshSession.Progress(id: id, done: done, total: total)
    }

    override func uploadDone(_ id: UInt32, _ paths: [String], _ error: String?) {
        guard uploading?.id == id, let finish = uploaded else { return }
        uploaded = nil
        finish(paths, error)
    }
}
