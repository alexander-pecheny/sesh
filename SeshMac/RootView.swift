import SwiftUI

/// Tasks on the left, the chosen Task's Tabs or the search results on the right.
struct RootView: View {
    @EnvironmentObject private var library: Library
    @StateObject private var search = Search()
    @FocusState private var searching: Bool

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
        .searchFocused($searching)
        .onSubmit(of: .search) { search.openFirst = true }
        .onReceive(NotificationCenter.default.publisher(for: .searchAll)) { _ in searching = true }
        // A Task picked in the sidebar is what the user wants to see, not the results.
        .onChange(of: library.selection) { if library.selection != nil { search.query = "" } }
        .onAppear { search.library = library }
        .reportsCleanup(library)
        #if DEBUG
        // `-newtask TITLE` makes a Task in the first Vault, as the New Task sheet does.
        .task {
            guard let title = UserDefaults.standard.string(forKey: "newtask") else { return }
            for _ in 0..<40 where library.vaults.first?.online != true { try? await Task.sleep(for: .milliseconds(500)) }
            if let vault = library.vaults.first { library.newTask(title, in: vault, parent: nil) }
        }
        #endif
    }
}
