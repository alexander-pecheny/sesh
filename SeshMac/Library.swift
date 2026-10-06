import Foundation

/// Every Vault this Mac has opened, and what is open in each Task.
@MainActor
final class Library: ObservableObject {
    @Published private(set) var vaults: [Vault] = []
    @Published var selection: String?
    /// The open Tabs of each Task by its id; the Journal is never in here, as it never closes.
    @Published var tabs: [String: [TabItem]] = [:]
    @Published var current: [String: TabItem] = [:]

    private static let key = "vaults"
    /// Kept while the app runs, so switching Tabs neither loses the place nor refetches.
    private var conversations: [String: Conversation] = [:]

    func conversation(for session: Record) -> Conversation {
        if let known = conversations[session.id] { return known }
        let conversation = Conversation(
            pane: session.body.pane ?? "", agent: session.body.agent.flatMap(Agent.init),
            runner: TaskActions.machine(session.body.machine))
        conversations[session.id] = conversation
        return conversation
    }

    init() {
        let places = UserDefaults.standard.data(forKey: Self.key)
            .flatMap { try? JSONDecoder().decode([Vault.Place].self, from: $0) } ?? []
        vaults = places.map(Vault.init)
        vaults.forEach { $0.start() }
    }

    func add(_ place: Vault.Place) {
        guard !vaults.contains(where: { $0.name == place.name }) else { return }
        let vault = Vault(place)
        vaults.append(vault)
        vault.start()
        UserDefaults.standard.set(try? JSONEncoder().encode(vaults.map(\.place)), forKey: Self.key)
    }

    func vault(of task: String) -> Vault? { vaults.first { $0.records[task] != nil } }

    func open(_ tab: TabItem, in task: String) {
        if tab != .journal, !(tabs[task] ?? []).contains(tab) { tabs[task, default: []].append(tab) }
        current[task] = tab
        selection = task
    }

    func close(_ tab: TabItem, in task: String) {
        tabs[task]?.removeAll { $0 == tab }
        if current[task] == tab { current[task] = tabs[task]?.last ?? .journal }
    }
}

/// One Tab of a Task. Sessions and Documents are records; a Terminal lives only while open.
enum TabItem: Hashable, Identifiable {
    case journal
    case session(String)
    case document(String)
    case terminal(UUID)

    var id: Self { self }
}
