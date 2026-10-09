import MarkdownUI
import SwiftUI

struct ConversationView: View {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.openURL) private var openURL
    @ObservedObject var conversation: Conversation
    let title: String
    let fresh: Bool
    var hidden = false
    /// Starts the Agent again, for a session whose Agent has stopped.
    var resume: (() async -> Void)?
    @State private var draft = ""
    @State private var notice: String?
    @State private var sending = false
    @State private var picking = false
    @State private var lost = false
    @StateObject private var field = PlainField()
    @State private var composing = false
    /// A message open on its own for selecting part of it, which the phone's list cannot do.
    @State private var selecting: String?
    /// The rows keep their drawing until their version changes, so a time stamped today
    /// changes to a date only when the day is part of it.
    @State private var today = Calendar.current.startOfDay(for: .now)

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var working: Bool { conversation.state == "working" }
    private static let rowSpacing: CGFloat = 14
    private static let focusTint = 0.15
    static let shadow: CGFloat = 3

    var body: some View {
        chat
        .overlay(alignment: .top) {
            if let notice {
                Text(notice)
                    .font(.ui(Metric.note).weight(.medium))
                    .foregroundStyle(flavour(.text))
                    .padding(.horizontal, Metric.pad)
                    .padding(.vertical, Metric.gap)
                    .background(flavour(.surface1), in: .capsule)
                    .shadow(radius: Self.shadow)
                    .padding(Metric.pad)
                    .transition(.opacity)
                    .allowsHitTesting(false)
            }
        }
        .overlay(alignment: .bottom) { MoveToBottom(scroll: conversation.scroll, flavour: flavour) }
        .scrollDismissesKeyboard(.interactively)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 0) {
                if let todo = conversation.todo?.items, !todo.isEmpty { TodoBar(items: todo) }
                // A Transcript read from a file, a copy or a subagent's, has no one to send to.
                if conversation.pane != nil {
                    BackgroundLine(conversation: conversation)
                    input
                } else if let resume {
                    EndedBar(agent: conversation.agent?.title ?? "The Agent", resume: resume)
                }
            }
        }
        // Not sideways: under macOS 26's floating sidebar the colour would tint its glass.
        .background(flavour(.base), ignoresSafeAreaEdges: .vertical)
        .navigationTitle(title)
        .inlineTitle()
        #if os(macOS)
        .navigationSubtitle(stateLabel)
        .buttonStyle(.plain)
        #endif
        .toolbar {
            #if os(iOS)
            ToolbarItem(placement: .principal) {
                VStack(spacing: 0) {
                    Text(title).font(.ui(15).weight(.semibold)).foregroundStyle(flavour(.text))
                    Text(stateLabel).font(.ui(11)).foregroundStyle(flavour(.subtext0))
                }
            }
            #endif
            if conversation.agent == .claude, resume == nil {
                ToolbarItem(placement: .primaryAction) {
                    Button { Task { await openInClaude() } } label: { Label("Open in Claude", image: "external-link") }
                }
            }
        }
        .task { await conversation.follow() }
        .environment(\.openURL, links)
        .onAppear(perform: appeared)
        .onReceive(NotificationCenter.default.publisher(for: .NSCalendarDayChanged).receive(on: RunLoop.main)) { _ in
            today = Calendar.current.startOfDay(for: .now)
        }
        .onChange(of: draft) { conversation.draft = draft }
        #if os(macOS)
        .onChange(of: hidden) { field.catchesTyping = !hidden }
        .background {
            if !hidden, let permission = conversation.permissions.first {
                PermissionKeys(permission: permission, conversation: conversation).id(permission.id)
            }
        }
        #endif
        .sheet(isPresented: Binding(get: { selecting != nil }, set: { if !$0 { selecting = nil } })) {
            if let selecting { SelectSheet(text: selecting) }
        }
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

    private var chat: some View {
        ChatList(items: listItems, scroll: conversation.scroll, ready: conversation.loaded, spacing: Self.rowSpacing, inset: Metric.wide)
            .animation(.easeOut(duration: 1), value: conversation.focus)
    }

    private func rowView(_ row: SessionLog.Row) -> some View {
        RowView(row: row, conversation: conversation)
            .padding(Metric.tiny)
            .background(
                conversation.focus.map(row.contains) == true ? flavour(.yellow).opacity(Self.focusTint) : .clear,
                in: .rect(cornerRadius: Metric.corner))
            .bookmarkable(row.entry, live: row.live, keep: confirmed(conversation.bookmark, "Bookmarked in the Journal"),
                          copy: confirmed(conversation.copyLink, "Link copied"), column: row.prose ? Metric.proseColumn : nil,
                          text: row.text, copyText: { copyText($0) }, select: { selecting = $0 })
    }

    /// Everything the native list shows, each row versioned by what it draws.
    private var listItems: [ChatList.Item] {
        func item(_ id: String, _ version: Int, _ view: some View) -> ChatList.Item {
            ChatList.Item(id: id, version: version, view: AnyView(
                view.frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, Metric.wide).environment(\.openURL, links)))
        }
        var items: [ChatList.Item] = []
        if conversation.earlier { items.append(item(ChatScroll.earlier, 0, ProgressView().frame(maxWidth: .infinity))) }
        for row in conversation.rows {
            var hasher = Hasher()
            hasher.combine(row.version)
            hasher.combine(conversation.opened.contains(row.id))
            hasher.combine(conversation.focus.map(row.contains))
            hasher.combine(today)
            items.append(item(row.id, hasher.finalize(), rowView(row)))
        }
        for permission in conversation.permissions {
            items.append(item("permission." + permission.id, permission.hashValue, PermissionCard(permission: permission, conversation: conversation)))
        }
        for message in conversation.queued {
            var hasher = Hasher()
            hasher.combine(message.text)
            hasher.combine(message.handed)
            items.append(item("queued." + message.id.uuidString, hasher.finalize(), QueuedBubble(message: message, conversation: conversation, edit: takeBack)))
        }
        if working || conversation.state == "background" && !conversation.status.isEmpty {
            items.append(item("working", conversation.status.hashValue, WorkingRow(status: conversation.status)))
        }
        return items
    }

    private var links: OpenURLAction {
        OpenURLAction { url in
            // A link read off the screen, whose address only the Transcript has.
            if url.absoluteString == "about:blank" { return .handled }
            guard let path = PathLinks.path(from: url), let open = conversation.openPath else { return .systemAction }
            open(path)
            return .handled
        }
    }

    private var stateLabel: String {
        let agent = conversation.agent?.title ?? "Agent"
        if resume != nil { return "Ended" }
        if conversation.starting { return "Starting \(agent)" }
        switch conversation.state {
        case "working": return "\(agent) is working"
        case "background": return "\(agent) is waiting on background work"
        case "blocked": return "\(agent) needs you"
        case "done", "idle": return "\(agent) is ready"
        default: return agent
        }
    }

    private var input: some View {
        HStack(alignment: .bottom, spacing: Metric.gap) {
            #if os(iOS)
            if let link = (conversation.runner as? Machine)?.link {
                UploadButton(link: link) { picking = true }
            } else {
                Image.lucide("image-up", size: 20).foregroundStyle(flavour(.overlay0))
                    .frame(width: Metric.control, height: Metric.control)
            }
            #endif
            PlainText(field: field, text: $draft, font: .systemFont(ofSize: Metric.title), lines: 6) { if canSend { Task { await send() } } }
                .overlay(alignment: .leading) {
                    if draft.isEmpty {
                        Text("Message").font(.ui(Metric.title)).foregroundStyle(flavour(.overlay0)).allowsHitTesting(false)
                    }
                }
                .padding(.leading, Metric.pad)
                .padding(.trailing, Metric.control)
                .padding(.vertical, Metric.gap)
                .frame(minHeight: Metric.control)
                .overlay(alignment: .bottomTrailing) {
                    Button { composing = true } label: {
                        Image.lucide("maximize-2", size: Metric.label).foregroundStyle(flavour(.overlay1))
                            .frame(width: Metric.control, height: Metric.control)
                    }
                    .accessibilityLabel("Expand")
                }
                .background(flavour(.base), in: .rect(cornerRadius: Metric.control / 2))
                .foregroundStyle(flavour(.text))
            Button { Task { await sendOrStop() } } label: {
                Image.lucide(stops ? "square" : "arrow-up", size: 18)
                    .foregroundStyle(flavour(.base))
                    .frame(width: Metric.control, height: Metric.control)
                    .background(flavour(stops ? .red : .mauve).opacity(canSend || stops ? 1 : 0.4), in: .circle)
            }
            .disabled(!canSend && !stops)
            .accessibilityLabel(stops ? "Stop" : "Send")
            .help(stops ? "Stop the Agent" : "Send")
        }
        .padding(.horizontal, Metric.pad)
        .padding(.vertical, Metric.gap)
        .background(flavour(.mantle), ignoresSafeAreaEdges: .vertical)
        #if os(iOS)
        .sheet(isPresented: $picking) {
            PhotoPicker { results in
                picking = false
                (conversation.runner as? Machine)?.link?.upload(results) { field.insertPaths($0) }
            }
            .ignoresSafeArea()
        }
        #endif
        .cover(isPresented: Binding(
            get: { conversation.viewing != nil }, set: { if !$0 { conversation.viewing = nil } }
        )) {
            if let image = conversation.viewing { ImageViewer(image: image) }
        }
        // A send from the full-screen editor jumps while the chat is covered, by sizes that
        // change as it closes; the jump is made again once it has.
        .onChange(of: composing) { if !composing, conversation.scroll.atBottom { conversation.scroll.jump() } }
        .cover(isPresented: $composing) {
            Composer(text: $draft, canSend: canSend) { Task { await send() } }
        }
    }

    private func appeared() {
        if draft.isEmpty { draft = conversation.draft }
        if fresh { DispatchQueue.main.async { field.focus() } }
        let conversation = conversation
        field.recall = { [weak conversation] in
            conversation?.rows.flatMap(\.entries).last { $0.kind == "user" && $0.text?.isEmpty == false }?.text
        }
        #if os(macOS)
        field.pasteImage = { [weak conversation] data, ext in
            let path = await conversation?.upload(data, ext: ext)
            if path == nil { conversation?.problem = "Sesh could not upload the pasted image." }
            return path
        }
        field.escape = { [weak conversation] in
            guard let conversation, conversation.state == "working" else { return }
            Task { await conversation.stop() }
        }
        field.catchesTyping = !hidden
        #endif
    }

    /// An action on a message, followed by a word that it happened.
    private func confirmed(_ action: ((SessionLog.Entry) -> Void)?, _ words: String) -> ((SessionLog.Entry) -> Void)? {
        action.map { action in
            { entry in
                action(entry)
                withAnimation { notice = words }
                Task {
                    try? await Task.sleep(for: .seconds(1.5))
                    withAnimation { if notice == words { notice = nil } }
                }
            }
        }
    }

    private func copyText(_ text: String) {
        Pasteboard.copy(text)
        withAnimation { notice = "Copied" }
        Task {
            try? await Task.sleep(for: .seconds(1.5))
            withAnimation { if notice == "Copied" { notice = nil } }
        }
    }

    /// A queued message goes back into the box, ahead of anything typed since.
    private func takeBack(_ text: String) {
        draft = draft.isEmpty ? text : text + "\n\n" + draft
    }

    private var canSend: Bool { !sending && !empty }
    private var empty: Bool { draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    /// The button stops the Agent only for an empty box, never while a message is on its way.
    private var stops: Bool { working && empty && !sending }

    /// While the Agent works, a written message queues and an empty box stops the Agent.
    private func sendOrStop() async {
        guard !stops else { return await conversation.stop() }
        await send()
    }

    private func send() async {
        sending = true
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        if let problem = await conversation.send(text) { conversation.problem = problem } else {
            // Words typed while the message was on its way stay in the box.
            let now = draft.trimmingCharacters(in: .whitespacesAndNewlines)
            draft = now.hasPrefix(text) ? String(now.dropFirst(text.count)).trimmingCharacters(in: .whitespacesAndNewlines) : draft
            // Whoever sends wants to see the reply, wherever they had scrolled.
            conversation.scroll.jump()
        }
        sending = false
    }

    private func openInClaude() async {
        guard let url = await conversation.link() else { return lost = true }
        openURL(url)
    }
}

