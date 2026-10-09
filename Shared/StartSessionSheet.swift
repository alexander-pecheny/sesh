import SwiftUI

/// What a new Agent session works on: a repository used lately, another one, or none. A
/// repository gets the Task's Worktree on first use, on the branch named for the Task.
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
    private var ready: Bool { choice != Self.other || !path.isEmpty }
    private var branch: String? { vault.records[task.id]?.body.branch }

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
                if choice == Self.other {
                    Text("A folder that does not exist yet becomes a new repository.")
                } else if repo != nil {
                    Text("A Worktree on \(branch ?? "the Task's branch") is made in it the first time.")
                }
            }
        }
        .onAppear {
            alias = machines.first?.alias
            choose()
        }
        .onChange(of: alias) { choose() }
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

    private func start() {
        guard ready, let machine else { return }
        if agent == .claude { TaskActions.setClaudeConfig(config.trimmingCharacters(in: .whitespaces), vault, on: machine) }
        library.start(agent, for: vault.records[task.id] ?? task, on: machine, repo: repo, problem: problem)
        dismiss()
    }
}
