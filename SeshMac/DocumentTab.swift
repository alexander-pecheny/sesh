import SwiftUI

/// A Document of the Task: Markdown kept in the Vault, or a file on a machine that the Vault
/// remembers the last text of. Edits to a file go to the file itself (ADR 0008).
struct DocumentTab: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @ObservedObject var document: DocumentText
    @State private var editing = false
    @StateObject private var field = PlainField()

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var path: String? { document.path }

    var body: some View {
        VStack(spacing: 0) {
            bar
            Divider()
            if let unreachable = document.unreachable {
                Label(unreachable, systemImage: "wifi.slash")
                    .font(.caption).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(Metric.gap)
            }
            if editing {
                PlainText(field: field, text: $document.text, font: .monospacedSystemFont(ofSize: Metric.label, weight: .regular))
                    .padding(Metric.pad)
                    .onAppear { DispatchQueue.main.async { field.focus() } }
            } else {
                if !document.markdown {
                    CodeView(text: document.text, language: path.map { ($0 as NSString).pathExtension })
                } else {
                ScrollView {
                    Group {
                        Prose(text: document.text)
                    }
                    .textSelection(.enabled)
                    .padding(Metric.wide)
                    .frame(maxWidth: 820, alignment: .leading)
                    .frame(maxWidth: .infinity)
                }
                }
            }
        }
        .background(flavour(.base), ignoresSafeAreaEdges: .vertical)
        .environment(\.openURL, OpenURLAction { url in library.follow(url) ? .handled : .systemAction })
        .task {
            // Coming back to unsaved edits keeps them, in the editor; otherwise the text is read afresh.
            if document.dirty { editing = true } else { await document.load() }
            if document.text.isEmpty, document.editable { editing = true }
        }
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
            Text(path ?? "In the Vault").font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.head)
            Spacer()
            if document.saving { ProgressView().controlSize(.small) }
            if document.dirty { Text("Edited").font(.caption).foregroundStyle(.secondary) }
            Picker("", selection: $editing) {
                Text("Read").tag(false)
                Text("Edit").tag(true)
            }
            .pickerStyle(.segmented)
            .fixedSize()
            .disabled(!document.editable)
            .help("Read or edit (⇧⌘E)")
            .background {
                Button("") { if document.editable { editing.toggle() } }.keyboardShortcut("e", modifiers: [.command, .shift]).hidden()
            }
            Button("Save") { Task { await document.write(force: false) } }
                .keyboardShortcut("s")
                .disabled(!document.dirty || document.saving)
        }
        .padding(.horizontal, Metric.pad)
        .frame(height: 32)
    }
}

/// A source file, coloured and selectable, at its own width: code is not reflowed.
private struct CodeView: NSViewRepresentable {
    @Environment(\.colorScheme) private var colorScheme
    let text: String
    let language: String?

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        scroll.drawsBackground = false
        scroll.hasHorizontalScroller = true
        guard let view = scroll.documentView as? NSTextView else { return scroll }
        view.isEditable = false
        view.drawsBackground = false
        view.usesFindBar = true
        view.isIncrementalSearchingEnabled = true
        view.textContainerInset = NSSize(width: Metric.wide, height: Metric.pad)
        view.isHorizontallyResizable = true
        view.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        view.textContainer?.widthTracksTextView = false
        view.textContainer?.containerSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let view = scroll.documentView as? NSTextView, context.coordinator.shown != "\(colorScheme)\(text)" else { return }
        context.coordinator.shown = "\(colorScheme)\(text)"
        let dark = colorScheme == .dark
        let font = NSFont.monospacedSystemFont(ofSize: Metric.note, weight: .regular)
        let plain = NSAttributedString(string: text, attributes: [
            .font: font, .foregroundColor: NSColor((dark ? Catppuccin.Flavour.mocha : .latte)(.text)),
        ])
        view.textStorage?.setAttributedString(Highlight.code(text, language: language, dark: dark, size: Metric.note) ?? plain)
        view.sizeToFit()
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    /// What the view shows, so it is coloured once rather than on every update.
    final class Coordinator {
        var shown = ""
    }
}