/// An Agent's reply: one selectable text on the Mac, so a copy can span paragraphs.
private struct AgentText: View {
    let text: String
    var table = false

    var body: some View {
        #if os(macOS)
        Prose(text: text)
        #else
        if table { ProseTable(text: text) } else { Prose(text: text) }
        #endif
    }
}

/// A Bookmark, or a copied link, from a row's context menu and, where there is a pointer,
/// icons on hover: text that can be selected keeps its own context menu.
private struct Bookmarkable: ViewModifier {
    @Environment(\.colorScheme) private var colorScheme
    let entry: SessionLog.Entry?
    /// A live item, which the Transcript will hold soon: it takes the room its icons will need.
    let live: Bool
    let keep: ((SessionLog.Entry) -> Void)?
    let copy: ((SessionLog.Entry) -> Void)?
    /// How wide a reply's text column is, so the icon sits at its corner, not the window's.
    let column: CGFloat?
    /// The row's own words, for Copy and, on the phone, Select Text.
    let text: String?
    let copyText: (String) -> Void
    let select: (String) -> Void
    @State private var hovered = false
    #if os(macOS)
    private static let gutter: CGFloat = 48
    #else
    /// No hover on the phone, so no icons to make room for; the context menu has the actions.
    private static let gutter: CGFloat = 0
    #endif
    #if DEBUG
    /// `-bookmarks YES` shows every icon, for a snapshot of a window no pointer reaches.
    private static let always = UserDefaults.standard.bool(forKey: "bookmarks")
    #else
    private static let always = false
    #endif

