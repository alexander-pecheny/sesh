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
    /// `nil` means save and go back; otherwise send, with or without a trailing Enter.
    let finish: (Draft, Bool?) -> Void

    @State private var selection: TextSelection?
    @State private var picking = false

    var body: some View {
        TextEditor(text: $draft.text, selection: $selection)
            .font(.body)
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            .padding(8)
            .navigationTitle("Draft")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        finish(draft, nil)
                        dismiss()
                    }
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
                    session.upload(results) { insert($0) }
                }
                .ignoresSafeArea()
            }
            .uploadFailure($session.uploadError)
    }

    /// Ordinary paste behaviour: at the caret, over any selection, and never glued to the
    /// word in front of it.
    private func insert(_ paths: String) {
        let end = draft.text.endIndex
        var range = end..<end
        if case .selection(let selected)? = selection?.indices { range = selected }
        let gap = draft.text[..<range.lowerBound].last.map { $0.isWhitespace } ?? true
        let text = (gap ? "" : " ") + paths
        draft.text.replaceSubrange(range, with: text)
        let caret = draft.text.index(range.lowerBound, offsetBy: text.count)
        selection = TextSelection(insertionPoint: caret)
    }
}
