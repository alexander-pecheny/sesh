import SwiftUI

/// What a new Agent session works on: a repository used lately, another one, or none. A
/// repository gets the Task's Worktree on first use, on the branch named for the Task, or a new
/// Worktree of its own for a session working in parallel.
struct StartSessionSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    let agent: Agent
    let problem: (String) -> Void
    @State private var alias: String?
    @State private var choice = Self.other
    @State private var typed = ""
    @State private var config = ""
    @State private var parallel = false
    @State private var newBranch = ""
    @FocusState private var focused: Bool

    private static let other = "\u{0}other"
    private static let none = "\u{0}none"

    private var machines: [Machine] { TaskActions.machines(for: task, in: vault) }
    private var recents: [String] { TaskActions.recentRepos(for: vault.records[task.id] ?? task, on: alias, library: library) }
    private var path: String { typed.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var repo: String? {
        switch choice {
        case Self.other: path.isEmpty ? nil : path
        case Self.none: nil
        default: choice
        }
    }
    private var ready: Bool { (choice != Self.other || !path.isEmpty) && fresh?.isEmpty != true }
    private var main: String { machine.map { TaskActions.branch(of: vault.records[task.id] ?? task, on: $0) } ?? "" }
    /// The branch of the session's own new Worktree, when it gets one.
    private var fresh: String? { parallel && repo != nil ? newBranch.trimmingCharacters(in: .whitespaces) : nil }

    var body: some View {
        #if os(macOS)
        form
            .formStyle(.grouped)
            .frame(width: 460)
            .toolbar { buttons }
        #else
        NavigationStack {
            form
                .navigationTitle("New \(agent.title) session")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { buttons }
        }
        #endif
    }

    private var form: some View {
        Form {
            if machines.count > 1 {
                Picker("Machine", selection: $alias) {
                    ForEach(machines) { Text($0.title).tag($0.alias) }
                }
            }
            Section {
                Picker("Repository", selection: $choice) {
                    ForEach(recents, id: \.self) { Text($0).tag($0) }
                    Text("Another repository…").tag(Self.other)
                    if task.body.path == nil { Text("None, in the home folder").tag(Self.none) }
                }
                if choice == Self.other {
                    TextField("Path", text: $typed, prompt: Text("~/src/project, its main checkout"))
                        .labelsHidden()
                        .multilineTextAlignment(.leading)
                        .focused($focused)
                        .onSubmit(start)
                }
                if agent == .claude {
                    TextField("Claude config folder", text: $config, prompt: Text("~/.claude"))
                }
            } footer: {
                if choice == Self.other { Text("A folder that does not exist yet becomes a new repository.") }
            }
            if repo != nil {
                Section {
                    Picker("Worktree", selection: $parallel) {
                        Text("The Task's (\(main))").tag(false)
                        Text("A new one").tag(true)
                    }
                    if parallel {
                        LabeledContent("Branch") {
                            TextField("Branch", text: $newBranch).labelsHidden().multilineTextAlignment(.trailing).onSubmit(start)
                        }
                    }
                } footer: {
                    Text(parallel
                        ? "This session works in a Worktree of its own; the Task's stays as it is."
                        : "A Worktree on \(main) is made in the repository the first time.")
                }
            }
        }
        .onAppear {
            alias = machines.first?.alias
            choose()
        }
        .onChange(of: alias) { choose() }
        .task(id: "\(alias ?? "")|\(repo ?? "")|\(parallel)") { await suggest() }
    }

    @ToolbarContentBuilder private var buttons: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
        ToolbarItem(placement: .confirmationAction) { Button("Start", action: start).disabled(!ready) }
    }

    private var machine: Machine? { machines.first { $0.alias == alias } ?? machines.first }

    private func choose() {
        config = machine.flatMap { TaskActions.claudeConfig(vault, on: $0) } ?? ""
        choice = recents.first ?? Self.other
        focused = choice == Self.other
    }

    /// Offers the Task's branch with the next number no branch of the repository has yet.
    private func suggest() async {
        guard parallel, let repo, let machine else { return }
        let taken = await Git.branches(of: repo, on: machine)
        guard !Task.isCancelled else { return }
        newBranch = Start.nextBranch(after: main, taken: taken)
    }

    private func start() {
        guard ready, let machine else { return }
        if agent == .claude { TaskActions.setClaudeConfig(config.trimmingCharacters(in: .whitespaces), vault, on: machine) }
        library.start(agent, for: vault.records[task.id] ?? task, on: machine, repo: repo,
                      branch: fresh, problem: problem)
        dismiss()
    }
}
