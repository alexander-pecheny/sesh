import SwiftUI

/// Tasks on the left, the chosen Task's Tabs or the search results on the right.
struct RootView: View {
    @EnvironmentObject private var library: Library
    @StateObject private var search = Search()

    var body: some View {
        NavigationSplitView {
            Sidebar()
                .navigationSplitViewColumnWidth(min: 200, ideal: 260, max: 400)
        } detail: {
            if !search.query.trimmingCharacters(in: .whitespaces).isEmpty {
                SearchView(search: search)
            } else if let id = library.selection, let vault = library.vault(of: id) {
                TaskView(vault: vault, id: id).id(id)
            } else {
                Text(library.vaults.isEmpty ? "Add a Vault to keep Tasks in." : "Pick a Task")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .searchable(text: $search.query, placement: .toolbar, prompt: "Tasks, Journals, Documents, Conversations")
        .onAppear { search.library = library }
    }
}
