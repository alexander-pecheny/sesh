import SwiftUI

/// The user's messages, sent or still queued, share one shape.
private enum Bubble {
    static let spacing: CGFloat = 6
    static let across: CGFloat = 14
    static let down: CGFloat = 9
    static let corner: CGFloat = 18
    static let inset: CGFloat = 48
}

struct UserBubble: View {
    @Environment(\.flavour) private var flavour
    let entry: SessionLog.Entry
    let conversation: Conversation

    var body: some View {
        VStack(alignment: .trailing, spacing: Bubble.spacing) {
            if let images = entry.images, !images.isEmpty {
                HStack(spacing: Bubble.spacing) {
                    ForEach(images, id: \.self) { RemoteImage(path: $0, conversation: conversation) }
                }
            }
            if let text = entry.text, !text.isEmpty {
                message(text)
                    .font(.ui(Metric.body))
                    .foregroundStyle(flavour(.text))
                    .padding(.horizontal, Bubble.across)
                    .padding(.vertical, Bubble.down)
                    .background(flavour(.surface0), in: .rect(cornerRadius: Bubble.corner))
                    .textSelection(.enabled)
            }
            Stamp(at: entry.at)
        }
        // Sent, and not yet seen by the Agent.
        .opacity(entry.state == "sent" ? Self.unseen : 1)
        .readable(alignment: .trailing)
        .padding(.leading, Bubble.inset)
    }

    private func message(_ text: String) -> some View { UserText(text: text) }

    private static let unseen = 0.5
}

/// When a message was written: the time today, the date and time before.
struct Stamp: View {
    @Environment(\.flavour) private var flavour
    let at: String?

    private static let parser: ISO8601DateFormatter = {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return parser
    }()

    var body: some View {
        if let at, let date = Self.parser.date(from: at) ?? ISO8601DateFormatter().date(from: at) {
            Text(Calendar.current.isDateInToday(date)
                 ? date.formatted(date: .omitted, time: .shortened)
                 : date.formatted(.dateTime.day().month(.abbreviated).hour().minute()))
                .font(.ui(Metric.small).monospacedDigit())
                .foregroundStyle(flavour(.overlay0))
                .help(date.formatted(date: .complete, time: .standard))
        }
    }
}

/// The user's Markdown too: the whole of it on the Mac, its inline marks on the phone.
private struct UserText: View {
    let text: String

    var body: some View {
        #if os(macOS)
        Prose(text: text)
        #else
        if MathText.extract(text).formulas.isEmpty { Text(LocalizedStringKey(text)) } else { Prose(text: text) }
        #endif
    }
}

/// A message waiting to be taken (ADR 0015): it can be taken back, pushed in, or sent again.
struct QueuedBubble: View {
    @Environment(\.flavour) private var flavour
    let message: SessionLog.Entry
    @ObservedObject var conversation: Conversation

    private var agent: String { conversation.agent?.title ?? "the Agent" }

    var body: some View {
        VStack(alignment: .trailing, spacing: Bubble.spacing) {
            UserText(text: message.text ?? "")
                .font(.ui(Metric.body))
                .foregroundStyle(flavour(.subtext0))
                .padding(.horizontal, Bubble.across)
                .padding(.vertical, Bubble.down)
                .overlay(RoundedRectangle(cornerRadius: Bubble.corner).strokeBorder(flavour(.surface1), style: Self.dashes))
            HStack(spacing: Metric.pad) {
                if message.state == "handed" {
                    Text("Queued; \(agent) reads it at its next step").foregroundStyle(flavour(.overlay1))
                    Button("Interrupt and send now") { Task { await conversation.interrupt() } }
                } else if message.state == "lost" {
                    Text("\(agent) did not take it").foregroundStyle(flavour(.overlay1))
                    Button("Send again") { Task { await conversation.resend(message) } }
                    takeBack
                } else if conversation.isLocal(message) {
                    Text(conversation.starting ? "Sent once \(agent) is up" : "Not sent").foregroundStyle(flavour(.overlay1))
                    takeBack
                } else {
                    Text("Sent when \(agent) finishes").foregroundStyle(flavour(.overlay1))
                    takeBack
                    Button("Interrupt") { Task { await conversation.interrupt() } }
                    if conversation.agent == .claude {
                        Button("Send now") { Task { await conversation.sendNow() } }
                            .help("Claude takes it without stopping and reads it at its next step")
                    }
                }
            }
            .font(.ui(Metric.caption))
            .tint(flavour(.mauve))
        }
        .readable(alignment: .trailing)
        .padding(.leading, Bubble.inset)
    }

    @ViewBuilder private var takeBack: some View {
        Button("Edit") { Task { if let text = await conversation.unqueue(message) { conversation.takeBack(text) } } }
        Button("Remove") { Task { _ = await conversation.unqueue(message) } }
    }

    private static let dashes = StrokeStyle(lineWidth: 1, dash: [Metric.tiny, 3])
}

private struct RemoteImage: View {
    @Environment(\.flavour) private var flavour
    let path: String
    let conversation: Conversation
    @State private var image: PlatformImage?

    private static let widest: CGFloat = 320
    private static let tallest: CGFloat = 200
    private static let loading: CGFloat = 120

    var body: some View {
        Group {
            if let image {
                let scale = min(Self.widest / image.size.width, Self.tallest / image.size.height)
                Image(platform: image).resizable()
                    .frame(width: image.size.width * scale, height: image.size.height * scale)
            } else {
                Image.lucide("image", size: 20).foregroundStyle(flavour(.overlay1))
                    .frame(width: Self.loading, height: Self.loading)
            }
        }
        .background(flavour(.surface0))
        .clipShape(.rect(cornerRadius: 14))
        .task { image = await conversation.image(path) }
        .onTapGesture { conversation.viewing = image }
        .accessibilityLabel((path as NSString).lastPathComponent)
        .accessibilityAddTraits(.isButton)
    }
}
