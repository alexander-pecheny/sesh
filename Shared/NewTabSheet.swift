import SwiftUI

/// A new Tab of a Task: an Agent session, a Terminal or a Document, and where it opens. An Agent
/// session works on a repository used lately, another one, or none. A repository gets the Task's
/// Worktree on first use, on the branch named for the Task, or a new Worktree of its own for a
/// session working in parallel.
struct NewTabSheet: View {
    /// The sheet: a fresh one, or one shown again as it was filled in when its start failed.
    struct Shown: Identifiable {
        let id = UUID()
        var draft: Draft?
        var problem: String?
    }

    struct Draft {
        var tab: NewTab
        var agent: Agent
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
    let terminal: (Machine) -> Void
    @State private var tab = NewTab.session
    @State private var agent = Agent.claude
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

    private var current: Record { vault.records[task.id] ?? task }
    /// Whether the Agent session asks where it works; a Scratchpad's starts in the home folder.
    private var asksFolder: Bool { tab == .session && !task.scratchpad }
    private var machines: [Machine] { TaskActions.machines(for: task, in: vault) }
    private var recents: [String] { TaskActions.recentRepos(for: current, on: alias, home: home, in: vault, library: library) }
    private var path: String { typed.trimmingCharacters(in: .whitespacesAndNewlines) }
    /// The repository's name, when what was typed is a link to clone.
    private var linked: String? { choice == Self.other ? Start.cloneName(of: path) : nil }
    private var destination: String { into ?? "\(Start.parent(of: recents))/\(linked ?? "")" }
    private var repo: String? {
        switch choice {
        case _ where !asksFolder: nil
        case Self.other: path.isEmpty ? nil : path
        case Self.none: nil
        default: choice
        }
    }
    private var ready: Bool { !asksFolder || (choice != Self.other || !path.isEmpty) && fresh?.isEmpty != true && cloning == nil }
    private var main: String { machine.map { TaskActions.branch(of: current, on: $0) } ?? "" }
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
                .navigationTitle("New Tab")
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
            Section {
                Picker("New", selection: $tab) {
                    ForEach(NewTab.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                if tab == .session {
                    Picker("Agent", selection: $agent) {
                        ForEach(Agent.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented)
                }
                if tab != .document, machines.count > 1 {
                    Picker("Machine", selection: $alias) {
                        ForEach(machines) { Text($0.title).tag($0.alias) }
                    }
                }
            }
            if asksFolder { folder }
            if tab == .session {
                Section {
                    if agent == .claude {
                        TextField("Claude config folder", text: $config, prompt: Text("~/.claude"))
                        Toggle("Remote control", isOn: $remote)
                    }
                    if agent.asks { Toggle("Skip permissions", isOn: $bypass) }
                }
            }
            if !reopen.sections.isEmpty {
                Section {
                    Menu("Reopen") {
                        ForEach(reopen.sections, id: \.self) { section in
                            Section(section.title) {
                                ForEach(section.items, id: \.self) { item in
                                    Button(item.title) {
                                        library.open(item.tab, in: task.id)
                                        dismiss()
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        .onAppear {
            if let draft = shown.draft {
                (tab, agent, alias, choice, typed, config) = (draft.tab, draft.agent, draft.alias, draft.choice, draft.typed, draft.config)
                (remote, bypass, parallel, newBranch) = (draft.remote, draft.bypass, draft.parallel, draft.newBranch)
                return
            }
            (tab, agent) = NewTab.last(of: current, among: TaskActions.actions(in: library), in: vault)
            alias = TaskActions.lastMachine(for: current, in: vault, library: library)?.alias
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

    private var reopen: ReopenMenu {
        ReopenMenu(task: task.id, in: vault, ended: { library.ended($0) }, reachable: { library.reachable($0) })
    }

    /// Where the Agent session works: the repository and the Worktree in it.
    @ViewBuilder private var folder: some View {
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

    @ToolbarContentBuilder private var buttons: some ToolbarContent {
        ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
        ToolbarItem(placement: .confirmationAction) { Button(confirm, action: go).disabled(!ready) }
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

    private var confirm: String {
        switch tab {
        case .session: linked == nil ? "Start" : "Clone and Start"
        case .terminal, .document: "Open"
        }
    }

    private func go() {
        switch tab {
        case .session: if linked == nil { start() } else { cloneThenStart() }
        case .terminal:
            guard let machine else { return }
            terminal(machine)
            dismiss()
        case .document:
            let document = vault.create(.document, .init(title: "Untitled", text: "", task: task.id, edited: 0))
            library.open(.document(document.id), in: task.id)
            dismiss()
        }
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
        let draft = Draft(tab: tab, agent: agent, alias: alias, choice: choice, typed: typed, config: config, remote: remote, bypass: bypass,
                          parallel: parallel, newBranch: newBranch)
        library.start(agent, for: current, on: machine, repo: repo, branch: fresh, remote: remote,
                      bypass: bypass) { [shown, failed] in
            failed(Shown(draft: draft, problem: $0))
        }
        dismiss()
    }
}
