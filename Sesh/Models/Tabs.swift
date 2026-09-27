import GhosttyKit
import SwiftUI

@MainActor
protocol TabContent: ObservableObject {
    var id: UUID { get }
    var name: String { get }
    var status: String { get }
    func close()
}

extension SeshSession: TabContent {}
extension Projects: TabContent {}

enum Tab: Identifiable {
    case terminal(SeshSession)
    case projects(Projects)

    @MainActor var content: any TabContent {
        switch self {
        case .terminal(let session): session
        case .projects(let projects): projects
        }
    }

    @MainActor var id: UUID { content.id }
}

/// The open Tabs. A Session keeps running while its Tab is not the visible one; only its
/// surface stops drawing, which `TerminalView` arranges when it leaves the window.
@MainActor
final class Tabs: ObservableObject {
    @Published private(set) var all: [Tab] = []
    @Published private(set) var activeID: UUID?

    var active: Tab? { all.first { $0.id == activeID } }

    func open(_ host: Host, _ mode: Host.UIMode, store: Store, app: ghostty_app_t) {
        let tab: Tab =
            switch mode {
            case .terminal: .terminal(SeshSession(host: host, store: store, app: app))
            case .projects: .projects(Projects(host: host, store: store))
            }
        all.append(tab)
        activeID = tab.id
    }

    func show(_ tab: Tab) { activeID = tab.id }

    func showHosts() { activeID = nil }

    func close(_ tab: Tab) {
        tab.content.close()
        all.removeAll { $0.id == tab.id }
        if activeID == tab.id { activeID = all.last?.id }
    }
}

/// A Tab's name and status, kept current as they change.
struct TabTitle<Label: View>: View {
    let tab: Tab
    @ViewBuilder let label: (_ name: String, _ status: String) -> Label

    var body: some View {
        switch tab {
        case .terminal(let session): Observed(content: session, label: label)
        case .projects(let projects): Observed(content: projects, label: label)
        }
    }

    private struct Observed<Content: TabContent>: View {
        @ObservedObject var content: Content
        let label: (String, String) -> Label

        var body: some View { label(content.name, content.status) }
    }
}
