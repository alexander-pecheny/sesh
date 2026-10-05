import SwiftUI

struct ProjectsView: View {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.scenePhase) private var scenePhase
    @ObservedObject var projects: Projects

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Group {
            switch projects.stage {
            case .ready where projects.needsHerdr:
                NeedsHerdr()
            case .ready:
                NavigationStack(path: $projects.route) {
                    FolderView(projects: projects, path: projects.home)
                        .navigationDestination(for: Projects.Place.self) { place in
                            switch place {
                            case .folder(let path): FolderView(projects: projects, path: path)
                            case .conversation(let pane, let fresh):
                                ConversationScreen(projects: projects, pane: pane, fresh: fresh)
                            }
                        }
                }
                .tint(flavour(.mauve))
            case .failed:
                Notice(text: projects.status) {
                    Button("Reconnect") { projects.resume() }
                        .buttonStyle(.borderedProminent)
                        .tint(flavour(.mauve))
                }
            default:
                Notice(text: projects.status) { ProgressView() }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(flavour(.base))
        .task {
            while !Task.isCancelled {
                await projects.refresh()
                try? await Task.sleep(for: .seconds(3))
            }
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background: projects.close()
            case .active: projects.resume()
            default: break
            }
        }
        .sheet(item: $projects.hostKeyQuestion) { question in
            HostKeySheet(question: question, host: projects.host) { projects.answerHostKey($0) }
        }
        .sheet(item: $projects.authQuestion) { question in
            AuthSheet(question: question, savePassword: $projects.savePassword) {
                projects.answerPrompt(question, $0)
            }
        }
    }
}

private struct ConversationScreen: View {
    @ObservedObject var projects: Projects
    let pane: String
    let fresh: Bool
    @StateObject private var conversation: Conversation

    init(projects: Projects, pane: String, fresh: Bool) {
        self.projects = projects
        self.pane = pane
        self.fresh = fresh
        let agent = projects.sessions.first { $0.pane == pane }?.agent
        _conversation = StateObject(wrappedValue: Conversation(pane: pane, agent: agent, projects: projects))
    }

    var body: some View {
        ConversationView(
            conversation: conversation,
            title: projects.sessions.first { $0.pane == pane }?.name ?? "Agent session",
            fresh: fresh)
    }
}

private struct Notice<Action: View>: View {
    @Environment(\.colorScheme) private var colorScheme
    let text: String
    @ViewBuilder let action: () -> Action

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(spacing: 16) {
            Text(text)
                .font(.ui(14))
                .multilineTextAlignment(.center)
                .foregroundStyle(flavour(.text))
            action()
        }
        .padding(24)
    }
}

/// Commands someone runs on the Host, with a Copy button, since Sesh never installs anything.
private struct Install: View {
    @Environment(\.colorScheme) private var colorScheme
    let commands: String

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Text(commands)
            .font(.system(size: 12, design: .monospaced))
            .textSelection(.enabled)
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(flavour(.mantle), in: .rect(cornerRadius: 8))
            .foregroundStyle(flavour(.text))
        Button("Copy") { UIPasteboard.general.string = commands }
            .buttonStyle(.borderedProminent)
            .tint(flavour(.mauve))
    }
}

struct NeedsHerdr: View {
    @Environment(\.colorScheme) private var colorScheme

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("This Host needs Sesh's herdr")
                .font(.ui(17))
                .foregroundStyle(flavour(.text))
            Text("Projects shows Claude, Codex and pi as chat through a build of herdr made for Sesh. Whoever looks after the Host can install it, with Rust and Zig on the PATH, by running:")
                .font(.ui(14))
                .foregroundStyle(flavour(.subtext0))
            Install(commands: """
                git clone https://code.pecheny.me/pecheny/herdr.git
                cd herdr && just build
                install -m755 target/release/herdr ~/.local/bin/herdr
                herdr server live-handoff
                herdr integration install claude
                herdr integration install codex
                herdr integration install pi
                """)
            Text("Codex then shows its permission prompts here only once its new hook is trusted: open Codex, type /hooks and press t.")
                .font(.ui(14))
                .foregroundStyle(flavour(.subtext0))
        }
        .padding(24)
    }
}

