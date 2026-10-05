import SwiftUI

struct ConversationView: View {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.openURL) private var openURL
    @ObservedObject var conversation: Conversation
    let title: String
    let fresh: Bool
    @State private var draft = ""
    @State private var sending = false
    @State private var picking = false
    @State private var lost = false
    @StateObject private var field = PlainField()

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var working: Bool { conversation.state == "working" }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 14) {
                ForEach(Row.rows(conversation.items)) { row in
                    RowView(row: row, conversation: conversation)
                }
                ForEach(conversation.permissions) { PermissionCard(permission: $0, conversation: conversation) }
            }
            .padding(16)
        }
        .defaultScrollAnchor(.bottom)
        .scrollDismissesKeyboard(.interactively)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                if let todo = conversation.todo?.items, !todo.isEmpty { TodoBar(items: todo) }
                input
            }
        }
        .background(flavour(.base))
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 0) {
                    Text(title).font(.ui(15).weight(.semibold)).foregroundStyle(flavour(.text))
                    Text(stateLabel).font(.ui(11)).foregroundStyle(flavour(.subtext0))
                }
            }
            if conversation.agent == .claude {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { Task { await openInClaude() } } label: { Label("Open in Claude", image: "external-link") }
                }
            }
        }
        .task { await conversation.follow() }
        .onAppear { if fresh { DispatchQueue.main.async { field.becomeFirstResponder() } } }
        .alert("Open it in the Claude app", isPresented: $lost) {
            Button("OK") {}
        } message: {
            Text("Sesh found no link for \(title). Remote Control needs Claude on the Host signed in with a Claude plan, not an API key. If it is, look for that name in the Claude app.")
        }
        .alert("Sesh could not do that", isPresented: Binding(
            get: { conversation.problem != nil }, set: { if !$0 { conversation.problem = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(conversation.problem ?? "")
        }
    }

    private var stateLabel: String {
        let agent = conversation.agent?.title ?? "Agent"
        switch conversation.state {
        case "working": return "\(agent) is working"
        case "blocked": return "\(agent) needs you"
        case "done", "idle": return "\(agent) is ready"
        default: return agent
        }
    }

    private var input: some View {
        HStack(alignment: .bottom, spacing: Metric.gap) {
            if let projects = conversation.projects {
                UploadButton(projects: projects) { picking = true }
            } else {
                Image.lucide("image-up", size: 20).foregroundStyle(flavour(.overlay0))
                    .frame(width: Metric.control, height: Metric.control)
            }
            PlainText(field: field, text: $draft, font: .systemFont(ofSize: Metric.title), lines: 6)
                .overlay(alignment: .leading) {
                    if draft.isEmpty {
                        Text("Message").font(.ui(Metric.title)).foregroundStyle(flavour(.overlay0)).allowsHitTesting(false)
                    }
                }
                .padding(.horizontal, Metric.pad)
                .padding(.vertical, Metric.gap)
                .frame(minHeight: Metric.control)
                .background(flavour(.base), in: .rect(cornerRadius: Metric.control / 2))
                .foregroundStyle(flavour(.text))
            Button { Task { await sendOrStop() } } label: {
                Image.lucide(working ? "square" : "arrow-up", size: 18)
                    .foregroundStyle(flavour(.base))
                    .frame(width: Metric.control, height: Metric.control)
                    .background(flavour(working ? .red : .mauve).opacity(canSend || working ? 1 : 0.4), in: .circle)
            }
            .disabled(!canSend && !working)
            .accessibilityLabel(working ? "Stop" : "Send")
        }
        .padding(.horizontal, Metric.pad)
        .padding(.vertical, Metric.gap)
        .background(flavour(.mantle))
        .sheet(isPresented: $picking) {
            PhotoPicker { results in
                picking = false
                conversation.projects?.upload(results) { field.insertPaths($0) }
            }
            .ignoresSafeArea()
        }
    }

    private var canSend: Bool { !sending && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    private func sendOrStop() async {
        guard !working else { return await conversation.stop() }
        sending = true
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        if let problem = await conversation.send(text) { conversation.problem = problem } else { draft = "" }
        sending = false
    }

    private func openInClaude() async {
        guard let url = await conversation.link() else { return lost = true }
        openURL(url)
    }
}

/// What the list shows: one entry, or a run of reads, searches and fetches as one line.
private enum Row: Identifiable {
    case item(Conversation.Item)
    case lookups([Conversation.Entry])

    var id: String {
        switch self {
        case .item(let item): item.id
        case .lookups(let entries): entries[0].id
        }
    }

    static func rows(_ items: [Conversation.Item]) -> [Row] {
        var rows: [Row] = []
        for item in items {
            guard case .entry(let entry) = item, entry.kind == "tool",
                  ["read", "search", "fetch"].contains(entry.tool) else {
                rows.append(.item(item))
                continue
            }
            if case .lookups(let run)? = rows.last {
                rows[rows.count - 1] = .lookups(run + [entry])
            } else {
                rows.append(.lookups([entry]))
            }
        }
        return rows
    }
}

private struct RowView: View {
    @Environment(\.colorScheme) private var colorScheme
    let row: Row
    @ObservedObject var conversation: Conversation

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        switch row {
        case .lookups(let entries): Lookups(entries: entries)
        case .item(.switched(_, let reason)): SwitchDivider(reason: reason)
        case .item(.entry(let entry)):
            switch entry.kind {
            case "user": UserBubble(entry: entry, conversation: conversation)
            case "text": Markdown(text: entry.text ?? entry.summary)
            case "thinking": Thinking(entry: entry)
            case "tool": ToolCard(entry: entry, result: conversation.results[entry.id], conversation: conversation)
            case "question": QuestionCard(entry: entry, result: conversation.results[entry.id], conversation: conversation)
            default: Text(entry.summary).font(.ui(13)).foregroundStyle(flavour(.subtext0))
            }
        }
    }
}

// MARK: Messages

private struct UserBubble: View {
    @Environment(\.colorScheme) private var colorScheme
    let entry: Conversation.Entry
    let conversation: Conversation

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            if let images = entry.images, !images.isEmpty {
                HStack(spacing: 6) {
                    ForEach(images, id: \.self) { RemoteImage(path: $0, conversation: conversation) }
                }
            }
            if let text = entry.text, !text.isEmpty {
                Text(text)
                    .font(.ui(15))
                    .foregroundStyle(flavour(.text))
                    .padding(.horizontal, 14)
                    .padding(.vertical, 9)
                    .background(flavour(.surface0), in: .rect(cornerRadius: 18))
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, 48)
    }
}

