import SwiftUI

/// A Document of the Task, to read, and to edit when it is Markdown.
struct DocumentScreen: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @StateObject private var document: DocumentText
    @State private var editing = false
    @StateObject private var field = PlainField()

    init(vault: Vault, id: String) {
        _document = StateObject(wrappedValue: DocumentText(vault: vault, id: id))
    }

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var path: String? { document.path }

    var body: some View {
        VStack(spacing: 0) {
            bar
            if let unreachable = document.unreachable {
                Label(unreachable, systemImage: "wifi.slash")
                    .font(.ui(Metric.small)).foregroundStyle(flavour(.subtext0))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, Metric.pad)
                    .padding(.bottom, Metric.gap)
            }
            if editing {
                PlainText(field: field, text: $document.text, font: .monospacedSystemFont(ofSize: Metric.label, weight: .regular))
                    .padding(Metric.gap)
                    .onAppear { DispatchQueue.main.async { field.focus() } }
            } else if document.markdown {
                ScrollView {
                    Markdown(text: document.text)
                        .textSelection(.enabled)
                        .padding(Metric.wide)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                ScrollView([.horizontal, .vertical]) {
                    Text(document.text)
                        .font(.system(size: Metric.caption, design: .monospaced))
                        .foregroundStyle(flavour(.text))
                        .textSelection(.enabled)
                        .padding(Metric.pad)
                }
            }
        }
        .background(flavour(.base))
        .environment(\.openURL, OpenURLAction { url in library.follow(url) ? .handled : .systemAction })
        .task { await document.load() }
        .alert("The file changed since Sesh read it", isPresented: Binding(get: { document.clash != nil }, set: { if !$0 { document.clash = nil } })) {
            Button("Keep my text", role: .destructive) { Task { await document.write(force: true) } }
            Button("Take the file's text") { document.takeClash() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Someone, probably an Agent, rewrote \(path ?? "it") while you were editing. Keeping your text overwrites theirs.")
        }
    }

    private var bar: some View {
        HStack(spacing: Metric.gap) {
            Text(path ?? "In the Vault").font(.ui(Metric.small)).foregroundStyle(flavour(.subtext0))
                .lineLimit(1).truncationMode(.head)
            Spacer()
            if document.saving { ProgressView().controlSize(.small) }
            Picker("Mode", selection: $editing) {
                Text("Read").tag(false)
                Text("Edit").tag(true)
            }
            .pickerStyle(.segmented)
            .fixedSize()
            .disabled(!document.editable)
            Button("Save") { Task { await document.write(force: false) } }
                .font(.ui(Metric.label))
                .disabled(!document.dirty || document.saving)
        }
        .padding(.horizontal, Metric.pad)
        .padding(.vertical, Metric.gap)
    }
}
