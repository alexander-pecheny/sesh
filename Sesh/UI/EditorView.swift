import PhotosUI
import SwiftUI

struct EditorView: View {
    @EnvironmentObject private var store: Store
    @ObservedObject var session: SeshSession
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    @State private var editing: Draft?
    let send: (String, Bool) -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        NavigationStack {
            List {
                ForEach(store.drafts) { draft in
                    Button { editing = draft } label: {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(draft.title).font(.ui(15)).foregroundStyle(flavour(.text))
                            Text(draft.edited.formatted(date: .abbreviated, time: .shortened))
                                .font(.ui(11)).foregroundStyle(flavour(.subtext0))
                        }
                    }
                    .listRowBackground(flavour(.mantle))
                }
                .onDelete { store.drafts.remove(atOffsets: $0) }
            }
            .scrollContentBackground(.hidden)
            .background(flavour(.base))
            .overlay { if store.drafts.isEmpty { Text("No Drafts yet").font(.ui(15)) } }
            .navigationTitle("Editor")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { editing = Draft() } label: { Label("New Draft", image: "plus") }
                }
            }
            .navigationDestination(item: $editing) { draft in
                DraftView(draft: draft, session: session) { saved, enter in
                    store.upsert(saved)
                    guard let enter else { return }
                    send(saved.text, enter)
                    dismiss()
                }
            }
        }
        .tint(flavour(.mauve))
    }
}

private struct DraftView: View {
    @Environment(\.dismiss) private var dismiss
    @State var draft: Draft
    @ObservedObject var session: SeshSession
    /// `nil` means save; otherwise send, with or without a trailing Enter.
    let finish: (Draft, Bool?) -> Void

    @StateObject private var field = DraftField()
    @State private var picking = false

    var body: some View {
        DraftText(field: field, text: $draft.text)
            .padding(8)
            .onChange(of: draft.text) { finish(draft, nil) }
            .navigationTitle("Draft")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button { picking = true } label: {
                        if let fraction = session.uploading?.fraction {
                            UploadRing(fraction: fraction, colour: .accentColor)
                        } else {
                            Label("upload", image: "image-up")
                        }
                    }
                    .disabled(session.uploading == nil && !session.canUpload)
                    Button("Send") { finish(draft, false) }
                    // Short, so the upload button fits beside it rather than in an overflow.
                    Button("Send \u{23CE}") { finish(draft, true) }
                }
            }
            .sheet(isPresented: $picking) {
                PhotoPicker { results in
                    picking = false
                    session.upload(results) { field.insertPaths($0) }
                }
                .ignoresSafeArea()
            }
            .uploadFailure($session.uploadError)
    }
}

/// Unlike TextEditor, keeps the caret in sight when a taller keyboard shrinks it or an Upload path lands.
private final class DraftField: UITextView, ObservableObject {
    private var height = 0.0

    override func layoutSubviews() {
        super.layoutSubviews()
        if bounds.height < height, isFirstResponder { scrollRangeToVisible(selectedRange) }
        height = bounds.height
    }

    /// At the caret, over any selection, and never glued to the word in front of it.
    func insertPaths(_ paths: String) {
        let before = (text as NSString).substring(to: selectedRange.location)
        let gap = before.last.map { $0.isWhitespace } ?? true
        insertText((gap ? "" : " ") + paths)
    }
}

private struct DraftText: UIViewRepresentable {
    let field: DraftField
    @Binding var text: String

    func makeUIView(context: Context) -> DraftField {
        field.font = .preferredFont(forTextStyle: .body)
        field.adjustsFontForContentSizeCategory = true
        field.autocorrectionType = .no
        field.autocapitalizationType = .none
        field.backgroundColor = .clear
        field.delegate = context.coordinator
        return field
    }

    func updateUIView(_ field: DraftField, context: Context) {
        if field.text != text { field.text = text }
    }

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, UITextViewDelegate {
        let text: Binding<String>
        init(text: Binding<String>) { self.text = text }
        func textViewDidChange(_ field: UITextView) { text.wrappedValue = field.text }
    }
}