private struct RemoteImage: View {
    @Environment(\.colorScheme) private var colorScheme
    let path: String
    let conversation: Conversation
    @State private var image: UIImage?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Group {
            if let image {
                Image(uiImage: image).resizable().scaledToFill()
            } else {
                Image.lucide("image", size: 20).foregroundStyle(flavour(.overlay1))
            }
        }
        .frame(width: 120, height: 120)
        .background(flavour(.surface0))
        .clipShape(.rect(cornerRadius: 14))
        .task { image = await conversation.image(path) }
        .accessibilityLabel((path as NSString).lastPathComponent)
    }
}

/// Paragraphs, headings and fenced code; inline markdown inside paragraphs is SwiftUI's.
struct Markdown: View {
    @Environment(\.colorScheme) private var colorScheme
    let text: String

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    private enum Block {
        case paragraph(String), heading(String), code(String)
    }

    private var blocks: [Block] {
        var blocks: [Block] = []
        var lines: [String] = []
        var fenced = false
        func flush() {
            let joined = lines.joined(separator: "\n")
            if fenced { blocks.append(.code(joined)) } else if !joined.isEmpty { blocks.append(.paragraph(joined)) }
            lines = []
        }
        for line in text.components(separatedBy: "\n") {
            if line.hasPrefix("```") {
                flush()
                fenced.toggle()
            } else if fenced {
                lines.append(line)
            } else if line.hasPrefix("#") {
                flush()
                blocks.append(.heading(line.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces)))
            } else if line.trimmingCharacters(in: .whitespaces).isEmpty {
                flush()
            } else {
                lines.append(line.replacing(#/^(\s*)[-*] /#, with: { "\($0.1)• " }))
            }
        }
        flush()
        return blocks
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case .heading(let line): Text(inline(line)).font(.ui(17).weight(.semibold))
                case .paragraph(let lines): Text(inline(lines)).font(.ui(15))
                case .code(let code):
                    ScrollView(.horizontal, showsIndicators: false) {
                        Text(code).font(.system(size: 12, design: .monospaced)).padding(12)
                    }
                    .background(flavour(.mantle), in: .rect(cornerRadius: 8))
                }
            }
        }
        .foregroundStyle(flavour(.text))
        .tint(flavour(.blue))
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: text, options: options)) ?? AttributedString(text)
    }
}