    @ViewBuilder private var menu: some View {
        if let text, !text.isEmpty {
            Button("Copy") { copyText(text) }
            let code = Cmark.codeBlocks(text)
            if !code.isEmpty { Button(code.count == 1 ? "Copy Code" : "Copy All Code") { copyText(code.joined(separator: "\n")) } }
            #if os(iOS)
            Button("Select Text") { select(text) }
            #endif
        }
        if let entry, let keep {
            Button("Bookmark in the Journal") { keep(entry) }
            if let copy { Button("Copy Link") { copy(entry) } }
        }
    }

    func body(content: Content) -> some View {
        if let entry, let keep {
            content
                .padding(.trailing, Self.gutter)
                .contextMenu { menu }
                .overlay(alignment: .topLeading) {
                    HStack(spacing: 0) {
                        Spacer(minLength: 0)
                        if hovered || Self.always {
                            if let copy {
                                icon("link", help: "Copy a link to this message", label: "Copy Link") { copy(entry) }
                            }
                            icon("bookmark", help: "Bookmark in the Journal", label: "Bookmark") { keep(entry) }
                        }
                    }
                    .frame(maxWidth: column.map { $0 + Self.gutter } ?? .infinity)
                }
                // The whole row, not just its ink, so the pointer can travel to the icon.
                .contentShape(.rect)
                .onHover { hovered = $0 }
        } else if text?.isEmpty == false {
            content.padding(.trailing, live && keep != nil ? Self.gutter : 0).contextMenu { menu }
        } else {
            content.padding(.trailing, live && keep != nil ? Self.gutter : 0)
        }
    }

    private func icon(_ name: String, help: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: name)
                .foregroundStyle((colorScheme == .dark ? Catppuccin.Flavour.mocha : .latte)(.overlay1))
                .padding(Metric.tiny)
        }
        .buttonStyle(.plain)
        .help(help)
        .accessibilityLabel(label)
    }
}

extension View {
    fileprivate func bookmarkable(_ entry: SessionLog.Entry?, live: Bool, keep: ((SessionLog.Entry) -> Void)?,
                                  copy: ((SessionLog.Entry) -> Void)?, column: CGFloat?, text: String?,
                                  copyText: @escaping (String) -> Void, select: @escaping (String) -> Void) -> some View {
        modifier(Bookmarkable(entry: entry, live: live, keep: keep, copy: copy, column: column, text: text, copyText: copyText, select: select))
    }
}

