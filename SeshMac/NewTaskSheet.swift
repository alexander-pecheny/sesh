import SwiftUI

/// A title in plain words, and optionally a new Worktree with a branch suggested from it.
struct NewTaskSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let parent: String?
    @State private var title = ""
    @State private var worktree = true
    @State private var alias: String?
    @State private var repo = ""
    @State private var branch = ""
    @State private var branchEdited = false
    @State private var working = false
    @State private var problem: String?
    /// Kept so that a retry after a failed Worktree does not make a second Task.
    @State private var made: Record?

    private var machines: [Machine] { vault.place.alias == nil ? [.mac] : [vault.machine, .mac] }
    private var trimmed: String { title.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var recentKey: String { "repo.\(alias ?? "")" }

    var body: some View {
        Form {
            TextField("Title", text: $title, prompt: Text("What is this Task about, in plain words?"))
            Toggle("Work in a new Worktree", isOn: $worktree)
            if worktree {
                Picker("Machine", selection: $alias) {
                    ForEach(machines) { Text($0.title).tag($0.alias) }
                }
                TextField("Repository", text: $repo, prompt: Text("~/src/project, its main checkout"))
                TextField("Branch", text: Binding(get: { branch }, set: { branch = $0; branchEdited = true }))
            }
            if let problem {
                Text(problem).font(.caption).foregroundStyle(.red).textSelection(.enabled)
            }
        }
        .formStyle(.grouped)
        .frame(width: 460)
        .disabled(working)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            ToolbarItem(placement: .confirmationAction) {
                if working { ProgressView().controlSize(.small) } else {
                    Button("Create") { Task { await create() } }
                        .disabled(trimmed.isEmpty || (worktree && (repo.isEmpty || branch.isEmpty)))
                }
            }
        }
        .onAppear {
            alias = vault.place.alias
            repo = UserDefaults.standard.string(forKey: recentKey) ?? ""
        }
        .onChange(of: alias) { repo = UserDefaults.standard.string(forKey: recentKey) ?? repo }
        .onChange(of: title) { if !branchEdited { branch = TaskActions.branch(for: title) } }
    }

    private func create() async {
        working = true
        defer { working = false }
        let task = made ?? vault.create(.task, .init(title: trimmed, parent: parent, position: Tree.next(in: parent, of: vault)))
        made = task
        library.selection = task.id
        guard worktree else { return dismiss() }
        UserDefaults.standard.set(repo, forKey: recentKey)
        let machine = TaskActions.machine(alias)
        let folder = repo.hasPrefix("~/") ? (await machine.run("printf %s \"$HOME\"")).out + repo.dropFirst(1) : repo
        if let failed = await TaskActions.makeWorktree(for: task, repo: folder, branch: branch, on: machine, in: vault) {
            problem = "The Task is made, but its Worktree is not: \(failed)"
            return
        }
        dismiss()
    }
}