private struct Thinking: View {
    @Environment(\.colorScheme) private var colorScheme
    let entry: Conversation.Entry
    @State private var open = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button { open.toggle() } label: {
                HStack(spacing: 6) {
                    Image.lucide("brain", size: 14)
                    Text(entry.seconds.map { "Thought for \(Int($0.rounded()))s" } ?? "Thought")
                    Image.lucide(open ? "chevron-down" : "chevron-right", size: 12)
                }
                .font(.ui(13))
                .foregroundStyle(flavour(.overlay1))
            }
            if open, let text = entry.text {
                Text(text).font(.ui(13).italic()).foregroundStyle(flavour(.subtext0)).textSelection(.enabled)
            }
        }
    }
}

private struct SwitchDivider: View {
    @Environment(\.colorScheme) private var colorScheme
    let reason: String

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    private var label: String {
        switch reason {
        case "clear": "Cleared"
        case "resume": "Resumed another conversation"
        case "new": "New conversation"
        case "compact": "Compacted"
        case "fork": "Forked"
        default: "Moved to another Transcript"
        }
    }

    var body: some View {
        HStack(spacing: 10) {
            Rectangle().fill(flavour(.surface1)).frame(height: 1)
            Text(label).font(.ui(12)).foregroundStyle(flavour(.subtext0)).fixedSize()
            Rectangle().fill(flavour(.surface1)).frame(height: 1)
        }
        .padding(.vertical, 6)
    }
}

// MARK: Tool cards

/// A rounded row that opens to show more. Every tool, question and permission is one.
private struct Card<Header: View, Detail: View>: View {
    @Environment(\.colorScheme) private var colorScheme
    let icon: String
    var tint = Catppuccin.Swatch.overlay1
    var opens = true
    @ViewBuilder let header: () -> Header
    @ViewBuilder let detail: () -> Detail
    var opened: () -> Void = {}
    @State private var open = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.pad) {
            Button {
                guard opens else { return }
                open.toggle()
                if open { opened() }
            } label: {
                HStack(alignment: .top, spacing: Metric.pad) {
                    Image.lucide(icon, size: Metric.body).foregroundStyle(flavour(tint)).padding(.top, 1)
                    header().frame(maxWidth: .infinity, alignment: .leading)
                    if opens {
                        Image.lucide(open ? "chevron-down" : "chevron-right", size: Metric.note).foregroundStyle(flavour(.overlay1))
                    }
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if open { detail() }
        }
        .padding(Metric.pad)
        .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
    }
}

private struct ToolCard: View {
    @Environment(\.colorScheme) private var colorScheme
    let entry: Conversation.Entry
    let result: Conversation.Entry?
    let conversation: Conversation

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var failed: Bool { result?.error == true }

    var body: some View {
        switch entry.tool {
        case "edit", "write":
            Card(icon: entry.tool == "edit" ? "file-pen" : "file-plus", tint: failed ? .red : .blue,
                 opens: result?.diff != nil || result?.text != nil) {
                HStack(spacing: Metric.gap) {
                    Text(entry.file.map { ($0 as NSString).lastPathComponent } ?? entry.summary)
                        .font(.ui(Metric.label).weight(.medium)).foregroundStyle(flavour(.text)).lineLimit(1)
                    Spacer(minLength: Metric.tiny)
                    if let added = result?.added, added > 0 { Text("+\(added)").foregroundStyle(flavour(.green)) }
                    if let removed = result?.removed, removed > 0 { Text("\u{2212}\(removed)").foregroundStyle(flavour(.red)) }
                }
                .font(.system(size: Metric.caption, design: .monospaced))
            } detail: {
                if let diff = result?.diff { Diff(text: diff, truncated: result?.truncated == true) } else { output }
            } opened: { expand() }
        case "bash":
            Card(icon: "terminal", tint: failed ? .red : .green, opens: result != nil) {
                VStack(alignment: .leading, spacing: Metric.tiny) {
                    if let description = entry.description {
                        Text(description).font(.ui(Metric.note)).foregroundStyle(flavour(.subtext0))
                    }
                    Text(entry.command ?? entry.summary)
                        .font(.system(size: Metric.caption, design: .monospaced)).foregroundStyle(flavour(.text)).lineLimit(3)
                }
            } detail: { output } opened: { expand() }
        case "task":
            Card(icon: "bot", tint: .mauve, opens: result != nil) {
                VStack(alignment: .leading, spacing: Metric.tiny) {
                    Text("Task").font(.ui(Metric.caption)).foregroundStyle(flavour(.subtext0))
                    Text(entry.description ?? entry.summary).font(.ui(Metric.label)).foregroundStyle(flavour(.text))
                }
            } detail: {
                Markdown(text: result?.text ?? "")
            } opened: { expand() }
        default:
            Card(icon: "wrench", tint: failed ? .red : .overlay1, opens: result?.text != nil) {
                Text(entry.summary).font(.ui(Metric.label)).foregroundStyle(flavour(.text))
            } detail: { output } opened: { expand() }
        }
    }

