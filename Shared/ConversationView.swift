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
    @State private var atBottom = true
    @State private var nearTop = false
    @State private var position = ScrollPosition(idType: String.self)
    /// Counts the user's sends: whoever sends wants to see the reply, wherever they had scrolled.
    @State private var sent = 0
    /// A row a link scrolled to, held at the top until the text around it has settled.
    @State private var pinned: (row: String, until: Date)?
    /// Counts the requests to go to the end, for the native list.
    @State private var jumps = 0

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var working: Bool { conversation.state == "working" }
    /// Grows whenever something lands at the bottom.
    private var changes: Int {
        conversation.items.count + conversation.permissions.count + conversation.queued.count + (working ? 1 : 0)
            + conversation.live.reduce(0) { $0 + ($1.text ?? $1.summary).count }
    }
    private static let nearTop = 200.0
    private static let rowSpacing: CGFloat = 14
    private static let focusTint = 0.15
    private static let shadow: CGFloat = 3
    private static let jumpWidth: CGFloat = 320

    /// Pages back while the reader stays near the top and herdr has more.
    private func loadEarlier() async {
        while nearTop, conversation.earlier, !conversation.items.isEmpty {
            let count = conversation.items.count
            await conversation.loadEarlier()
            if conversation.items.count == count { return }
            // The view says where the reader is only once it has laid the page out.
            try? await Task.sleep(for: .milliseconds(150))
        }
    }

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
        .overlay(alignment: .bottom) {
            if !atBottom {
                Button { jump() } label: {
                    Text("Move to bottom ↓")
                        .font(.ui(Metric.note).weight(.medium))
                        .foregroundStyle(flavour(.text))
                        .frame(maxWidth: Self.jumpWidth)
                        .padding(.vertical, Metric.gap)
                        .background(flavour(.surface1), in: .capsule)
                        .shadow(radius: Self.shadow)
                }
                .buttonStyle(.plain)
                .padding(Metric.pad)
            }
        }
        .onChange(of: sent) { jump() }
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
        .onChange(of: draft) { conversation.draft = draft }
        #if os(macOS)
        .onChange(of: hidden) { field.catchesTyping = !hidden }
        #endif
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

    @ViewBuilder private var chat: some View {
        #if os(macOS)
        if Conversation.useLog {
            ChatList(items: listItems, atBottom: $atBottom, nearTop: $nearTop, jumps: jumps, reveal: revealedRow,
                     spacing: Self.rowSpacing, inset: Metric.wide)
                .onChange(of: nearTop) { if nearTop { Task { await loadEarlier() } } }
        } else {
            scrolling
        }
        #else
        scrolling
        #endif
    }

    private var scrolling: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: Self.rowSpacing) {
                if conversation.earlier { ProgressView().frame(maxWidth: .infinity) }
                if conversation.starting {
                    HStack(spacing: Metric.gap) {
                        ProgressView().controlSize(.small)
                        Text("Starting \(conversation.agent?.title ?? "the Agent")… Anything you send now goes once it is up.")
                    }
                    .font(.ui(Metric.label)).foregroundStyle(flavour(.overlay1))
                    .frame(maxWidth: .infinity)
                    .padding(.top, Metric.wide)
                } else if conversation.loaded, conversation.items.isEmpty, conversation.pane != nil {
                    Text("\(conversation.agent?.title ?? "The Agent") is ready. Its Conversation starts with your first message.")
                        .font(.ui(Metric.label)).foregroundStyle(flavour(.overlay1))
                        .frame(maxWidth: .infinity)
                        .padding(.top, Metric.wide)
                }
                ForEach(Row.rows(conversation.shown, key: conversation.rowKey)) { row in rowView(row) }
                ForEach(conversation.permissions) { PermissionCard(permission: $0, conversation: conversation) }
                ForEach(conversation.queued) { QueuedBubble(message: $0, conversation: conversation, edit: takeBack) }
                // Waiting on background work, Claude's own status line says on what.
                if working || conversation.state == "background" && !conversation.status.isEmpty {
                    WorkingRow(status: conversation.status)
                }
            }
            .scrollTargetLayout()
            .padding(Metric.wide)
            // An opening card with an unbroken path asks for more than the screen; never give it.
            .fitWidth()
        }
        // Tracking the top row keeps it in place while a page lands above it.
        .scrollPosition($position, anchor: .top)
        .defaultScrollAnchor(.bottom)
        .onScrollGeometryChange(for: Bool.self) { geometry in
            geometry.contentOffset.y + geometry.contentInsets.top < Self.nearTop
        } action: { _, top in
            nearTop = top
            if top { Task { await loadEarlier() } }
        }
        .onScrollGeometryChange(for: Edge.self) { geometry in
            Edge(height: geometry.contentSize.height, top: geometry.visibleRect.minY, bottom: geometry.visibleRect.maxY)
        } action: { old, new in
            // The reader left the end when the view, keeping its size, rose by more than the
            // content shrank: rows above settling move both alike.
            let rose = new.top - old.top, shrank = new.height - old.height
            let resized = abs((new.bottom - new.top) - (old.bottom - old.top)) > 1
            // While a revealed row is held in place, the view is where the link put it.
            let holding = pinned.map { $0.until > .now } ?? false
            // A view left wholly past the content's end, by a jump measured while covered, shows
            // nothing; it goes back to the end.
            if new.top > new.height, new.height > 0 { position.scrollTo(edge: .bottom) }
            if holding {
            } else if new.bottom >= new.height - Metric.control {
                atBottom = true
            } else if !resized, rose < -1, rose < shrank - 1 {
                atBottom = false
                return
            }
            // New content, or text that has only now measured its height, moves the end away
            // without the reader moving: follow it if the end was in view.
            guard new.height != old.height else { return }
            if holding, let pinned {
                position.scrollTo(id: pinned.row, anchor: .top)
            } else if atBottom {
                position.scrollTo(edge: .bottom)
            }
        }
        .onChange(of: conversation.focus) { showFocus() }
        .animation(.easeOut(duration: 1), value: conversation.focus)
        .onChange(of: changes) {
            // A Conversation shorter than the screen never scrolls to the top to ask.
            if nearTop { Task { await loadEarlier() } }
            if atBottom { position.scrollTo(edge: .bottom) }
        }
    }

    private func rowView(_ row: Row) -> some View {
        RowView(row: row, conversation: conversation)
            .padding(Metric.tiny)
            .background(
                conversation.focus.map(row.contains) == true ? flavour(.yellow).opacity(Self.focusTint) : .clear,
                in: .rect(cornerRadius: Metric.corner))
            .bookmarkable(row.entry, live: row.live, keep: confirmed(conversation.bookmark, "Bookmarked in the Journal"),
                          copy: confirmed(conversation.copyLink, "Link copied"), column: row.prose ? Metric.proseColumn : nil)
    }

    #if os(macOS)
    /// The row the focused entry is in, for the native list to bring to the top.
    private var revealedRow: String? {
        guard let focus = conversation.focus else { return nil }
        return Row.rows(conversation.shown, key: conversation.rowKey).first { $0.contains(focus) }?.id
    }

    /// Everything the native list shows, each row versioned by what it draws.
    private var listItems: [ChatList.Item] {
        func item(_ id: String, _ version: Int, _ view: some View) -> ChatList.Item {
            ChatList.Item(id: id, version: version, view: AnyView(
                view.frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, Metric.wide).environment(\.openURL, links)))
        }
        var items: [ChatList.Item] = []
        if conversation.earlier { items.append(item(ChatList.earlier, 0, ProgressView().frame(maxWidth: .infinity))) }
        for row in Row.rows(conversation.shown, key: conversation.rowKey) {
            var hasher = Hasher()
            switch row.content {
            case .item(.entry(let entry)):
                hasher.combine(entry)
                hasher.combine(entry.call.flatMap { conversation.results[$0] })
            case .item(.switched(_, let reason)): hasher.combine(reason)
            case .lookups(let entries): hasher.combine(entries)
            }
            hasher.combine(conversation.focus.map(row.contains))
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
            items.append(item("working", 0, WorkingRow(status: conversation.status)))
        }
        return items
    }
    #endif

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
                Image.lucide(working && !canSend ? "square" : "arrow-up", size: 18)
                    .foregroundStyle(flavour(.base))
                    .frame(width: Metric.control, height: Metric.control)
                    .background(flavour(working && !canSend ? .red : .mauve).opacity(canSend || working ? 1 : 0.4), in: .circle)
            }
            .disabled(!canSend && !working)
            .accessibilityLabel(working && !canSend ? "Stop" : "Send")
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
        .onChange(of: composing) { if !composing, atBottom { jump() } }
        .cover(isPresented: $composing) {
            Composer(text: $draft, canSend: canSend) { Task { await send() } }
        }
    }

    /// One jump lands short while the rows on the way still guess their heights, so it is
    /// repeated as they measure, unless the reader scrolls away meanwhile.
    private func jump() {
        atBottom = true
        jumps += 1
        position.scrollTo(edge: .bottom)
        Task {
            for delay in [50, 150, 400] {
                try? await Task.sleep(for: .milliseconds(delay))
                guard atBottom else { return }
                position.scrollTo(edge: .bottom)
            }
        }
    }

    /// Scrolls to the item a link pointed at, set before this view existed or while it shows.
    private func showFocus() {
        guard let focus = conversation.focus,
              let row = Row.rows(conversation.items, key: conversation.rowKey).first(where: { $0.contains(focus) }) else { return }
        atBottom = false
        // Text above it is still measuring its height; keep the row in place meanwhile.
        pinned = (row.id, .now + 2)
        position.scrollTo(id: row.id, anchor: .top)
    }

    private func appeared() {
        if conversation.focus != nil { DispatchQueue.main.async { showFocus() } }
        if draft.isEmpty { draft = conversation.draft }
        if fresh { DispatchQueue.main.async { field.focus() } }
        #if os(macOS)
        let conversation = conversation
        field.pasteImage = { [weak conversation] data, ext in await conversation?.upload(data, ext: ext) }
        field.catchesTyping = !hidden
        #endif
    }

    /// An action on a message, followed by a word that it happened.
    private func confirmed(_ action: ((Conversation.Entry) -> Void)?, _ words: String) -> ((Conversation.Entry) -> Void)? {
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

    /// A queued message goes back into the box, ahead of anything typed since.
    private func takeBack(_ text: String) {
        draft = draft.isEmpty ? text : text + "\n\n" + draft
    }

    private var canSend: Bool { !sending && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    /// While the Agent works, a written message queues and an empty box stops the Agent.
    private func sendOrStop() async {
        guard !working || canSend else { return await conversation.stop() }
        await send()
    }

    private func send() async {
        sending = true
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        if let problem = await conversation.send(text) { conversation.problem = problem } else {
            draft = ""
            sent += 1
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

    var body: some View {
        #if os(macOS)
        Prose(text: text)
        #else
        Markdown(text: text)
        #endif
    }
}

/// A Bookmark, or a copied link, from a row's context menu and, where there is a pointer,
/// icons on hover: text that can be selected keeps its own context menu.
private struct Bookmarkable: ViewModifier {
    @Environment(\.colorScheme) private var colorScheme
    let entry: Conversation.Entry?
    /// A live item, which the Transcript will hold soon: it takes the room its icons will need.
    let live: Bool
    let keep: ((Conversation.Entry) -> Void)?
    let copy: ((Conversation.Entry) -> Void)?
    /// How wide a reply's text column is, so the icon sits at its corner, not the window's.
    let column: CGFloat?
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

    func body(content: Content) -> some View {
        if let entry, let keep {
            content
                .padding(.trailing, Self.gutter)
                .contextMenu {
                    Button("Bookmark in the Journal") { keep(entry) }
                    if let copy { Button("Copy Link") { copy(entry) } }
                }
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
    fileprivate func bookmarkable(_ entry: Conversation.Entry?, live: Bool, keep: ((Conversation.Entry) -> Void)?,
                                  copy: ((Conversation.Entry) -> Void)?, column: CGFloat?) -> some View {
        modifier(Bookmarkable(entry: entry, live: live, keep: keep, copy: copy, column: column))
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
            .popover(isPresented: $listing, arrowEdge: .top) { list }
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

/// How tall the Conversation is and where the visible part ends.
private struct Edge: Equatable {
    let height: CGFloat
    let top: CGFloat
    let bottom: CGFloat
}

/// What the list shows: one entry, or a run of reads, searches and fetches as one line.
private struct Row: Identifiable {
    enum Content {
        case item(Conversation.Item)
        case lookups([Conversation.Entry])
    }

    var content: Content
    /// The first entry's row: an entry that replaced a live item keeps that item's row.
    let id: String

    func contains(_ id: String) -> Bool {
        switch content {
        case .item(let item): item.id == id
        case .lookups(let entries): entries.contains { $0.id == id }
        }
    }

    /// A reply set in the prose column, rather than a card, a bubble or a table, which takes
    /// the whole width and caps each cell instead.
    var prose: Bool {
        guard case .item(.entry(let entry)) = content, entry.kind == "text" else { return false }
        return !Cmark.hasTable(entry.text ?? "")
    }

    var live: Bool { Conversation.isLive(id) }

    /// The entry a Bookmark of this row keeps; a live item is not in the Transcript yet.
    var entry: Conversation.Entry? {
        let entry: Conversation.Entry? = switch content {
        case .item(.entry(let entry)): entry
        case .lookups(let entries): entries.first
        case .item(.switched): nil
        }
        return entry.flatMap { Conversation.isLive($0.id) ? nil : $0 }
    }

    static func rows(_ items: [Conversation.Item], key: (String) -> String) -> [Row] {
        var rows: [Row] = []
        for item in items {
            guard case .entry(let entry) = item, entry.kind == "tool",
                  ["read", "search", "fetch"].contains(entry.tool) else {
                rows.append(Row(content: .item(item), id: key(item.id)))
                continue
            }
            if case .lookups(let run)? = rows.last?.content {
                rows[rows.count - 1].content = .lookups(run + [entry])
            } else {
                rows.append(Row(content: .lookups([entry]), id: key(entry.id)))
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
        switch row.content {
        case .lookups(let entries): Lookups(entries: entries)
        case .item(.switched(_, let reason)): SwitchDivider(reason: reason)
        case .item(.entry(let entry)):
            switch entry.kind {
            case "user": UserBubble(entry: entry, conversation: conversation)
            case "text":
                let text = conversation.openPath == nil && conversation.repo == nil
                    ? entry.text ?? entry.summary : PathLinks.link(entry.text ?? entry.summary, repo: conversation.repo)
                let pieces = Cmark.pieces(text)
                VStack(alignment: .leading, spacing: Metric.tiny) {
                    ForEach(pieces.indices, id: \.self) { index in
                        AgentText(text: pieces[index].text).readable(wide: pieces[index].table)
                    }
                    Stamp(at: entry.at).readable()
                }
            case "thinking": Thinking(entry: entry)
            case "tool": ToolCard(entry: entry, result: conversation.results[entry.id], conversation: conversation)
            case "question": QuestionCard(entry: entry, result: conversation.results[entry.id], conversation: conversation)
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
    let entry: Conversation.Entry
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
                } else {
                    Text("Sent when \(conversation.agent?.title ?? "the Agent") finishes").foregroundStyle(flavour(.overlay1))
                    Button("Edit") { edit(conversation.unqueue(message)) }
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

    var body: some View {
        Image(platform: image)
            .resizable()
            .scaledToFit()
            .scaleEffect(scale * pinch)
            .gesture(MagnifyGesture().updating($pinch) { value, pinch, _ in pinch = value.magnification }
                .onEnded { scale = max(1, scale * $0.magnification) })
            .onTapGesture(count: 2) { withAnimation { scale = scale > 1 ? 1 : 2 } }
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
    @ViewBuilder let header: () -> Header
    @ViewBuilder let detail: () -> Detail
    var opened: () -> Void = {}
    @State private var open = false

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
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(side.label)
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
    let entry: Conversation.Entry
    let result: Conversation.Entry?
    let conversation: Conversation

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var failed: Bool { result?.error == true }

    var body: some View {
        switch entry.tool {
        case "edit", "write":
            Card(icon: entry.tool == "edit" ? "file-pen" : "file-plus", tint: failed ? .red : .blue,
                 opens: result?.diff != nil || result?.text != nil, side: openSide) {
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
            Card(icon: "bot", tint: .mauve, opens: result != nil, side: subagentSide) {
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
            .frame(maxWidth: .infinity, alignment: .leading)
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
            if let options = permission.options {
                Text(permission.summary).font(.ui(Metric.note)).foregroundStyle(flavour(.text)).textSelection(.enabled)
                ForEach(options, id: \.self) { choice in
                    Button { Task { await pick(choice) } } label: {
                        Text("\(choice.key). \(choice.label)").frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(.bordered)
                    .tint(flavour(choice == options.first ? .mauve : .overlay1))
                }
                .disabled(answering)
            } else {
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
        if permission.options != nil { return "\(agent) is asking" }
        return permission.summary
    }

    private func pick(_ choice: Conversation.Permission.Choice) async {
        answering = true
        await conversation.choose(choice)
        answering = false
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
        .accessibilityLabel("upload")
        .uploadFailure($link.uploadError)
    }
}
#endif
