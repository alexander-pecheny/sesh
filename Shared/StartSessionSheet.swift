import SwiftUI

/// What a new Agent session works on: a repository used lately, another one, or none. A
/// repository gets the Task's Worktree on first use, on the branch named for the Task, or a new
/// Worktree of its own for a session working in parallel.
struct StartSessionSheet: View {
    /// The sheet for `agent`: a fresh one, or one shown again as it was filled in when its start failed.
    struct Shown: Identifiable {
        let id = UUID()
        let agent: Agent
        var draft: Draft?
        var problem: String?
    }

    struct Draft {
        var alias: String?
        var choice, typed, config: String
        var remote, bypass, parallel: Bool
        var newBranch: String
    }

    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    let shown: Shown
    let failed: (Shown) -> Void
    @State private var alias: String?
    @State private var choice = Self.other
    @State private var typed = ""
    @State private var config = ""
    @State private var remote = true
    @State private var bypass = true
    @State private var parallel = false
    @State private var newBranch = ""
    /// What the new Worktree branches from, as the repository's branches say.
    @State private var base: String?
    @State private var home: String?
    @FocusState private var focused: Bool
    /// The folder a link is cloned into, once the user changed it.
    @State private var into: String?
    @State private var picking: Picking?
    @State private var cloning: (phase: String, done: Double?)?
    @State private var cloneProblem: String?
    @State private var clone: Task<Void, Never>?

    private enum Picking: String, Identifiable {
        case repo, parent
        var id: String { rawValue }
    }

    private static let other = "\u{0}other"
    private static let none = "\u{0}none"