    private var output: some View {
        Output(text: result?.text ?? "", failed: failed, truncated: result?.truncated == true)
    }

    private func expand() {
        guard let result else { return }
        Task { await conversation.expand(result) }
    }
}

private struct Output: View {
    @Environment(\.colorScheme) private var colorScheme
    let text: String
    let failed: Bool
    let truncated: Bool

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(text.isEmpty ? "No output" : text.trimmingCharacters(in: .newlines))
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(flavour(failed ? .red : .subtext1))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            if truncated { Text("Some lines in the middle are not shown").font(.ui(11)).foregroundStyle(flavour(.overlay1)) }
        }
        .padding(10)
        .background(flavour(.base), in: .rect(cornerRadius: 6))
    }
}

private struct Diff: View {
    @Environment(\.colorScheme) private var colorScheme
    let text: String
    let truncated: Bool

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    private var lines: [String] {
        let lines = text.trimmingCharacters(in: .newlines).components(separatedBy: "\n")
        return Array(lines.drop { !$0.hasPrefix("@@") })
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                Text(line.isEmpty ? " " : line)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(flavour(line.hasPrefix("@@") ? .overlay1 : .text))
                    .padding(.horizontal, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(background(line))
            }
            if truncated { Text("Some lines in the middle are not shown").font(.ui(11)).foregroundStyle(flavour(.overlay1)).padding(8) }
        }
        .padding(.vertical, 6)
        .background(flavour(.base), in: .rect(cornerRadius: 6))
        .clipShape(.rect(cornerRadius: 6))
    }

    private func background(_ line: String) -> Color {
        switch line.first {
        case "+": flavour(.green).opacity(0.18)
        case "-": flavour(.red).opacity(0.18)
        default: .clear
        }
    }
}

private struct Lookups: View {
    @Environment(\.colorScheme) private var colorScheme
    let entries: [Conversation.Entry]

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    private var line: String {
        let count = { (tool: String) in entries.filter { $0.tool == tool }.count }
        let parts = [
            (count("read"), "Read %d file", "Read %d files"),
            (count("search"), "searched once", "searched %d times"),
            (count("fetch"), "fetched a page", "fetched %d pages"),
        ]
        .filter { $0.0 > 0 }
        .map { String(format: $0.0 == 1 ? $0.1 : $0.2, $0.0) }
        let joined = parts.joined(separator: ", ")
        return joined.prefix(1).uppercased() + joined.dropFirst()
    }

    var body: some View {
        Card(icon: "search") {
            Text(line).font(.ui(14)).foregroundStyle(flavour(.text))
        } detail: {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(entries) { entry in
                    Text(entry.file ?? entry.command ?? entry.summary)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(flavour(.subtext1))
                        .lineLimit(2)
                }
            }
        }
    }
}

// MARK: Questions and permissions

