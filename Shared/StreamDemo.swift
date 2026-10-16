#if DEBUG
import SwiftUI

/// `-stream FILE` plays the file as a reply streaming in, over and over, the way the helper
/// sends one read off Claude's screen: a few words at a time and a pause between paragraphs.
struct StreamDemo: View {
    @StateObject private var conversation = Conversation(pane: "stream", agent: nil, runner: nil)
    let file: String
    private static let tick = Duration.milliseconds(60)
    private static let words = 4
    private static let paragraph = Duration.milliseconds(400)
    private static let between = Duration.seconds(2)

    var body: some View {
        ConversationView(conversation: conversation, title: file, fresh: false)
            .task { await play() }
    }

    private func play() async {
        let text = (try? String(contentsOfFile: file, encoding: .utf8)) ?? ""
        send(["t": "cursor", "cursor": "0"])
        for round in 1... {
            send(["t": "entry", "id": "ask\(round)", "kind": "user", "summary": "", "at": "2026-10-16T12:00:00Z", "text": "Explain it again, round \(round)."])
            send(["t": "session", "state": "working", "status": ""])
            var shown = ""
            for paragraph in text.components(separatedBy: "\n\n") {
                let words = paragraph.split(separator: " ", omittingEmptySubsequences: false)
                for end in stride(from: Self.words, through: words.count + Self.words - 1, by: Self.words) {
                    let part = words.prefix(end).joined(separator: " ")
                    send(["t": "live", "status": "", "items": [["id": "live\(round)", "kind": "text", "summary": "", "text": shown + part]]])
                    try? await Task.sleep(for: Self.tick)
                }
                shown += paragraph + "\n\n"
                try? await Task.sleep(for: Self.paragraph)
                if Task.isCancelled { return }
            }
            send(["t": "entry", "id": "reply\(round)", "replaces": "live\(round)", "kind": "text", "summary": "", "at": "2026-10-16T12:00:00Z", "text": text])
            send(["t": "session", "state": "idle", "status": ""])
            try? await Task.sleep(for: Self.between)
        }
    }

    private func send(_ line: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: line) else { return }
        conversation.receive(String(decoding: data, as: UTF8.self))
    }
}
#endif
