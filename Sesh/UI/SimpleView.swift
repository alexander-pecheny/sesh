import SwiftUI

/// Simple mode's one screen: the Claude sessions this phone started on a Host, each opened in
/// the Claude app, and a way to start another, always with remote control.
struct SimpleView: View {
    @Environment(\.flavour) private var flavour
    @Environment(\.openURL) private var openURL
    @EnvironmentObject private var store: Store
    @AppStorage("simpleHost") private var chosen = ""
    @State private var states: [UUID: Status] = [:]
    @State private var starting = false
    @State private var settings = false
    @State private var opening: UUID?
    @State private var lost: SimpleSession?
    @State private var problem: String?

    enum Status { case running, ended, gone }

    static func label(_ folder: String) -> String { folder == "~" ? "Home" : folder }

    private var host: Host? { store.hosts.first { $0.id.uuidString == chosen } ?? store.hosts.first }
    private var sessions: [SimpleSession] { store.simpleSessions.filter { $0.host == host?.id }.reversed() }

    var body: some View {
        Group {
            if let host {
                list(host)
            } else {
                empty
            }
        }
        .scrollContentBackground(.hidden)
        .background(flavour(.base))
        .navigationTitle(host?.title ?? "Claude")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { settings = true } label: { Label("Settings", image: "settings") }
            }
            if store.hosts.count > 1 {
                ToolbarItem(placement: .principal) {
                    Menu {
                        Picker("Host", selection: $chosen) {
                            ForEach(store.hosts) { Text($0.title).tag($0.id.uuidString) }
                        }
                    } label: {
                        HStack(spacing: Metric.tiny) {
                            Text(host?.title ?? "Host").font(.ui(Metric.title))
                            Image.lucide("chevron-down", size: Metric.small)
                        }
                        .foregroundStyle(flavour(.text))
                    }
                }
            }
            if host != nil {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { starting = true } label: { Label("New Claude session", image: "plus") }
                }
            }
        }
        .sheet(isPresented: $settings) { SettingsView() }
        .sheet(isPresented: $starting) {
            if let host { SimpleStartSheet(host: host) { session in Task { await open(session, fresh: true) } } }
        }
        .task(id: host?.id) {
            while !Task.isCancelled {
                await refresh()
                try? await Task.sleep(for: .seconds(5))
            }
        }
        .alert("Open it in the Claude app", isPresented: Binding(get: { lost != nil }, set: { if !$0 { lost = nil } })) {
            Button("OK") {}
        } message: {
            Text("Sesh found no link for \(lost?.name ?? "this session"). Remote control needs Claude on the Host signed in with a Claude plan, not an API key. If it is, look for that name in the Claude app.")
        }
        .alert("Sesh could not do that on \(host?.title ?? "the Host")", isPresented: Binding(get: { problem != nil }, set: { if !$0 { problem = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(problem ?? "")
        }
    }

    private func list(_ host: Host) -> some View {
        List {
            ForEach(sessions) { session in
                Button { Task { await open(session) } } label: { row(session) }
                    .disabled(opening != nil)
                    .swipeActions(allowsFullSwipe: false) {
                        Button(states[session.id] == .running ? "Stop" : "Remove", role: .destructive) {
                            Task { await stop(session, on: host) }
                        }
                    }
            }
            if sessions.isEmpty {
                VStack(spacing: Metric.wide) {
                    Text("No Claude sessions yet").font(.ui(Metric.body)).foregroundStyle(flavour(.text))
                    Text("Start one here, then carry on in the Claude app.")
                        .font(.ui(Metric.label)).foregroundStyle(flavour(.subtext0)).multilineTextAlignment(.center)
                    Button("New Claude session") { starting = true }
                        .font(.ui(Metric.body))
                        .buttonStyle(.borderedProminent)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, Metric.wide)
                .listRowBackground(Color.clear)
            }
        }
        .refreshable { await refresh() }
    }

    private func row(_ session: SimpleSession) -> some View {
        HStack(spacing: Metric.pad) {
            Circle().fill(dot(states[session.id])).frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 2) {
                Text(session.name).font(.ui(Metric.title)).foregroundStyle(flavour(.text))
                Text(SimpleView.label(session.folder)).font(.ui(Metric.caption)).foregroundStyle(flavour(.subtext0))
            }
            Spacer()
            if opening == session.id {
                ProgressView()
            } else if states[session.id] == .running {
                Image.lucide("external-link", size: Metric.label).foregroundStyle(flavour(.overlay1))
            } else if let status = states[session.id] {
                Text(status == .ended ? "Ended" : "Gone").font(.ui(Metric.caption)).foregroundStyle(flavour(.overlay1))
            }
        }
        .contentShape(.rect)
    }

    private func dot(_ status: Status?) -> Color {
        switch status {
        case .running: flavour(.green)
        case .ended, .gone: flavour(.overlay0)
        case nil: flavour(.surface2)
        }
    }

    private var empty: some View {
        VStack(spacing: Metric.wide) {
            Text("No Hosts yet").font(.ui(17)).foregroundStyle(flavour(.text))
            Text("Add the machine Claude runs on in Settings.")
                .font(.ui(Metric.label)).foregroundStyle(flavour(.subtext0)).multilineTextAlignment(.center)
            Button("Open Settings") { settings = true }
                .font(.ui(Metric.body))
                .buttonStyle(.borderedProminent)
        }
        .padding(Metric.wide)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// sesh-mux gives a closed pane's id to the next, so a pane counts only under the same name.
    private func refresh() async {
        guard let host else { return }
        let machine = Machine.of(host)
        for session in sessions {
            switch await Mux.look(for: session.pane, on: machine) {
            case .found(let pane) where pane.name == session.name: states[session.id] = pane.agent == nil ? .ended : .running
            case .found, .gone: states[session.id] = .gone
            case .unreachable: states[session.id] = nil
            }
        }
    }

    /// A fresh session prints its link only once Claude is up, so Sesh waits a little for it.
    private func open(_ session: SimpleSession, fresh: Bool = false) async {
        guard let host = store.hosts.first(where: { $0.id == session.host }) else { return }
        opening = session.id
        defer { opening = nil }
        let machine = Machine.of(host)
        for _ in 0..<(fresh ? 20 : 1) {
            if let url = await Mux.claudeLink(for: session.pane, on: machine) { return openURL(url) }
            try? await Task.sleep(for: .seconds(1))
        }
        lost = session
    }

    private func stop(_ session: SimpleSession, on host: Host) async {
        if states[session.id] != .gone { await Mux.closeWorkspace(session.workspace, on: Machine.of(host)) }
        store.simpleSessions.removeAll { $0.id == session.id }
        states[session.id] = nil
    }
}

/// The folder a new Claude session starts in, with remote control, named after the folder.
private struct SimpleStartSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var store: Store
    let host: Host
    let started: (SimpleSession) -> Void
    @State private var folder = "~"
    @State private var home: String?
    @State private var picking = false
    @State private var working = false
    @State private var problem: String?

    private var machine: Machine { Machine.of(host) }

    var body: some View {
        SheetForm(title: "New Claude session", action: "Start", enabled: home != nil,
                  working: working, perform: { Task { await start() } }) {
            Section {
                Button { picking = true } label: {
                    LabeledContent("Folder") { Text(SimpleView.label(folder)) }
                }
                .disabled(home == nil)
            } footer: {
                Text(problem ?? "The Claude app opens on it once Claude is up.")
            }
        }
        .task {
            let ran = await machine.run("printf %s \"$HOME\"")
            if ran.ok, !ran.out.isEmpty { home = ran.out } else { problem = "Sesh could not reach \(host.title): \(ran.problem)" }
        }
        .sheet(isPresented: $picking) {
            FolderPicker(machine: machine, start: folder, home: home) { folder = $0 }
        }
    }

    private func start() async {
        guard let home else { return }
        working = true
        defer { working = false }
        let path = folder == "~" ? home : folder.hasPrefix("~/") ? home + folder.dropFirst(1) : folder
        if let problem = await machine.prepare() { return self.problem = problem }
        let base = Names.slug(folder == "~" ? "home" : (folder as NSString).lastPathComponent)
        let slug = Names.free(base.isEmpty ? "claude" : base, taken: await Mux.agentNames(on: machine))
        switch await Mux.workspace(path, label: slug, on: machine) {
        case .failure(let failure): problem = failure.message
        case .success(let opened):
            if let failed = await machine.follower.start(.claude, name: slug, pane: opened.pane, remote: true) {
                await Mux.closeWorkspace(opened.workspace, on: machine)
                return problem = failed
            }
            let session = SimpleSession(host: host.id, workspace: opened.workspace, pane: opened.pane, name: slug, folder: folder)
            store.simpleSessions.append(session)
            dismiss()
            started(session)
        }
    }
}
