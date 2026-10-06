import SwiftUI

/// The Agent sessions herdr runs on this Mac, each opening as a Conversation.
struct RootView: View {
    /// `-host ALIAS` lists another machine's sessions instead of this Mac's.
    private let machine = UserDefaults.standard.string(forKey: "host").map { Machine(alias: $0) } ?? .mac
    @State private var sessions: [Listed] = []
    /// `-open PANE` on the command line opens one at launch, for looking without clicking.
    @State private var selection = UserDefaults.standard.string(forKey: "open")
    @State private var problem: String?

    struct Listed: Identifiable, Decodable {
        let agent: String
        let cwd: String
        let pane_id: String
        var id: String { pane_id }
    }

    private struct List: Decodable {
        struct Result: Decodable { let agents: [Listed] }
        let result: Result
    }

    var body: some View {
        NavigationSplitView {
            SwiftUI.List(sessions, selection: $selection) { session in
                VStack(alignment: .leading) {
                    Text((session.cwd as NSString).lastPathComponent)
                    Text(session.agent).font(.caption).foregroundStyle(.secondary)
                }
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 220)
        } detail: {
            if let selection, let session = sessions.first(where: { $0.id == selection }) {
                SessionDetail(pane: selection, agent: Agent(rawValue: session.agent), machine: machine, title: (session.cwd as NSString).lastPathComponent)
                    .id(selection)
            } else {
                Text(problem ?? "Pick an Agent session").foregroundStyle(.secondary)
            }
        }
        .task {
            problem = await machine.prepare()
            let ran = await machine.run("herdr agent list")
            let list = try? JSONDecoder().decode(List.self, from: Data(ran.out.utf8))
            sessions = list?.result.agents.filter { Agent(rawValue: $0.agent) != nil } ?? []
        }
    }
}

private struct SessionDetail: View {
    @StateObject private var conversation: Conversation
    let title: String

    init(pane: String, agent: Agent?, machine: Machine, title: String) {
        _conversation = StateObject(wrappedValue: Conversation(pane: pane, agent: agent, runner: machine))
        self.title = title
    }

    var body: some View {
        // A fixed floor stops the split view re-reading the Conversation's size in a loop.
        ConversationView(conversation: conversation, title: title, fresh: false)
            .frame(minWidth: 420, maxWidth: .infinity, minHeight: 300, maxHeight: .infinity)
    }
}