/// One message on its own, where a long press selects words instead of opening a menu.
private struct SelectSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    let text: String

    var body: some View {
        NavigationStack {
            ScrollView { Prose(text: text).frame(maxWidth: .infinity, alignment: .leading).padding(Metric.wide) }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background((colorScheme == .dark ? Catppuccin.Flavour.mocha : .latte)(.base), ignoresSafeAreaEdges: .all)
                .navigationTitle("Select Text")
                .inlineTitle()
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        #if os(macOS)
        .frame(minWidth: 520, minHeight: 360)
        #endif
    }
}

/// What the Agent left running in the background, a command or a subagent; shown only while
/// there is some, since the working row already says when the Agent itself works. It opens
/// a list of all of it, from which a subagent opens in its own Tab.
private struct BackgroundLine: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var conversation: Conversation
    @State private var listing = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        if let first = conversation.background.first {
            Button { listing = true } label: {
                HStack(spacing: Metric.gap) {
                    Image(systemName: "gearshape.2").foregroundStyle(flavour(.yellow))
                    Text("In the background: " + (conversation.background.count == 1 ? first.label : "\(first.label) and \(conversation.background.count - 1) more"))
                        .foregroundStyle(flavour(.subtext0)).lineLimit(1).truncationMode(.tail)
                    Image(systemName: "chevron.up").foregroundStyle(flavour(.overlay1))
                    Spacer(minLength: 0)
                }
                .font(.ui(Metric.caption))
                .padding(.horizontal, Metric.wide)
                .padding(.vertical, Metric.tiny)
                .frame(maxWidth: .infinity)
                .background(flavour(.mantle))
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            .popover(isPresented: $listing, arrowEdge: .top) { list.presentationCompactAdaptation(.popover) }
        }
    }

    private var list: some View {
        VStack(alignment: .leading, spacing: Metric.gap) {
            ForEach(conversation.background) { work in
                HStack(spacing: Metric.gap) {
                    Image(systemName: work.agent == true ? "person.2" : "terminal").foregroundStyle(flavour(.overlay1))
                    Text(work.label).lineLimit(2).frame(maxWidth: 360, alignment: .leading)
                    Spacer(minLength: Metric.gap)
                    if work.agent == true, let open = conversation.openSubagent {
                        Button("Open") {
                            listing = false
                            open(work.call, work.label)
                        }
                    }
                    if let entry = conversation.entry(forCall: work.call) {
                        Button("Show") {
                            listing = false
                            conversation.focus = entry
                        }
                    }
                }
                .font(.ui(Metric.note))
            }
        }
        .padding(Metric.pad)
    }
}

/// What the list needs of a row beyond what it draws.
private extension SessionLog.Row {
    /// A reply set in the prose column, rather than a card, a bubble or a table, which takes
    /// the whole width and caps each cell instead.
    var prose: Bool {
        guard case .entry(let entry) = content, entry.kind == "text" else { return false }
        return !Cmark.hasTable(entry.text ?? "")
    }

    var live: Bool { SessionLog.isLive(id) }

    /// What Copy puts on the clipboard: the words, or the command or file a tool worked on.
    var text: String? {
        switch content {
        case .switched: nil
        case .lookups(let entries): entries.map { $0.file ?? $0.command ?? $0.summary }.joined(separator: "\n")
        case .entry(let entry):
            entry.kind == "tool" ? entry.command ?? entry.file ?? entry.summary : entry.text ?? entry.summary
        }
    }

    /// The entry a Bookmark of this row keeps; a live item is not in the Transcript yet.
    var entry: SessionLog.Entry? {
        entries.first.flatMap { SessionLog.isLive($0.id) ? nil : $0 }
    }
}

private struct RowView: View {
    @Environment(\.colorScheme) private var colorScheme
    let row: SessionLog.Row
    @ObservedObject var conversation: Conversation

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        switch row.content {
        case .lookups(let entries): Lookups(entries: entries, open: conversation.isOpen(entries[0].id), openPath: conversation.openPath)
        case .switched(let reason): SwitchDivider(reason: reason)
        case .entry(let entry):
            switch entry.kind {
            case "user": UserBubble(entry: entry, conversation: conversation)
            case "text":
                let text = conversation.openPath == nil && conversation.repo == nil
                    ? entry.text ?? entry.summary
                    : PathLinks.link(entry.text ?? entry.summary, repo: conversation.repo, paths: conversation.openPath != nil)
                let pieces = Cmark.pieces(text)
                VStack(alignment: .leading, spacing: Metric.tiny) {
                    ForEach(pieces.indices, id: \.self) { index in
                        AgentText(text: pieces[index].text, table: pieces[index].table).readable(wide: pieces[index].table)
                    }
                    Stamp(at: entry.at).readable()
                }
            case "thinking": Thinking(entry: entry, open: conversation.isOpen(entry.id))
            case "tool": ToolCard(entry: entry, result: conversation.results[entry.id], conversation: conversation)
            case "question": QuestionCard(entry: entry, result: conversation.results[entry.id], conversation: conversation).id(entry.id)
            default: Text(entry.summary).font(.ui(13)).foregroundStyle(flavour(.subtext0))
            }
        }
    }
}

// MARK: Messages

/// The user's messages, sent or still queued, share one shape.
private enum Bubble {
    static let spacing: CGFloat = 6
    static let across: CGFloat = 14
    static let down: CGFloat = 9
    static let corner: CGFloat = 18
    static let inset: CGFloat = 48
}

private struct UserBubble: View {
    @Environment(\.colorScheme) private var colorScheme
    let entry: SessionLog.Entry
    let conversation: Conversation

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

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
        .readable(alignment: .trailing)
        .padding(.leading, Bubble.inset)
    }

    private func message(_ text: String) -> some View { UserText(text: text) }
}

