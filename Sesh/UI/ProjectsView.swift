import SwiftUI

struct ProjectsView: View {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.scenePhase) private var scenePhase
    @ObservedObject var projects: Projects

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Group {
            switch projects.stage {
            case .ready where projects.missing.contains("herdr") || projects.missing.contains("claude"):
                MissingTools(missing: projects.missing.filter { $0 != "git" })
            case .ready:
                NavigationStack {
                    FolderView(projects: projects, path: projects.home)
                        .navigationDestination(for: String.self) { FolderView(projects: projects, path: $0) }
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

private struct MissingTools: View {
    @Environment(\.colorScheme) private var colorScheme
    let missing: [String]

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    private var commands: String {
        missing.map {
            $0 == "herdr" ? "curl -fsSL https://herdr.dev/install.sh | sh" : "curl -fsSL https://claude.ai/install.sh | bash"
        }
        .joined(separator: "\n")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("This Host is missing \(missing.joined(separator: " and "))")
                .font(.ui(17))
                .foregroundStyle(flavour(.text))
            Text("Projects needs them to run Claude. Whoever looks after the Host can install them with:")
                .font(.ui(14))
                .foregroundStyle(flavour(.subtext0))
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
        .padding(24)
    }
}

private struct FolderView: View {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.openURL) private var openURL
    @ObservedObject var projects: Projects
    let path: String
    @State private var listing: Projects.Listing?
    @State private var problem: String?
    @State private var naming = false
    @State private var newName = ""
    @State private var starting = false
    @State private var lost: String?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var isHome: Bool { path == projects.home }
    private var title: String { isHome ? "Home" : (path as NSString).lastPathComponent }
    private var here: [Projects.ClaudeSession] {
        isHome ? projects.sessions : projects.sessions.filter { $0.folder == path }
    }

    var body: some View {
        List {
            if !here.isEmpty {
                Section("Claude sessions") {
                    ForEach(here) { session in
                        SessionRow(session: session, home: projects.home, showFolder: isHome) {
                            Task { await open(session) }
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
                        Label { Text("Start Claude here") } icon: { Image.lucide("sparkles") }
                    }
                    .font(.ui(16))
                }
                .listRowBackground(flavour(.mantle))
            }
            Section("Folders") {
                if let listing {
                    ForEach(listing.folders, id: \.self) { name in
                        NavigationLink(value: path + "/" + name) {
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
        .alert("Open it in the Claude app", isPresented: Binding(get: { lost != nil }, set: { if !$0 { lost = nil } })) {
            Button("OK") { lost = nil }
        } message: {
            Text("Sesh found no link for \(lost ?? ""). Remote Control needs Claude on the Host signed in with a Claude plan, not an API key. If it is, look for that name in the Claude app.")
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

    private func open(_ session: Projects.ClaudeSession) async {
        guard let url = await projects.link(for: session.pane) else { return lost = session.name }
        openURL(url)
    }
}

private struct SessionRow: View {
    @Environment(\.colorScheme) private var colorScheme
    let session: Projects.ClaudeSession
    let home: String
    let showFolder: Bool
    let open: () -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    private var subtitle: String? {
        let folder = session.folder.hasPrefix(home) ? "~" + session.folder.dropFirst(home.count) : session.folder
        let parts = [showFolder ? folder : nil, session.branch.map { "branch \($0)" }].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
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
        Button(action: open) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(session.name).font(.ui(16)).foregroundStyle(flavour(.text))
                    if let subtitle {
                        Text(subtitle).font(.ui(12)).foregroundStyle(flavour(.subtext0)).lineLimit(1)
                    }
                }
                Spacer(minLength: 8)
                Text(state.0)
                    .font(.ui(12))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(flavour(state.1).opacity(0.18), in: .capsule)
                    .foregroundStyle(flavour(state.1))
            }
        }
        .accessibilityHint("Opens it in the Claude app")
    }
}

private struct StartSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var projects: Projects
    let folder: String
    let git: Bool
    @State private var name = ""
    @State private var branch = false
    @State private var working = false
    @State private var problem: String?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledField("Name", "", text: $name)
                    if git { Toggle("On a new branch", isOn: $branch) }
                } footer: {
                    Text(branch
                         ? "Claude works on its own copy of \((folder as NSString).lastPathComponent), on a branch named \(Projects.slug(name))."
                         : "Claude keeps running on the Host after you close Sesh. Talk to it in the Claude app.")
                }
                if let problem {
                    Section("Claude did not start") {
                        Text(problem).font(.system(size: 12, design: .monospaced)).foregroundStyle(flavour(.red))
                    }
                }
            }
            .font(.ui(14))
            .disabled(working)
            .navigationTitle("Start Claude")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(working) }
                ToolbarItem(placement: .confirmationAction) {
                    if working { ProgressView() } else { Button("Start") { Task { await start() } } }
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
        problem = await projects.start(slug, in: folder, branch: branch)
        working = false
        if problem == nil { dismiss() }
    }
}