private struct QuestionCard: View {
    @Environment(\.colorScheme) private var colorScheme
    let entry: Conversation.Entry
    let result: Conversation.Entry?
    let conversation: Conversation
    @State private var picked: [Int: Set<String>] = [:]
    @State private var typed: [Int: String] = [:]
    @State private var sending = false
    @State private var problem: String?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var questions: [Conversation.Question] { entry.questions ?? [] }

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
                        TextField("Something else", text: Binding(get: { typed[index, default: ""] }, set: { typed[index] = $0 }))
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
                    Text(sending ? "Sending…" : "Send").frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .tint(flavour(.mauve))
                .disabled(sending || !complete)
            }
        }
        .padding(Metric.pad)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
        .overlay(RoundedRectangle(cornerRadius: Metric.corner).stroke(flavour(result == nil ? .mauve : .mantle).opacity(0.5)))
    }

    private func answered(_ text: String) -> some View {
        Label { Text(text) } icon: { Image.lucide("circle-check", size: Metric.label) }
            .font(.ui(Metric.label))
            .foregroundStyle(flavour(.subtext0))
    }

    private var complete: Bool {
        questions.indices.allSatisfy { !picked[$0, default: []].isEmpty || !typed[$0, default: ""].isEmpty }
    }

    private func choice(_ option: Conversation.Question.Option, multi: Bool, in index: Int) -> some View {
        let on = picked[index, default: []].contains(option.label)
        return Button {
            var set = multi ? picked[index, default: []] : []
            if on { set.remove(option.label) } else { set.insert(option.label) }
            picked[index] = set
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

    private func send() async {
        sending = true
        problem = await conversation.answer(questions.indices.map {
            (Array(picked[$0, default: []]), typed[$0, default: ""])
        })
        sending = false
    }
}

private struct PermissionCard: View {
    @Environment(\.colorScheme) private var colorScheme
    let permission: Conversation.Permission
    let conversation: Conversation
    @State private var answering = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
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
            HStack(spacing: Metric.pad) {
                Button { Task { await answer(false) } } label: { Text("Deny").frame(maxWidth: .infinity) }
                    .buttonStyle(.bordered)
                    .tint(flavour(.red))
                Button { Task { await answer(true) } } label: { Text("Allow").frame(maxWidth: .infinity) }
                    .buttonStyle(.borderedProminent)
                    .tint(flavour(.mauve))
            }
            .disabled(answering)
        }
        .padding(Metric.pad)
        .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
        .overlay(RoundedRectangle(cornerRadius: Metric.corner).stroke(flavour(.peach).opacity(0.6)))
    }

    /// The command or file is drawn below, so the header does not repeat it.
    private var title: String {
        let agent = conversation.agent?.title ?? "The Agent"
        if permission.command != nil { return "\(agent) wants to run a command" }
        if permission.file != nil { return "\(agent) wants to change a file" }
        return permission.summary
    }

    private func answer(_ allow: Bool) async {
        answering = true
        await conversation.permit(allow)
        answering = false
    }
}

private struct TodoBar: View {
    @Environment(\.colorScheme) private var colorScheme
    let items: [Conversation.Todo]
    @State private var open = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var done: Int { items.filter { $0.status == "completed" }.count }
    private var current: Conversation.Todo? {
        items.first { $0.status == "in_progress" } ?? items.first { $0.status == "pending" }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.gap) {
            Button { open.toggle() } label: {
                HStack(spacing: Metric.gap) {
                    Image.lucide("list-checks", size: Metric.body).foregroundStyle(flavour(.mauve))
                    Text("\(done)/\(items.count)").font(.ui(Metric.note).monospacedDigit()).foregroundStyle(flavour(.subtext0))
                    Text(current?.text ?? "All done").font(.ui(Metric.note)).foregroundStyle(flavour(.text)).lineLimit(1)
                    Spacer(minLength: Metric.tiny)
                    Image.lucide(open ? "chevron-down" : "chevron-up", size: Metric.note).foregroundStyle(flavour(.overlay1))
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if open {
                ForEach(items, id: \.self) { item in
                    HStack(alignment: .top, spacing: Metric.gap) {
                        Image.lucide(icon(item.status), size: 14).foregroundStyle(flavour(tint(item.status)))
                        Text(item.text)
                            .font(.ui(Metric.note))
                            .strikethrough(item.status == "completed")
                            .foregroundStyle(flavour(item.status == "completed" ? .subtext0 : .text))
                    }
                }
            }
        }
        .padding(.horizontal, Metric.wide)
        .padding(.vertical, Metric.pad)
        .background(flavour(.mantle))
        .overlay(alignment: .bottom) { Rectangle().fill(flavour(.surface0)).frame(height: 1) }
    }

    private func icon(_ status: String) -> String {
        switch status {
        case "completed": "circle-check"
        case "in_progress": "circle-dot"
        default: "circle"
        }
    }

    private func tint(_ status: String) -> Catppuccin.Swatch {
        switch status {
        case "completed": .green
        case "in_progress": .blue
        default: .overlay1
        }
    }
}

private struct UploadButton: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var projects: Projects
    let pick: () -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Button {
            if projects.uploading != nil { projects.cancelUpload() } else { pick() }
        } label: {
            Group {
                if let fraction = projects.uploading?.fraction {
                    UploadRing(fraction: fraction, colour: flavour(.mauve))
                } else {
                    Image.lucide("image-up", size: 20)
                }
            }
            .foregroundStyle(flavour(.subtext0))
            .frame(width: 38, height: 38)
        }
        .disabled(projects.uploading == nil && !projects.canUpload)
        .accessibilityLabel("upload")
        .uploadFailure($projects.uploadError)
    }
}