/// When a message was written: the time today, the date and time before.
private struct Stamp: View {
    @Environment(\.colorScheme) private var colorScheme
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
                .foregroundStyle((colorScheme == .dark ? Catppuccin.Flavour.mocha : .latte)(.overlay0))
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
        Text(LocalizedStringKey(text))
        #endif
    }
}

/// A message written while the Agent works: it waits, and can be taken back or pushed in.
private struct QueuedBubble: View {
    @Environment(\.colorScheme) private var colorScheme
    let message: Conversation.Queued
    @ObservedObject var conversation: Conversation
    let edit: (String) -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .trailing, spacing: Bubble.spacing) {
            UserText(text: message.text)
                .font(.ui(Metric.body))
                .foregroundStyle(flavour(.subtext0))
                .padding(.horizontal, Bubble.across)
                .padding(.vertical, Bubble.down)
                .overlay(RoundedRectangle(cornerRadius: Bubble.corner).strokeBorder(flavour(.surface1), style: Self.dashes))
            HStack(spacing: Metric.pad) {
                if message.handed {
                    Text("Queued; \(conversation.agent?.title ?? "the Agent") reads it at its next step").foregroundStyle(flavour(.overlay1))
                    Button("Interrupt and send now") { Task { await conversation.interrupt() } }
                } else if conversation.starting {
                    Text("Sent once \(conversation.agent?.title ?? "the Agent") is up").foregroundStyle(flavour(.overlay1))
                    Button("Edit") { edit(conversation.unqueue(message)) }
                    Button("Remove") { _ = conversation.unqueue(message) }
                } else {
                    Text("Sent when \(conversation.agent?.title ?? "the Agent") finishes").foregroundStyle(flavour(.overlay1))
                    Button("Edit") { edit(conversation.unqueue(message)) }
                    Button("Remove") { _ = conversation.unqueue(message) }
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

    private static let dashes = StrokeStyle(lineWidth: 1, dash: [Metric.tiny, 3])
}

/// The whole screen for a long message, on the same text as the message box.
private struct Composer: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    @Binding var text: String
    let canSend: Bool
    let send: () -> Void
    @StateObject private var field = PlainField()

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        NavigationStack {
            PlainText(field: field, text: $text)
                .padding(Metric.gap)
                #if os(macOS)
                .frame(minWidth: 640, idealWidth: 760, minHeight: 420, idealHeight: 560)
                #endif
                .background(flavour(.base))
                .navigationTitle("Message")
                .inlineTitle()
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Send") {
                            send()
                            dismiss()
                        }
                        .disabled(!canSend)
                    }
                }
                .onAppear { field.focus() }
        }
        .tint(flavour(.mauve))
    }
}

/// An image from a message, whole, with pinch to zoom.
private struct ImageViewer: View {
    @Environment(\.dismiss) private var dismiss
    let image: PlatformImage
    @State private var scale = 1.0
    @GestureState private var pinch = 1.0
    /// Where a zoomed image has been dragged to, so its edges can be reached.
    @State private var shift = CGSize.zero
    @GestureState private var drag = CGSize.zero

    var body: some View {
        Image(platform: image)
            .resizable()
            .scaledToFit()
            .scaleEffect(scale * pinch)
            .offset(x: shift.width + drag.width, y: shift.height + drag.height)
            .gesture(MagnifyGesture().updating($pinch) { value, pinch, _ in pinch = value.magnification }
                .onEnded {
                    scale = max(1, scale * $0.magnification)
                    if scale == 1 { shift = .zero }
                })
            .simultaneousGesture(DragGesture().updating($drag) { value, drag, _ in if scale > 1 { drag = value.translation } }
                .onEnded { value in if scale > 1 { shift.width += value.translation.width; shift.height += value.translation.height } })
            .onTapGesture(count: 2) {
                withAnimation {
                    scale = scale > 1 ? 1 : 2
                    shift = .zero
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(.black)
            .overlay(alignment: .topTrailing) {
                Button { dismiss() } label: {
                    Image.lucide("x", size: Metric.title).foregroundStyle(.white)
                        .frame(width: Metric.control, height: Metric.control)
                        .background(.white.opacity(0.2), in: .circle)
                }
                .padding(Metric.wide)
                .keyboardShortcut(.cancelAction)
                .accessibilityLabel("Close")
            }
            .overlay(alignment: .topLeading) {
                ShareLink(item: Image(platform: image), preview: SharePreview("Image", image: Image(platform: image))) {
                    Image(systemName: "square.and.arrow.up").foregroundStyle(.white)
                        .frame(width: Metric.control, height: Metric.control)
                        .background(.white.opacity(0.2), in: .circle)
                }
                .buttonStyle(.plain)
                .padding(Metric.wide)
                .accessibilityLabel("Share")
            }
            #if os(macOS)
            .frame(width: fitted.width, height: fitted.height)
            #endif
    }

    #if os(macOS)
    /// A sheet takes its content's size: the image's own, up to most of the screen.
    private var fitted: CGSize {
        let room = (NSScreen.main?.visibleFrame.size ?? CGSize(width: 1200, height: 800))
        let most = CGSize(width: room.width * 0.9, height: room.height * 0.9)
        let size = image.size
        guard size.width > 0, size.height > 0 else { return most }
        let scale = min(most.width / size.width, most.height / size.height, max(1, 600 / max(size.width, size.height)))
        return CGSize(width: size.width * scale, height: size.height * scale)
    }
    #endif
}

private struct RemoteImage: View {
    @Environment(\.colorScheme) private var colorScheme
    let path: String
    let conversation: Conversation
    @State private var image: PlatformImage?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Group {
            if let image {
                Image(platform: image).resizable().scaledToFill()
            } else {
                Image.lucide("image", size: 20).foregroundStyle(flavour(.overlay1))
            }
        }
        .frame(width: 120, height: 120)
        .background(flavour(.surface0))
        .clipShape(.rect(cornerRadius: 14))
        .task { image = await conversation.image(path) }
        .onTapGesture { conversation.viewing = image }
        .accessibilityLabel((path as NSString).lastPathComponent)
        .accessibilityAddTraits(.isButton)
    }
}

/// An Agent's markdown, GitHub-flavoured, in the app's colours.
struct Markdown: View {
    @Environment(\.colorScheme) private var colorScheme
    let text: String

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        MarkdownUI.Markdown(text)
            .markdownTheme(theme)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var theme: Theme {
        Theme.basic
            .text {
                ForegroundColor(flavour(.text))
                FontSize(Metric.body)
            }
            .code {
                FontFamilyVariant(.monospaced)
                FontSize(.em(0.9))
            }
            .link { ForegroundColor(flavour(.blue)) }
            .codeBlock { block in
                ScrollView(.horizontal, showsIndicators: false) {
                    block.label
                        .markdownTextStyle {
                            FontFamilyVariant(.monospaced)
                            FontSize(Metric.caption)
                        }
                        .padding(Metric.pad)
                }
                .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
                .markdownMargin(top: 0, bottom: Metric.pad)
            }
            .table { table in
                ScrollView(.horizontal, showsIndicators: false) {
                    table.label
                        .markdownTableBorderStyle(.init(color: flavour(.surface1)))
                        .markdownTableBackgroundStyle(.alternatingRows(.clear, flavour(.mantle)))
                }
                .markdownMargin(top: 0, bottom: Metric.pad)
            }
            .tableCell { cell in
                cell.label
                    .markdownTextStyle {
                        FontDigitVariant(.monospaced)
                        if cell.row == 0 { FontWeight(.semibold) }
                    }
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: Metric.measure, alignment: .leading)
                    .padding(.vertical, Metric.tiny)
                    .padding(.horizontal, Metric.gap)
            }
    }
}

