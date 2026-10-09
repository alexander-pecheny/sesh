#if DEBUG
import SwiftUI

/// `xcrun simctl launch <udid> me.pecheny.sesh -fixture claude` draws a Conversation from
/// Resources/Fixtures/claude.jsonl with no Host behind it.
struct FixtureView: View {
    let name: String
    @StateObject private var conversation: Conversation

    init(name: String) {
        self.name = name
        let conversation = Conversation(pane: "fixture", agent: nil, runner: nil)
        let url = Bundle.main.url(forResource: name, withExtension: "jsonl", subdirectory: "Fixtures")
        let lines = url.flatMap { try? String(contentsOf: $0, encoding: .utf8) } ?? ""
        // In bursts after the screen appears, as history comes from a Host, then the last
        // lines one a second, as a live turn does.
        let all = lines.split(separator: "\n")
        let live = max(all.count - 20, 0)
        for (index, line) in all.enumerated() {
            let delay = index < live ? Double(index / 20) * 0.1 : Double(live / 20) * 0.1 + Double(index - live + 1)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5 + delay) { conversation.receive(String(line)) }
        }
        _conversation = StateObject(wrappedValue: conversation)
    }

    var body: some View {
        NavigationStack { ConversationView(conversation: conversation, title: "\(name)-fixture", fresh: false) }
    }
}
#endif
