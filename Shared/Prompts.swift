import SwiftUI

/// A prominent button that keeps its colours in a background window, where macOS greys a tinted one.
struct FilledButton: ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    let fill: Color
    let text: Color

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(text)
            .padding(.vertical, Metric.gap)
            .padding(.horizontal, Metric.pad)
            .background(fill.opacity(configuration.isPressed ? 0.8 : 1), in: .rect(cornerRadius: Metric.corner))
            .opacity(enabled ? 1 : 0.4)
            .contentShape(.rect)
    }
}

struct QuestionCard: View {
    @Environment(\.flavour) private var flavour
    let entry: SessionLog.Entry
    let result: SessionLog.Entry?
    let conversation: Conversation
    @State private var picked: [Int: Set<String>]
    @State private var typed: [Int: String]
    @State private var problem: String?

    init(entry: SessionLog.Entry, result: SessionLog.Entry?, conversation: Conversation) {
        self.entry = entry
        self.result = result
        self.conversation = conversation
        let kept = conversation.answers[entry.id]
        _picked = State(initialValue: kept?.picked ?? [:])
        _typed = State(initialValue: kept?.typed ?? [:])
    }

    private var questions: [SessionLog.Question] { entry.questions ?? [] }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.wide) {
            ForEach(Array(questions.enumerated()), id: \.offset) { index, question in
                VStack(alignment: .leading, spacing: Metric.gap) {
                    if let header = question.header {
                        Text(header.uppercased()).font(.ui(Metric.small).weight(.semibold)).foregroundStyle(flavour(.mauve))
                    }
                    Text(question.question).font(.ui(Metric.body).weight(.medium)).foregroundStyle(flavour(.text))
                    if let answer = result?.answers?[safe: index] {
                        answered(answer)
                    } else if result == nil {
                        ForEach(question.options, id: \.label) { option in
                            choice(option, multi: question.multi == true, in: index)
                        }
                        TextField("Something else", text: Binding(get: { typed[index, default: ""] }, set: { type($0, multi: question.multi == true, in: index) }))
                            .font(.ui(Metric.label))
                            .padding(Metric.pad)
                            .background(flavour(.base), in: .rect(cornerRadius: Metric.corner))
                    }
                }
            }
            if let result {
                if result.answers == nil { answered(result.text ?? "Answered") }
            } else {
                if let problem { Text(problem).font(.system(size: Metric.small, design: .monospaced)).foregroundStyle(flavour(.red)) }
                Button { Task { await send() } } label: {
                    Text(sent ? "Sending…" : "Send").frame(maxWidth: .infinity)
                }
                .buttonStyle(FilledButton(fill: flavour(.mauve), text: flavour(.base)))
                .disabled(sent || !complete)
            }
        }
        .padding(Metric.pad)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
        .overlay(RoundedRectangle(cornerRadius: Metric.corner).stroke(flavour(result == nil ? .mauve : .mantle).opacity(0.5)))
        .onChange(of: picked) { conversation.answers[entry.id] = (picked, typed) }
        .onChange(of: typed) { conversation.answers[entry.id] = (picked, typed) }
    }

    private func answered(_ text: String) -> some View {
        Label { Text(text) } icon: { Image.lucide("circle-check", size: Metric.label) }
            .font(.ui(Metric.label))
            .foregroundStyle(flavour(.subtext0))
    }

    private var complete: Bool {
        questions.indices.allSatisfy { !picked[$0, default: []].isEmpty || !typed[$0, default: ""].isEmpty }
    }

    /// A single-choice question takes an option or text, so typing clears the option.
    private func type(_ text: String, multi: Bool, in index: Int) {
        typed[index] = text
        if !multi && !text.isEmpty { picked[index] = nil }
    }

    private func choice(_ option: SessionLog.Question.Option, multi: Bool, in index: Int) -> some View {
        let on = picked[index, default: []].contains(option.label)
        return Button {
            var set = multi ? picked[index, default: []] : []
            if on { set.remove(option.label) } else { set.insert(option.label) }
            picked[index] = set
            if !multi && !on { typed[index] = nil }
        } label: {
            HStack(alignment: .top, spacing: Metric.pad) {
                Image.lucide(on ? (multi ? "circle-check" : "circle-dot") : "circle", size: 16)
                    .foregroundStyle(flavour(on ? .mauve : .overlay1))
                VStack(alignment: .leading, spacing: 2) {
                    Text(option.label).font(.ui(Metric.label)).foregroundStyle(flavour(.text))
                    if let description = option.description {
                        Text(description).font(.ui(Metric.caption)).foregroundStyle(flavour(.subtext0))
                    }
                }
                Spacer(minLength: 0)
            }
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }

    /// Sent, the card waits for its answer to show.
    private var sent: Bool { conversation.answered.contains(entry.id) }

    private func send() async {
        problem = await conversation.answer(questions.indices.map {
            (Array(picked[$0, default: []]), typed[$0, default: ""])
        }, to: entry.id)
    }
}

