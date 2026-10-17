import Foundation

/// A Document's text as its Tab shows and edits it: Markdown kept in the Vault, or a file on a
/// machine that the Vault remembers the last text of. Edits to a file go to the file itself
/// (ADR 0008).
@MainActor
final class DocumentText: ObservableObject {
    @Published var text = "" { didSet { if loaded { keep(dirty ? text : "") } } }
    /// The file's text when last read or written; what a save expects to find there.
    @Published private(set) var base: String?
    @Published private(set) var loaded = false
    @Published var unreachable: String?
    @Published var clash: String?
    @Published private(set) var saving = false
    @Published private(set) var image: PlatformImage?

    let vault: Vault
    let id: String
    /// Unsaved edits, kept across restarts: what `keep` saved before, and how to save them.
    var kept = ""
    /// The Vault Document's seq when its text was read, which a save builds on.
    private var started: Int?
    var keep: (String) -> Void = { _ in }

    /// A file a folder's browser shows, which no Record remembers.
    private var loose: (path: String, machine: Machine)?
    /// Where its browser was left, which outlives the browser when the user switches Tabs.
    var browsed: (folder: String, picked: String?, files: [String: DocumentText])?

    init(vault: Vault, id: String) {
        self.vault = vault
        self.id = id
    }

    init(vault: Vault, path: String, machine: Machine) {
        self.vault = vault
        id = ""
        loose = (path, machine)
    }

    var record: Record? { loose == nil ? vault.records[id] : nil }
    var path: String? { loose?.path ?? record?.body.path }
    var machine: Machine { loose?.machine ?? TaskActions.machine(record?.body.machine) }
    /// A folder, which its Tab browses rather than shows.
    var folder: Bool { path?.hasSuffix("/") == true }
    var dirty: Bool { loaded && text != (path == nil ? record?.body.text ?? "" : base ?? record?.body.text ?? "") }
    private var ext: String? { path.map { ($0 as NSString).pathExtension.lowercased() } }
    var markdown: Bool { ext.map { ["md", "markdown"].contains($0) } ?? true }
    var html: Bool { ext.map { ["html", "htm"].contains($0) } ?? false }
    var picture: Bool { ext.map { ["png", "jpg", "jpeg", "gif", "webp", "heic", "heif", "tiff", "bmp"].contains($0) } ?? false }
    /// Any other text file, which opens straight in the code editor.
    var code: Bool { !markdown && !html && !picture }
    /// A file is read-only while its machine cannot be reached or it is not text.
    var editable: Bool { !picture && unreachable == nil && loaded }

    private static let binary: Int32 = 3

    func load() async {
        await read()
        loaded = true
        if !kept.isEmpty, editable { text = kept }
        kept = ""
    }

    private func read() async {
        guard let path else {
            guard let record else { return }
            started = record.seq
            return text = record.body.text ?? ""
        }
        if picture {
            image = await machine.read(path).flatMap(PlatformImage.init(data:))
            unreachable = image == nil ? "\((path as NSString).lastPathComponent) could not be read from \(machine.title)." : nil
            return
        }
        // An image or other binary, laid out as text, would hang the app on every launch.
        let file = shellPath(path)
        let ran = await machine.run("if [ -s \(file) ] && ! grep -qI . \(file); then exit \(Self.binary); fi; cat -- \(file)")
        if ran.status == Self.binary {
            unreachable = "\((path as NSString).lastPathComponent) is not a text file, so Sesh does not show it."
        } else if ran.ok {
            text = ran.out
            base = ran.out
            unreachable = nil
            remember(ran.out)
        } else {
            text = record?.body.text ?? ""
            unreachable = "Showing the last copy Sesh saw: \(machine.title) could not be read (\(ran.problem))."
        }
    }

    func write(force: Bool) async {
        defer { keep(dirty ? text : "") }
        guard let path else {
            guard var record else { return }
            record.body.text = text
            record.body.edited = Int64(Date().timeIntervalSince1970 * 1000)
            vault.write(record, base: started)
            started = vault.records[id]?.seq
            return
        }
        saving = true
        defer { saving = false }
        if !force {
            let now = await machine.run("cat -- \(shellPath(path))")
            if now.ok, now.out != base { return clash = now.out }
        }
        let put = await machine.put(Data(text.utf8), to: path)
        guard put.ok else { return unreachable = "Not saved: \(put.problem)" }
        unreachable = nil
        base = text
        remember(text)
    }

    /// Gives up the edits for what the file holds now.
    func takeClash() {
        guard let clash else { return }
        text = clash
        base = clash
        remember(clash)
    }

    /// The Vault keeps the file's last text, for reading it when the machine is off.
    private func remember(_ seen: String) {
        guard var record, record.body.text != seen else { return }
        record.body.text = seen
        record.body.edited = Int64(Date().timeIntervalSince1970 * 1000)
        vault.write(record)
    }
}
