#if DEBUG
import SwiftUI

/// `xcrun simctl launch <udid> me.pecheny.sesh -fixture claude` draws a Conversation from
/// Resources/Fixtures/claude.jsonl with no Host behind it; `-fixture refusal` draws the
/// screen a Host without Sesh's herdr gets.
struct FixtureView: View {
    let name: String
    @StateObject private var conversation: Conversation

    init(name: String) {
        self.name = name
        let conversation = Conversation(pane: "fixture", agent: nil, projects: nil)
        let url = Bundle.main.url(forResource: name, withExtension: "jsonl", subdirectory: "Fixtures")
        let lines = url.flatMap { try? String(contentsOf: $0, encoding: .utf8) } ?? ""
        // In bursts after the screen appears, as lines come from a Host.
        for (index, line) in lines.split(separator: "\n").enumerated() {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5 + Double(index / 20) * 0.1) {
                conversation.apply(String(line))
            }
        }
        _conversation = StateObject(wrappedValue: conversation)
    }

    var body: some View {
        if name == "refusal" {
            NeedsHerdr()
        } else {
            NavigationStack { ConversationView(conversation: conversation, title: "\(name)-fixture", fresh: false) }
        }
    }
}
#endif
