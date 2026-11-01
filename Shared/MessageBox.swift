import SwiftUI

/// What is typed in a Conversation's message box and not sent, observed apart from the
/// Conversation so that typing redraws the box alone.
@MainActor
final class Outbox: ObservableObject {
    @Published var text = "" { didSet { if text != oldValue { save?(text) } } }
    /// Messages on their way to the follower.
    @Published var sending = 0
    var save: ((String) -> Void)?

    var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
}

/// The message box under a Conversation, with its send or stop button.
struct MessageBox: View {
    @Environment(\.flavour) private var flavour
    @ObservedObject var conversation: Conversation
    @ObservedObject var outbox: Outbox
    let fresh: Bool
    let hidden: Bool
    @StateObject private var field = PlainField()
    @State private var picking = false
    @State private var composing = false

    private var stops: Bool { conversation.stops }

    var body: some View {
        HStack(alignment: .bottom, spacing: Metric.gap) {
            #if os(iOS)
            if let link = conversation.runner?.uploads {
                UploadButton(link: link) { picking = true }
            } else {
                Image.lucide("image-up", size: 20).foregroundStyle(flavour(.overlay0))
                    .frame(width: Metric.control, height: Metric.control)
            }
            #endif
            PlainText(field: field, text: $outbox.text, font: .systemFont(ofSize: Metric.title), lines: 6) { Task { await conversation.sendDraft() } }
                .overlay(alignment: .leading) {
                    if outbox.text.isEmpty {
                        Text("Message").font(.ui(Metric.title)).foregroundStyle(flavour(.overlay0)).allowsHitTesting(false)
                    }
                }
                .padding(.leading, Metric.pad)
                .padding(.trailing, Metric.control)
                .padding(.vertical, Metric.gap)
                .frame(minHeight: Metric.control)
                .overlay(alignment: .bottomTrailing) {
                    Button { composing = true } label: {
                        Image.lucide("maximize-2", size: Metric.label).foregroundStyle(flavour(.overlay1))
                            .frame(width: Metric.control, height: Metric.control)
                    }
                    .accessibilityLabel("Expand")
                }
                .background(flavour(.base), in: .rect(cornerRadius: Metric.control / 2))
                .foregroundStyle(flavour(.text))
            Button { Task { await conversation.sendOrStop() } } label: {
                Image.lucide(stops ? "square" : "arrow-up", size: 18)
                    .foregroundStyle(flavour(.base))
                    .frame(width: Metric.control, height: Metric.control)
                    .background(flavour(stops ? .red : .mauve).opacity(!outbox.empty || stops ? 1 : 0.4), in: .circle)
            }
            .disabled(outbox.empty && !stops)
            .accessibilityLabel(stops ? "Stop" : "Send")
            .help(stops ? "Stop the Agent" : "Send")
        }
        .padding(.horizontal, Metric.pad)
        .padding(.vertical, Metric.gap)
        .background(flavour(.mantle), ignoresSafeAreaEdges: .vertical)
        #if os(iOS)
        .sheet(isPresented: $picking) {
            PhotoPicker { results in
                picking = false
                conversation.runner?.uploads?.upload(results) { field.insertPaths($0) }
            }
            .ignoresSafeArea()
        }
        #endif
        // A send from the full-screen editor jumps while the chat is covered, by sizes that
        // change as it closes; the jump is made again once it has.
        .onChange(of: composing) { if !composing, conversation.scroll.atBottom { conversation.scroll.jump() } }
        .cover(isPresented: $composing) {
            #if os(iOS)
            Composer(text: $outbox.text, canSend: !outbox.empty, uploads: conversation.runner?.uploads) { Task { await conversation.sendDraft() } }
            #else
            Composer(text: $outbox.text, canSend: !outbox.empty) { Task { await conversation.sendDraft() } }
            #endif
        }
        .onAppear(perform: appeared)
        #if os(macOS)
        .onChange(of: hidden) { field.catchesTyping = !hidden }
        #endif
    }

    private func appeared() {
        if fresh { DispatchQueue.main.async { field.focus() } }
        let conversation = conversation
        field.recall = { [weak conversation] in
            conversation?.rows.flatMap(\.entries).last { $0.kind == "user" && $0.text?.isEmpty == false }?.text
        }
        #if os(macOS)
        field.pasteImage = { [weak conversation] data, ext in
            let path = await conversation?.upload(data, ext: ext)
            if path == nil { conversation?.problem = "Sesh could not upload the pasted image." }
            return path
        }
        field.escape = { [weak conversation] in
            guard let conversation, conversation.state == "working" else { return }
            Task { await conversation.stop() }
        }
        field.catchesTyping = !hidden
        #endif
    }
}

/// The whole screen for a long message, on the same text as the message box.
private struct Composer: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.flavour) private var flavour
    @Binding var text: String
    let canSend: Bool
    #if os(iOS)
    let uploads: HostLink?
    #endif
    let send: () -> Void
    @StateObject private var field = PlainField()
    @State private var picking = false

    var body: some View {
        NavigationStack {
            PlainText(field: field, text: $text)
                .padding(Metric.gap)
                #if os(macOS)
                .frame(minWidth: 640, idealWidth: 760, minHeight: 420, idealHeight: 560)
                #endif
                .background(flavour(.base))
                .navigationTitle("Message")
                .inlineTitle()
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                    #if os(iOS)
                    if let uploads {
                        ToolbarItem(placement: .primaryAction) { UploadButton(link: uploads) { picking = true } }
                    }
                    #endif
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Send") {
                            send()
                            dismiss()
                        }
                        .disabled(!canSend)
                    }
                }
                .onAppear { field.focus() }
                #if os(iOS)
                .sheet(isPresented: $picking) {
                    PhotoPicker { results in
                        picking = false
                        uploads?.upload(results) { field.insertPaths($0) }
                    }
                    .ignoresSafeArea()
                }
                #endif
        }
        .tint(flavour(.mauve))
    }
}

#if os(iOS)
private struct UploadButton: View {
    @Environment(\.flavour) private var flavour
    @ObservedObject var link: HostLink
    let pick: () -> Void

    var body: some View {
        Button {
            if link.uploading != nil { link.cancelUpload() } else { pick() }
        } label: {
            Group {
                if let fraction = link.uploading?.fraction {
                    UploadRing(fraction: fraction, colour: flavour(.mauve))
                } else {
                    Image.lucide("image-up", size: 20)
                }
            }
            .foregroundStyle(flavour(.subtext0))
            .frame(width: 38, height: 38)
        }
        .disabled(link.uploading == nil && !link.canUpload)
        .accessibilityLabel(link.uploading == nil ? "upload" : "cancel upload")
        .uploadFailure($link.uploadError)
    }
}
#endif