private struct FolderView: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var projects: Projects
    let path: String
    @State private var listing: Projects.Listing?
    @State private var problem: String?
    @State private var naming = false
    @State private var newName = ""
    @State private var starting = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var isHome: Bool { path == projects.home }
    private var title: String { isHome ? "Home" : (path as NSString).lastPathComponent }
    private var here: [Projects.AgentSession] {
        isHome ? projects.sessions : projects.sessions.filter { $0.folder == path }
    }

    var body: some View {
        List {
            if !here.isEmpty {
                Section("Agent sessions") {
                    ForEach(here) { session in
                        NavigationLink(value: Projects.Place.conversation(pane: session.pane, fresh: false)) {
                            SessionRow(session: session, home: projects.home, showFolder: isHome)
                        }
                        .swipeActions {
                            Button("Stop", role: .destructive) { Task { await projects.stop(session) } }
                        }
                    }
                }
                .listRowBackground(flavour(.mantle))
            }
            if !isHome {
                Section {
                    Button { starting = true } label: {
                        Label { Text("Start an Agent here") } icon: { Image.lucide("sparkles") }
                    }
                    .font(.ui(16))
                }
                .listRowBackground(flavour(.mantle))
            }
            Section("Folders") {
                if let listing {
                    ForEach(listing.folders, id: \.self) { name in
                        NavigationLink(value: Projects.Place.folder(path + "/" + name)) {
                            Label { Text(name).foregroundStyle(flavour(.text)) } icon: { Image.lucide("folder").foregroundStyle(flavour(.blue)) }
                                .font(.ui(16))
                        }
                    }
                    if listing.folders.isEmpty {
                        Text("No folders yet").font(.ui(14)).foregroundStyle(flavour(.subtext0))
                    }
                } else if let problem {
                    Text(problem).font(.ui(14)).foregroundStyle(flavour(.red))
                } else {
                    ProgressView()
                }
            }
            .listRowBackground(flavour(.mantle))
        }
        .scrollContentBackground(.hidden)
        .background(flavour(.base))
        .navigationTitle(title)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { naming = true } label: { Label("New folder", image: "folder-plus") }
            }
        }
        .task { await load() }
        .refreshable { await load() }
        .alert("New folder", isPresented: $naming) {
            TextField("Name", text: $newName)
            Button("Cancel", role: .cancel) { newName = "" }
            Button("Create") { Task { await create() } }
        }
        .sheet(isPresented: $starting) {
            StartSheet(projects: projects, folder: path, git: listing?.git == true && projects.canBranch)
        }
    }

    private func load() async {
        switch await projects.list(path) {
        case .success(let found): listing = found; problem = nil
        case .failure(let failure): problem = failure.message
        }
    }

    private func create() async {
        let name = newName.trimmingCharacters(in: .whitespaces)
        newName = ""
        guard !name.isEmpty else { return }
        guard !name.contains("/"), !name.hasPrefix(".") else {
            problem = "A folder name cannot contain / or start with a dot."
            return
        }
        if let error = await projects.makeFolder(name, in: path) { problem = error }
        await load()
    }
}

private struct SessionRow: View {
    @Environment(\.colorScheme) private var colorScheme
    let session: Projects.AgentSession
    let home: String
    let showFolder: Bool

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    private var subtitle: String {
        let folder = session.folder.hasPrefix(home) ? "~" + session.folder.dropFirst(home.count) : session.folder
        return [session.agent.title, showFolder ? folder : nil, session.branch.map { "branch \($0)" }]
            .compactMap { $0 }
            .joined(separator: " · ")
    }

    private var state: (String, Catppuccin.Swatch) {
        switch session.state {
        case "working": ("Working", .blue)
        case "blocked": ("Needs you", .peach)
        case "done": ("Done", .green)
        case "idle": ("Ready", .green)
        default: ("Running", .overlay1)
        }
    }

    var body: some View {
        HStack(spacing: Metric.pad) {
            VStack(alignment: .leading, spacing: 2) {
                Text(session.name).font(.ui(Metric.title)).foregroundStyle(flavour(.text))
                Text(subtitle).font(.ui(Metric.caption)).foregroundStyle(flavour(.subtext0)).lineLimit(1)
            }
            Spacer(minLength: Metric.gap)
            Text(state.0)
                .font(.ui(Metric.caption))
                .padding(.horizontal, Metric.gap)
                .padding(.vertical, 3)
                .background(flavour(state.1).opacity(0.18), in: .capsule)
                .foregroundStyle(flavour(state.1))
        }
        .accessibilityHint("Opens its Conversation")
    }
}

private struct StartSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var projects: Projects
    let folder: String
    let git: Bool
    @AppStorage("agent") private var agent = Agent.claude
    @State private var name = ""
    @State private var branch = false
    @State private var working = false
    @State private var problem: String?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var missing: Bool { projects.missing.contains(agent.rawValue) }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker("Agent", selection: $agent) {
                        ForEach(Agent.allCases) { Text($0.title).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
                }
                if missing {
                    Section {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("This Host has no \(agent.rawValue). Whoever looks after the Host can install it with:")
                                .foregroundStyle(flavour(.subtext0))
                            Install(commands: agent.install)
                        }
                        .padding(.vertical, 6)
                    }
                } else {
                    Section {
                        LabeledField("Name", "", text: $name)
                        if git { Toggle("On a new branch", isOn: $branch) }
                    } footer: {
                        Text(branch
                             ? "\(agent.title) works on its own copy of \((folder as NSString).lastPathComponent), on a branch named \(Projects.slug(name))."
                             : "\(agent.title) keeps running on the Host after you close Sesh.")
                    }
                }
                if let problem {
                    Section("\(agent.title) did not start") {
                        Text(problem).font(.system(size: 12, design: .monospaced)).foregroundStyle(flavour(.red))
                    }
                }
            }
            .font(.ui(14))
            .disabled(working)
            .navigationTitle("Start an Agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(working) }
                ToolbarItem(placement: .confirmationAction) {
                    if working {
                        ProgressView()
                    } else {
                        Button("Start") { Task { await start() } }.disabled(missing)
                    }
                }
            }
        }
        .interactiveDismissDisabled(working)
        .onAppear { if name.isEmpty { name = projects.suggestName(for: folder) } }
    }

    private func start() async {
        let slug = Projects.slug(name)
        guard !slug.isEmpty else { return problem = "Give it a name first." }
        name = slug
        working = true
        let started = await projects.start(slug, agent: agent, in: folder, branch: branch)
        working = false
        switch started {
        case .failure(let failure): problem = failure.message
        case .success(let pane):
            dismiss()
            projects.route.append(.conversation(pane: pane, fresh: true))
        }
    }
}
