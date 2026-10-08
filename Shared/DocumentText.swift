import Foundation

/// A Document's text as its Tab shows and edits it: Markdown kept in the Vault, or a file on a
/// machine that the Vault remembers the last text of. Edits to a file go to the file itself
/// (ADR 0008).
@MainActor
final class DocumentText: ObservableObject {
    @Published var text = ""
    /// The file's text when last read or written; what a save expects to find there.
    @Published private(set) var base: String?
    @Published private(set) var loaded = false
    @Published var unreachable: String?
    @Published var clash: String?
    @Published private(set) var saving = false

    let vault: Vault
    let id: String

    init(vault: Vault, id: String) {
        self.vault = vault
        self.id = id
    }

    var record: Record? { vault.records[id] }
    var path: String? { record?.body.path }
    var machine: Machine { TaskActions.machine(record?.body.machine) }
    var dirty: Bool { loaded && text != (path == nil ? record?.body.text ?? "" : base ?? record?.body.text ?? "") }
    var markdown: Bool { path.map { ["md", "markdown"].contains(($0 as NSString).pathExtension.lowercased()) } ?? true }
    /// A file is read-only while its machine cannot be reached, and every non-Markdown file is.
    var editable: Bool { markdown && unreachable == nil && loaded }

    private static let binary: Int32 = 3

    func load() async {
        guard let record else { return }
        guard let path else {
            text = record.body.text ?? ""
            return loaded = true
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
            text = record.body.text ?? ""
            unreachable = "Showing the last copy Sesh saw: \(machine.title) could not be read (\(ran.problem))."
        }
        loaded = true
    }

    func write(force: Bool) async {
        guard var record else { return }
        guard let path else {
            record.body.text = text
            record.body.edited = Int64(Date().timeIntervalSince1970 * 1000)
            return vault.write(record)
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
