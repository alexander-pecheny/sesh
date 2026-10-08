import SwiftUI

/// A Task's working log: a box for the next Entry, then every Entry, newest first.
struct JournalScreen: View {
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

    private var conflicts: [Record] { vault.children(.conflict, task: task).filter { $0.body.kind == "entry" } }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Metric.wide) {
                composer
                ForEach(conflicts) { ConflictRow(vault: vault, conflict: $0) }
                ForEach(entries) { EntryRow(vault: vault, entry: $0) }
                if entries.isEmpty {
                    Text("Nothing written yet. Sesh never adds an Entry on its own.")
                        .font(.ui(Metric.note)).foregroundStyle(flavour(.subtext0))
                }
            }
            .padding(Metric.wide)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(flavour(.base))
        .environment(\.openURL, OpenURLAction { url in library.follow(url) ? .handled : .systemAction })
    }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: Metric.gap) {
            PlainText(field: field, text: $draft, font: .systemFont(ofSize: Metric.body), lines: 8)
                .overlay(alignment: .topLeading) {
                    if draft.isEmpty {
                        Text("What happened?").font(.ui(Metric.body)).foregroundStyle(flavour(.overlay0)).allowsHitTesting(false)
                    }
                }
                .padding(Metric.pad)
                .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
            Button(action: add) {
                Image.lucide("arrow-up", size: 18)
                    .foregroundStyle(flavour(.base))
                    .frame(width: Metric.control, height: Metric.control)
                    .background(flavour(.mauve).opacity(trimmed.isEmpty ? 0.4 : 1), in: .circle)
            }
            .disabled(trimmed.isEmpty)
            .accessibilityLabel("Add Entry")
        }
    }

    private var trimmed: String { draft.trimmingCharacters(in: .whitespacesAndNewlines) }

    private func add() {
        guard !trimmed.isEmpty else { return }
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        _ = vault.create(.entry, .init(text: trimmed, task: task, edited: now, at: now))
        draft = ""
    }
}

private struct EntryRow: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var vault: Vault
    let entry: Record
    @State private var editing = false
    @State private var text = ""
    @State private var deleting = false
    @StateObject private var field = PlainField()

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.tiny) {
            Text(Self.stamp(entry.body.at))
                .font(.ui(Metric.small).monospacedDigit())
                .foregroundStyle(flavour(.subtext0))
            if editing {
                PlainText(field: field, text: $text, font: .systemFont(ofSize: Metric.body), lines: 20)
                    .padding(Metric.gap)
                    .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
                    .onAppear { DispatchQueue.main.async { field.focus() } }
                HStack {
                    Button("Cancel") { editing = false }
                    Spacer()
                    Button("Save", action: save).buttonStyle(.borderedProminent)
                }
                .font(.ui(Metric.label))
            } else {
                Markdown(text: entry.body.text ?? "")
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(.rect)
        .contextMenu {
            Button("Edit") { edit() }
            Button("Copy") { UIPasteboard.general.string = entry.body.text }
            Button("Delete…", role: .destructive) { deleting = true }
        }
        .confirmationDialog("Delete this Entry?", isPresented: $deleting) {
            Button("Delete", role: .destructive) { vault.delete(entry) }
        } message: {
            Text((entry.body.text ?? "").prefix(120))
        }
    }

    private func edit() {
        text = entry.body.text ?? ""
        editing = true
    }

    /// An Entry emptied of its words is asked about, as a deletion, rather than kept blank.
    private func save() {
        let words = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !words.isEmpty else { return deleting = true }
        var record = entry
        record.body.text = words
        record.body.edited = Int64(Date().timeIntervalSince1970 * 1000)
        vault.write(record)
        editing = false
    }

    static func stamp(_ at: Int64?) -> String {
        guard let at else { return "" }
        let date = Date(timeIntervalSince1970: Double(at) / 1000)
        return date.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated).hour().minute())
    }
}

/// The text that lost when two devices edited one Entry, kept beside the winner.
private struct ConflictRow: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var vault: Vault
    let conflict: Record

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.tiny) {
            Label("Conflict copy: another device's edit of this Entry", systemImage: "exclamationmark.triangle")
                .font(.ui(Metric.small)).foregroundStyle(flavour(.peach))
            Markdown(text: conflict.body.text ?? "").textSelection(.enabled)
            Button("Delete this copy", role: .destructive) { vault.delete(conflict) }.font(.ui(Metric.note))
        }
        .padding(Metric.pad)
        .overlay(RoundedRectangle(cornerRadius: Metric.corner).stroke(flavour(.peach).opacity(0.5)))
    }
}