private struct Thinking: View {
    @Environment(\.colorScheme) private var colorScheme
    let entry: SessionLog.Entry
    @Binding var open: Bool

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button { open.toggle() } label: {
                HStack(spacing: 6) {
                    Image.lucide("brain", size: 14)
                    Text(entry.seconds.map { "Thought for \(Int($0.rounded()))s" } ?? "Thought")
                    if entry.text?.isEmpty == false {
                        Image.lucide(open ? "chevron-down" : "chevron-right", size: 12)
                    }
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

/// In place of the message box once the Agent has stopped: its Conversation stays readable.
private struct EndedBar: View {
    @Environment(\.colorScheme) private var colorScheme
    let agent: String
    let resume: () async -> Void
    @State private var resuming = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        HStack(spacing: Metric.gap) {
            Text("This Agent session has ended.").font(.ui(Metric.label)).foregroundStyle(flavour(.subtext0))
            Spacer(minLength: 0)
            Button {
                resuming = true
                Task {
                    await resume()
                    resuming = false
                }
            } label: {
                HStack(spacing: Metric.tiny) {
                    if resuming { ProgressView().controlSize(.small) }
                    Text(resuming ? "Resuming…" : "Resume \(agent)")
                }
                .font(.ui(Metric.label).weight(.medium))
                .foregroundStyle(flavour(.base))
                .padding(.horizontal, Metric.pad)
                .padding(.vertical, Metric.gap)
                .background(flavour(.mauve), in: .capsule)
            }
            .buttonStyle(.plain)
            .disabled(resuming)
        }
        .padding(.horizontal, Metric.pad)
        .padding(.vertical, Metric.gap)
        .background(flavour(.mantle), ignoresSafeAreaEdges: .vertical)
    }
}

/// Twinkling stars and a word with a light sweeping across it, while the Agent works.
/// "Move to bottom", while the end of the chat is out of view.
private struct MoveToBottom: View {
    @ObservedObject var scroll: ChatScroll
    let flavour: Catppuccin.Flavour
    private static let width: CGFloat = 320

    var body: some View {
        if !scroll.atBottom {
            Button { scroll.jump() } label: {
                Text("Move to bottom ↓")
                    .font(.ui(Metric.note).weight(.medium))
                    .foregroundStyle(flavour(.text))
                    .frame(maxWidth: Self.width)
                    .padding(.vertical, Metric.gap)
                    .background(flavour(.surface1), in: .capsule)
                    .shadow(radius: ConversationView.shadow)
            }
            .buttonStyle(.plain)
            .padding(Metric.pad)
        }
    }
}

private struct WorkingRow: View {
    @Environment(\.colorScheme) private var colorScheme
    @State private var start = Date()
    @State private var word = Self.words.randomElement() ?? "Working"
    /// The status line on the Agent's screen, which the row shows when there is one.
    var status = ""
    private var label: String { status.isEmpty ? "\(word)…" : status }

    private static let words = [
        "Shimmying", "Pondering", "Noodling", "Percolating", "Tinkering", "Conjuring", "Mulling",
        "Brewing", "Untangling", "Whirring", "Simmering", "Puzzling", "Spelunking", "Cogitating",
    ]
    private static let sweep = 1.6
    private static let band = 0.3

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        TimelineView(.animation) { context in
            let elapsed = context.date.timeIntervalSince(start)
            let light = elapsed.truncatingRemainder(dividingBy: Self.sweep) / Self.sweep * (1 + 2 * Self.band) - Self.band
            HStack(spacing: Metric.gap) {
                Sparkles(time: elapsed, colour: flavour(.mauve)).frame(width: Metric.control / 2, height: Metric.control / 2)
                Text(label)
                    .foregroundStyle(flavour(.overlay1))
                    .overlay {
                        LinearGradient(
                            colors: [.clear, flavour(.text), .clear],
                            startPoint: UnitPoint(x: light - Self.band, y: 0),
                            endPoint: UnitPoint(x: light + Self.band, y: 0)
                        )
                        .mask { Text(label) }
                    }
            }
            .font(.ui(Metric.label).monospacedDigit())
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
    }
}

/// Three four-point stars, each swelling, turning and fading on its own beat.
private struct Sparkles: View {
    let time: TimeInterval
    let colour: Color

    /// Centre and size as fractions of the box, and where in the beat each star starts.
    private static let stars: [(x: Double, y: Double, size: Double, offset: Double)] = [
        (0.42, 0.55, 0.62, 0), (0.8, 0.2, 0.32, 0.33), (0.18, 0.15, 0.26, 0.66),
    ]
    private static let beat = 1.4
    private static let waist = 0.18

    var body: some View {
        Canvas { context, size in
            for star in Self.stars {
                let phase = (time / Self.beat + star.offset).truncatingRemainder(dividingBy: 1)
                let swell = sin(phase * .pi)
                let radius = min(size.width, size.height) * star.size / 2 * swell
                guard radius > 0 else { continue }
                var star4 = context
                star4.translateBy(x: size.width * star.x, y: size.height * star.y)
                star4.rotate(by: .radians(phase * .pi / 2))
                star4.opacity = swell
                star4.fill(Self.path(radius), with: .color(colour))
            }
        }
    }

    private static func path(_ radius: Double) -> Path {
        var path = Path()
        let inner = radius * waist
        for point in 0..<8 {
            let angle = Double(point) * .pi / 4 - .pi / 2
            let length = point.isMultiple(of: 2) ? radius : inner
            let corner = CGPoint(x: cos(angle) * length, y: sin(angle) * length)
            if point == 0 { path.move(to: corner) } else { path.addLine(to: corner) }
        }
        path.closeSubpath()
        return path
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
    /// A button beside the header, outside the one that opens the card.
    var side: (label: String, icon: String, run: () -> Void)?
    @Binding var open: Bool
    @ViewBuilder let header: () -> Header
    @ViewBuilder let detail: () -> Detail
    var opened: () -> Void = {}

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.pad) {
            HStack(alignment: .top, spacing: Metric.pad) {
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
                if let side {
                    Button(action: side.run) {
                        Image.lucide(side.icon, size: Metric.label).foregroundStyle(flavour(.overlay1))
                            .padding(Metric.gap)
                            .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .padding(-Metric.gap)
                    .accessibilityLabel(side.label)
                    .help(side.label)
                }
            }
            if open { detail().frame(maxWidth: .infinity, alignment: .leading) }
        }
        .padding(Metric.pad)
        .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
    }
}

private struct ToolCard: View {
    @Environment(\.colorScheme) private var colorScheme
    let entry: SessionLog.Entry
    let result: SessionLog.Entry?
    let conversation: Conversation

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var failed: Bool { result?.error == true }
    private var open: Binding<Bool> { conversation.isOpen(entry.id) }

    var body: some View {
        switch entry.tool {
        case "edit", "write":
            Card(icon: entry.tool == "edit" ? "file-pen" : "file-plus", tint: failed ? .red : .blue,
                 opens: result?.diff != nil || result?.text != nil, side: openSide, open: open) {
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
            Card(icon: "terminal", tint: failed ? .red : .green, opens: result != nil, open: open) {
                VStack(alignment: .leading, spacing: Metric.tiny) {
                    if let description = entry.description {
                        Text(description).font(.ui(Metric.note)).foregroundStyle(flavour(.subtext0))
                    }
                    Text(entry.command ?? entry.summary)
                        .font(.system(size: Metric.caption, design: .monospaced)).foregroundStyle(flavour(.text)).lineLimit(3)
                }
            } detail: {
                VStack(alignment: .leading, spacing: Metric.gap) {
                    // The header cuts a long command after three lines.
                    if let command = entry.command, command.split(separator: "\n").count > 3 || command.count > 240 {
                        Text(command).font(.system(size: Metric.caption, design: .monospaced)).foregroundStyle(flavour(.text))
                            .textSelection(.enabled)
                    }
                    output
                }
            } opened: { expand() }
        case "task":
            Card(icon: "bot", tint: .mauve, opens: result != nil, side: subagentSide, open: open) {
                VStack(alignment: .leading, spacing: Metric.tiny) {
                    Text("Task").font(.ui(Metric.caption)).foregroundStyle(flavour(.subtext0))
                    Text(entry.description ?? entry.summary).font(.ui(Metric.label)).foregroundStyle(flavour(.text))
                }
            } detail: {
                Markdown(text: result?.text ?? "")
            } opened: { expand() }
        default:
            Card(icon: "wrench", tint: failed ? .red : .overlay1, opens: result?.text != nil, open: open) {
                Text(entry.summary).font(.ui(Metric.label)).foregroundStyle(flavour(.text))
            } detail: { output } opened: { expand() }
        }
    }

    /// Claude's subagents keep their own Transcripts, which open in a Tab of their own.
    private var subagentSide: (label: String, icon: String, run: () -> Void)? {
        guard let open = conversation.openSubagent, let range = entry.id.range(of: ".call.") else { return nil }
        let call = String(entry.id[range.upperBound...])
        let title = entry.description ?? entry.summary
        return ("Open the subagent", "external-link", { open(call, title) })
    }

    private var openSide: (label: String, icon: String, run: () -> Void)? {
        guard let open = conversation.openPath, let file = entry.file else { return nil }
        return ("Open \((file as NSString).lastPathComponent)", "external-link", { open(file) })
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
    let entries: [SessionLog.Entry]
    @Binding var open: Bool
    let openPath: ((String) -> Void)?

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
        Card(icon: "search", open: $open) {
            Text(line).font(.ui(14)).foregroundStyle(flavour(.text))
        } detail: {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(entries) { entry in
                    let text = Text(entry.file ?? entry.command ?? entry.summary)
                        .font(.system(size: 11, design: .monospaced))
                        .lineLimit(2)
                    if let file = entry.file, let openPath {
                        Button { openPath(file) } label: { text.foregroundStyle(flavour(.blue)).multilineTextAlignment(.leading) }
                            .buttonStyle(.plain)
                            .help("Open \((file as NSString).lastPathComponent)")
                    } else {
                        text.foregroundStyle(flavour(.subtext1))
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

// MARK: Questions and permissions

private struct QuestionCard: View {
    @Environment(\.colorScheme) private var colorScheme
    let entry: SessionLog.Entry
    let result: SessionLog.Entry?
    let conversation: Conversation
    @State private var picked: [Int: Set<String>]
    @State private var typed: [Int: String]
    @State private var sending = false
    @State private var problem: String?

    init(entry: SessionLog.Entry, result: SessionLog.Entry?, conversation: Conversation) {
        self.entry = entry
        self.result = result
        self.conversation = conversation
        let kept = conversation.answers[entry.id]
        _picked = State(initialValue: kept?.picked ?? [:])
        _typed = State(initialValue: kept?.typed ?? [:])
    }

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
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

    private func choice(_ option: SessionLog.Question.Option, multi: Bool, in index: Int) -> some View {
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
        // Sent, the card waits for its answer to show; a second tap would answer twice.
        if problem != nil { sending = false }
    }
}

private struct PermissionCard: View {
    @Environment(\.colorScheme) private var colorScheme
    let permission: SessionLog.Permission
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
            if let options = permission.options {
                Text(permission.summary).font(.ui(Metric.note)).foregroundStyle(flavour(.text)).textSelection(.enabled)
                ForEach(options, id: \.self) { choice in
                    Button { Task { await pick(choice) } } label: {
                        HStack {
                            Text("\(choice.key). \(choice.label)").frame(maxWidth: .infinity, alignment: .leading)
                            hint(choice.key.count == 1 ? "⌥⌘" + choice.key : nil)
                        }
                    }
                    .buttonStyle(.bordered)
                    .tint(flavour(choice == options.first ? .mauve : .overlay1))
                }
                .disabled(answering)
            } else {
                HStack(spacing: Metric.pad) {
                    Button { Task { await answer(false) } } label: {
                        HStack { Text("Deny"); hint("⌘.") }.frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)
                    .tint(flavour(.red))
                    Button { Task { await answer(true) } } label: {
                        HStack { Text("Allow"); hint("⌘↩") }.frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(flavour(.mauve))
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

    /// Answered, the card stays until the Agent moves on; a second tap would land as a keystroke.
    private func pick(_ choice: SessionLog.Permission.Choice) async {
        answering = true
        await conversation.choose(choice)
        if conversation.problem != nil { answering = false }
    }

    private func answer(_ allow: Bool) async {
        answering = true
        await conversation.permit(allow)
        if conversation.problem != nil { answering = false }
    }
}

#if os(macOS)
/// The open permission's answers as keys, which work wherever the card is scrolled to; typed
/// keys alone go to the message box.
private struct PermissionKeys: View {
    let permission: SessionLog.Permission
    let conversation: Conversation
    @State private var answering = false

    var body: some View {
        ZStack {
            if let options = permission.options {
                ForEach(options, id: \.self) { choice in
                    if choice.key.count == 1, let key = choice.key.first {
                        Button("") { answer { await conversation.choose(choice) } }
                            .keyboardShortcut(KeyEquivalent(key), modifiers: [.command, .option])
                    }
                }
            } else {
                Button("") { answer { await conversation.permit(true) } }.keyboardShortcut(.return)
                Button("") { answer { await conversation.permit(false) } }.keyboardShortcut(".")
            }
        }
        .hidden()
    }

    private func answer(_ send: @escaping () async -> Void) {
        guard !answering else { return }
        answering = true
        Task {
            await send()
            if conversation.problem != nil { answering = false }
        }
    }
}
#endif

private struct TodoBar: View {
    @Environment(\.colorScheme) private var colorScheme
    let items: [SessionLog.Todo]
    @State private var open = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var done: Int { items.filter { $0.status == "completed" }.count }
    private var current: SessionLog.Todo? {
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

#if os(iOS)
private struct UploadButton: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var link: HostLink
    let pick: () -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Button {
            if link.uploading != nil { link.cancelUpload() } else { pick() }
        } label: {
            Group {
                if let fraction = link.uploading?.fraction {
                    UploadRing(fraction: fraction, colour: flavour(.mauve))
                } else {
                    Image.lucide("image-up", size: 20)
                }
            }
            .foregroundStyle(flavour(.subtext0))
            .frame(width: 38, height: 38)
        }
        .disabled(link.uploading == nil && !link.canUpload)
        .accessibilityLabel(link.uploading == nil ? "upload" : "cancel upload")
        .uploadFailure($link.uploadError)
    }
}
#endif