struct PermissionCard: View {
    @Environment(\.flavour) private var flavour
    let permission: SessionLog.Permission
    let conversation: Conversation

    /// Answered, the card stays until the Agent moves on.
    private var answering: Bool { conversation.answered.contains(permission.id) }

    var body: some View {
        if permission.tool == "login", let url = permission.url.flatMap(URL.init(string:)) {
            LoginCard(url: url) { await conversation.login($0, permission) }
        } else {
            prompt
        }
    }

    private var prompt: some View {
        VStack(alignment: .leading, spacing: Metric.pad) {
            HStack(spacing: Metric.gap) {
                Image.lucide("shield-alert", size: Metric.title).foregroundStyle(flavour(.peach))
                Text(title).font(.ui(Metric.body).weight(.medium)).foregroundStyle(flavour(.text))
            }
            if let detail = permission.command ?? permission.file {
                Text(detail)
                    .font(.system(size: Metric.caption, design: .monospaced))
                    .foregroundStyle(flavour(.text))
                    .padding(Metric.pad)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(flavour(.base), in: .rect(cornerRadius: 6))
            }
            if let reason = permission.reason {
                Text(reason).font(.ui(Metric.note)).foregroundStyle(flavour(.subtext0))
            }
            if let options = permission.options {
                Text(permission.summary).font(.ui(Metric.note)).foregroundStyle(flavour(.text)).textSelection(.enabled)
                ForEach(options, id: \.self) { choice in
                    Button { Task { await conversation.choose(choice, in: permission) } } label: {
                        HStack {
                            Text(choice.key.allSatisfy(\.isNumber) ? "\(choice.key). \(choice.label)" : choice.label)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            hint(choice.key.count == 1 ? "⌥⌘" + choice.key : nil)
                        }
                    }
                    .buttonStyle(.bordered)
                    .tint(flavour(choice == options.first ? .mauve : .overlay1))
                }
                .disabled(answering)
            } else {
                HStack(spacing: Metric.pad) {
                    Button { Task { await conversation.permit(false, permission) } } label: {
                        HStack { Text("Deny"); hint("⌘.") }.frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)
                    .tint(flavour(.red))
                    Button { Task { await conversation.permit(true, permission) } } label: {
                        HStack { Text("Allow"); hint("⌘↩") }.frame(maxWidth: .infinity)
                    }
                    .buttonStyle(FilledButton(fill: flavour(.mauve), text: flavour(.base)))
                }
                .disabled(answering)
            }
        }
        .padding(Metric.pad)
        .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
        .overlay(RoundedRectangle(cornerRadius: Metric.corner).stroke(flavour(.peach).opacity(0.6)))
    }

    /// The keys that answer from anywhere in the window, which only the Mac's keyboard has.
    @ViewBuilder private func hint(_ keys: String?) -> some View {
        #if os(macOS)
        if let keys { Text(keys).opacity(0.6) }
        #endif
    }

    /// The command or file is drawn below, so the header does not repeat it.
    private var title: String {
        let agent = conversation.agent?.title ?? "The Agent"
        if permission.command != nil { return "\(agent) wants to run a command" }
        if permission.file != nil { return "\(agent) wants to change a file" }
        if permission.options != nil { return "\(agent) is asking" }
        return permission.summary
    }
}

#if os(macOS)
/// The open permission's answers as keys, which work wherever the card is scrolled to; typed
/// keys alone go to the message box.
struct PermissionKeys: View {
    let permission: SessionLog.Permission
    let conversation: Conversation

    var body: some View {
        ZStack {
            if let options = permission.options {
                ForEach(options, id: \.self) { choice in
                    if choice.key.count == 1, let key = choice.key.first {
                        Button("") { Task { await conversation.choose(choice, in: permission) } }
                            .keyboardShortcut(KeyEquivalent(key), modifiers: [.command, .option])
                    }
                }
            } else {
                Button("") { Task { await conversation.permit(true, permission) } }.keyboardShortcut(.return)
                Button("") { Task { await conversation.permit(false, permission) } }.keyboardShortcut(".")
            }
        }
        .hidden()
    }
}
#endif
