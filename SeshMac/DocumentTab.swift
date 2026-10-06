import SwiftUI

/// A Document of the Task: Markdown kept in the Vault, or a file on a machine that the Vault
/// remembers the last text of. Edits to a file go to the file itself (ADR 0008).
struct DocumentTab: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var vault: Vault
    let id: String
    @State private var text = ""
    /// The file's text when last read or written; what a save expects to find there.
    @State private var base: String?
    @State private var editing = false
    @State private var loaded = false
    @State private var unreachable: String?
    @State private var clash: String?
    @State private var saving = false
    @StateObject private var field = PlainField()

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var record: Record? { vault.records[id] }
    private var path: String? { record?.body.path }
    private var machine: Machine { TaskActions.machine(record?.body.machine) }
    private var dirty: Bool { loaded && text != (path == nil ? record?.body.text ?? "" : base ?? record?.body.text ?? "") }
    private var markdown: Bool { path.map { ["md", "markdown"].contains(($0 as NSString).pathExtension.lowercased()) } ?? true }

    var body: some View {
        VStack(spacing: 0) {
            bar
            Divider()
            if let unreachable {
                Label(unreachable, systemImage: "wifi.slash")
                    .font(.caption).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(Metric.gap)
            }
            if editing {
                PlainText(field: field, text: $text, font: .monospacedSystemFont(ofSize: Metric.label, weight: .regular))
                    .padding(Metric.pad)
                    .onAppear { DispatchQueue.main.async { field.focus() } }
            } else {
                ScrollView {
                    Group {
                        if markdown { Markdown(text: text) } else {
                            Text(text).font(.system(size: Metric.note, design: .monospaced))
                        }
                    }
                    .textSelection(.enabled)
                    .padding(Metric.wide)
                    .frame(maxWidth: 820, alignment: .leading)
                    .frame(maxWidth: .infinity)
                }
            }
        }
        .background(flavour(.base))
        .task { await load() }
        .alert("The file changed since Sesh read it", isPresented: Binding(get: { clash != nil }, set: { if !$0 { clash = nil } })) {
            Button("Keep my text", role: .destructive) { Task { await write(force: true) } }
            Button("Take the file's text") {
                if let clash { text = clash; base = clash; remember(clash) }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Someone, probably an Agent, rewrote \(path ?? "it") while you were editing. Keeping your text overwrites theirs.")
        }
    }

    private var bar: some View {
        HStack(spacing: Metric.gap) {
            Text(path ?? "In the Vault").font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
            Spacer()
            if saving { ProgressView().controlSize(.small) }
            if dirty { Text("Edited").font(.caption).foregroundStyle(.secondary) }
            Picker("", selection: $editing) {
                Text("Read").tag(false)
                Text("Edit").tag(true)
            }
            .pickerStyle(.segmented)
            .fixedSize()
            .disabled(!editable)
            Button("Save") { Task { await write(force: false) } }
                .keyboardShortcut("s")
                .disabled(!dirty || saving)
        }
        .padding(.horizontal, Metric.pad)
        .frame(height: 32)
    }

    /// A file is read-only while its machine cannot be reached, and every non-Markdown file is.
    private var editable: Bool { markdown && unreachable == nil && loaded }

    private func load() async {
        guard let record else { return }
        guard let path else {
            text = record.body.text ?? ""
            return loaded = true
        }
        let ran = await machine.run("cat -- \(Machine.shellPath(path))")
        if ran.ok {
            text = ran.out
            base = ran.out
            remember(ran.out)
        } else {
            text = record.body.text ?? ""
            unreachable = "Showing the last copy Sesh saw: \(machine.title) could not be read (\(ran.problem))."
        }
        loaded = true
    }

    private func write(force: Bool) async {
        guard var record else { return }
        guard let path else {
            record.body.text = text
            record.body.edited = Int64(Date().timeIntervalSince1970 * 1000)
            return vault.write(record)
        }
        saving = true
        defer { saving = false }
        if !force {
            let now = await machine.run("cat -- \(Machine.shellPath(path))")
            if now.ok, now.out != base { return clash = now.out }
        }
        let put = await machine.put(Data(text.utf8), to: path)
        guard put.ok else { return unreachable = "Not saved: \(put.problem)" }
        base = text
        remember(text)
    }

    /// The Vault keeps the file's last text, for reading it when the machine is off.
    private func remember(_ seen: String) {
        guard var record, record.body.text != seen else { return }
        record.body.text = seen
        record.body.edited = Int64(Date().timeIntervalSince1970 * 1000)
        vault.write(record)
    }
}
