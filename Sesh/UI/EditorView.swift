import SwiftUI

struct EditorView: View {
    @EnvironmentObject private var store: Store
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
                DraftView(draft: draft) { saved, enter in
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
    /// `nil` means save and go back; otherwise send, with or without a trailing Enter.
    let finish: (Draft, Bool?) -> Void

    var body: some View {
        TextEditor(text: $draft.text)
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
                    Button("Send") { finish(draft, false) }
                    Button("Send + Enter") { finish(draft, true) }
                }
            }
    }
}