    private var agent: Agent { shown.agent }
    private var machines: [Machine] { TaskActions.machines(for: task, in: vault) }
    private var recents: [String] { TaskActions.recentRepos(for: vault.records[task.id] ?? task, on: alias, home: home, in: vault, library: library) }
    private var path: String { typed.trimmingCharacters(in: .whitespacesAndNewlines) }
    /// The repository's name, when what was typed is a link to clone.
    private var linked: String? { choice == Self.other ? Start.cloneName(of: path) : nil }
    private var destination: String { into ?? "\(Start.parent(of: recents))/\(linked ?? "")" }
    private var repo: String? {
        switch choice {
        case Self.other: path.isEmpty ? nil : path
        case Self.none: nil
        default: choice
        }
    }
    private var ready: Bool { (choice != Self.other || !path.isEmpty) && fresh?.isEmpty != true && cloning == nil }
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
            if let problem = shown.problem {
                Section("Sesh could not start it") {
                    Text(problem).font(.callout.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                }
            }
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
                    HStack {
                        TextField("Path", text: $typed, prompt: Text("~/src/project, or a git link to clone"))
                            .labelsHidden()
                            .multilineTextAlignment(.leading)
                            .focused($focused)
                            .onSubmit(go)
                        Button("Browse", systemImage: "folder") { picking = .repo }.labelStyle(.iconOnly)
                    }
                    if linked != nil {
                        LabeledContent("Into") {
                            HStack {
                                TextField("Into", text: Binding(get: { destination }, set: { into = $0 })).labelsHidden().onSubmit(go)
                                Button("Browse", systemImage: "folder") { picking = .parent }.labelStyle(.iconOnly)
                            }
                        }
                    }
                    if let cloning {
                        ProgressView(value: cloning.done) { Text(cloning.phase).font(.caption) }
                    }
                    if let cloneProblem {
                        Text(cloneProblem).font(.caption.monospaced()).foregroundStyle(.red).textSelection(.enabled)
                    }
                }
                if agent == .claude {
                    TextField("Claude config folder", text: $config, prompt: Text("~/.claude"))
                    Toggle("Remote control", isOn: $remote)
                }
                if agent.asks { Toggle("Skip permissions", isOn: $bypass) }
            } footer: {
                if linked != nil, let machine {
                    Text("git clone runs on \(machine.title) with the credentials it has there.")
                } else if choice == Self.other {
                    Text("A folder that does not exist yet becomes a new repository.")
                }
            }
            if repo != nil {
                Section {
                    Picker("Worktree", selection: $parallel) {
                        Text("The Task's (\(main))").tag(false)
                        Text("A new one").tag(true)
                    }
                    if parallel {
                        LabeledContent("Branch") {
                            TextField("Branch", text: $newBranch).labelsHidden().multilineTextAlignment(.trailing).onSubmit(go)
                        }
                    }
                } footer: {
                    Text(parallel
                        ? "Branches from \(base ?? "what the repository has checked out"). The Task's Worktree stays as it is."
                        : "A Worktree on \(main) is made in the repository the first time.")
                }
            }
        }
        .onAppear {
            if let draft = shown.draft {
                (alias, choice, typed, config) = (draft.alias, draft.choice, draft.typed, draft.config)
                (remote, bypass, parallel, newBranch) = (draft.remote, draft.bypass, draft.parallel, draft.newBranch)
                return
            }
            alias = TaskActions.lastMachine(for: vault.records[task.id] ?? task, in: vault, library: library)?.alias
            remote = vault.settings.body.remote ?? true
            bypass = vault.settings.body.bypass ?? true
            choose()
        }
        .task(id: alias) {
            home = nil
            choose()
            guard let machine else { return }
            let found = (await machine.run("printf %s \"$HOME\"")).out
            guard !Task.isCancelled, !found.isEmpty else { return }
            home = found
            choose()
        }
        .task(id: "\(alias ?? "")|\(repo ?? "")|\(parallel)") { await suggest() }
        .onChange(of: linked) { into = nil }
        .onDisappear { clone?.cancel() }
        .sheet(item: $picking) { picking in
            if let machine {
                let start = picking == .parent ? (destination as NSString).deletingLastPathComponent : (path.isEmpty ? Start.parent(of: recents) : path)
                FolderPicker(machine: machine, start: start, home: home) { folder in
                    switch picking {
                    case .repo: typed = folder
                    case .parent: into = "\(folder)/\(linked ?? "")"
                    }
                }
            }
        }
    }

    @ToolbarContentBuilder private var buttons: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
        ToolbarItem(placement: .confirmationAction) { Button(linked == nil ? "Start" : "Clone and Start", action: go).disabled(!ready) }
    }

    private var machine: Machine? { machines.first { $0.alias == alias } ?? machines.first }

    private func choose() {
        if let draft = shown.draft, draft.alias == alias { return }
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
        base = Start.base(after: main, in: taken)
    }

    private func go() {
        if linked == nil { start() } else { cloneThenStart() }
    }

    /// Clones the link into its folder, showing git's progress, and starts there once it is in;
    /// a failure stays in the sheet with git's own words.
    private func cloneThenStart() {
        guard ready, let machine else { return }
        let (link, folder) = (path, destination)
        let target = shellPath(folder)
        let command = "mkdir -p \"$(dirname \(target))\" && { git clone --progress \(quote(link)) \(target) 2>&1; echo \"\(Self.exit)$?\"; } | tr '\\r' '\\n'"
        cloning = ("Cloning", nil)
        cloneProblem = nil
        clone = Task {
            let said = Said()
            let ran = await machine.fetch(command) { line in
                if line.hasPrefix(Self.exit) { return said.status = Int(line.dropFirst(Self.exit.count)) }
                said.tail = Array((said.tail + [line]).suffix(6))
                if let progress = Start.cloning(line) { cloning = progress }
            }
            cloning = nil
            guard !Task.isCancelled else { return }
            guard said.status == 0 else {
                return cloneProblem = said.tail.isEmpty ? ran.problem : said.tail.joined(separator: "\n")
            }
            typed = folder
            into = nil
            start()
        }
    }

    private static let exit = "sesh-clone-exit:"

    /// What `git clone` said, gathered as it streams.
    @MainActor
    private final class Said {
        var status: Int?
        var tail: [String] = []
    }

    private func start() {
        guard ready, let machine else { return }
        if agent == .claude { TaskActions.setClaudeConfig(config.trimmingCharacters(in: .whitespaces), vault, on: machine) }
        let draft = Draft(alias: alias, choice: choice, typed: typed, config: config, remote: remote, bypass: bypass,
                          parallel: parallel, newBranch: newBranch)
        library.start(agent, for: vault.records[task.id] ?? task, on: machine, repo: repo, branch: fresh, remote: remote,
                      bypass: bypass) { [shown, failed] in
            failed(Shown(agent: shown.agent, draft: draft, problem: $0))
        }
        dismiss()
    }
}
