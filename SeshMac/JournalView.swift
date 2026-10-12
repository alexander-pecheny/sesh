import SwiftUI

/// A Task's working log: a box for the next Entry, then every Entry, newest first.
struct JournalView: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: String
    @State private var draft = ""
    @StateObject private var field = PlainField()

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    private var entries: [Record] {
        vault.children(.entry, task: task).sorted { ($0.body.at ?? 0, $0.id) > ($1.body.at ?? 0, $1.id) }
    }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Metric.wide) {
                composer
                ForEach(conflicts) { ConflictRow(vault: vault, conflict: $0) }
                ForEach(entries) { EntryRow(vault: vault, entry: $0) }
            }
            .padding(Metric.wide)
            .frame(maxWidth: 820)
            .frame(maxWidth: .infinity)
        }
        .background(flavour(.base), ignoresSafeAreaEdges: .vertical)
        .environment(\.openURL, OpenURLAction { url in library.follow(url) ? .handled : .systemAction })
        .onAppear { draft = library.draft(for: "journal.\(task)") }
        .onChange(of: draft) { library.saveDraft(draft, for: "journal.\(task)") }
    }

    private var conflicts: [Record] { vault.children(.conflict, task: task).filter { $0.body.kind == "entry" } }

    private var composer: some View {
        VStack(alignment: .trailing, spacing: Metric.gap) {
            PlainText(field: field, text: $draft, font: .systemFont(ofSize: Metric.body), lines: 8) { add() }
                .overlay(alignment: .topLeading) {
                    if draft.isEmpty {
                        Text("What happened? Return adds it, shift-return starts a line.")
                            .font(.ui(Metric.body)).foregroundStyle(flavour(.overlay0)).allowsHitTesting(false)
                    }
                }
                .padding(Metric.pad)
                .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
        }
    }

    private func add() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        _ = vault.create(.entry, .init(text: text, task: task, edited: now, at: now))
        draft = ""
    }
}

private struct EntryRow: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let entry: Record
    @State private var editing = false
    @State private var text = ""
    @State private var deleting = false
    /// The Entry's seq when editing began, which a save builds on.
    @State private var base: Int?
    @StateObject private var field = PlainField()

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.tiny) {
            HStack {
                Text(Self.stamp(entry.body.at))
                    .font(.ui(Metric.small).monospacedDigit())
                    .foregroundStyle(flavour(.subtext0))
                Spacer()
                Menu { actions } label: { Image(systemName: "ellipsis").foregroundStyle(flavour(.subtext0)) }
                    .menuIndicator(.hidden)
                    .fixedSize()
                    .accessibilityLabel("Entry actions")
            }
            if editing {
                PlainText(field: field, text: $text, font: .systemFont(ofSize: Metric.body), lines: 20) { save() }
                    .padding(Metric.gap)
                    .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
                    .onExitCommand { stop() }
                    .onAppear { DispatchQueue.main.async { field.focus() } }
            } else {
                Markdown(text: entry.body.text ?? "")
                    .textSelection(.enabled)
                    .onTapGesture(count: 2) { edit() }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contextMenu { actions }
        .onAppear {
            let saved = library.draft(for: key)
            if !saved.isEmpty { (text, editing) = (saved, true) }
        }
        .onChange(of: text) { if editing { library.saveDraft(text, for: key) } }
        .confirmationDialog("Delete this Entry?", isPresented: $deleting) {
            Button("Delete", role: .destructive) {
                library.saveDraft("", for: key)
                vault.delete(entry)
            }
        } message: {
            Text((entry.body.text ?? "").prefix(120))
        }
    }

    private var key: String { "entry.\(entry.id)" }

    @ViewBuilder private var actions: some View {
        Button("Edit") { edit() }
        Button("Copy") { Pasteboard.copy(entry.body.text ?? "") }
        Button("Delete…", role: .destructive) { deleting = true }
    }

    private func stop() {
        library.saveDraft("", for: key)
        editing = false
    }

    private func edit() {
        text = entry.body.text ?? ""
        base = entry.seq
        editing = true
    }

    /// An Entry emptied of its words is asked about, as a deletion, rather than kept blank.
    private func save() {
        let words = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !words.isEmpty else { return deleting = true }
        var record = entry
        record.body.text = words
        record.body.edited = Int64(Date().timeIntervalSince1970 * 1000)
        vault.write(record, base: base)
        stop()
    }

    static func stamp(_ at: Int64?) -> String {
        guard let at else { return "" }
        let date = Date(timeIntervalSince1970: Double(at) / 1000)
        return date.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated).hour().minute())
    }
}

/// The text that lost when two devices edited one Entry, kept beside the winner.
private struct ConflictRow: View {
    @ObservedObject var vault: Vault
    let conflict: Record

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.tiny) {
            Label("Conflict copy: another device's edit of this Entry", systemImage: "exclamationmark.triangle")
                .font(.ui(Metric.small)).foregroundStyle(.orange)
            Markdown(text: conflict.body.text ?? "").textSelection(.enabled)
            Button("Delete this copy", role: .destructive) { vault.delete(conflict) }.buttonStyle(.link)
        }
        .padding(Metric.pad)
        .overlay(RoundedRectangle(cornerRadius: Metric.corner).stroke(.orange.opacity(0.5)))
    }
}
