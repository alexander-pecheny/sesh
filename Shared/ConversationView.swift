import MarkdownUI
import SwiftUI

struct ConversationView: View {
    @Environment(\.flavour) private var flavour
    @Environment(\.openURL) private var openURL
    @ObservedObject var conversation: Conversation
    let title: String
    let fresh: Bool
    var hidden = false
    /// Starts the Agent again, for a session whose Agent has stopped.
    var resume: (() async -> Void)?
    /// The branch and machine the Agent works on, which the phone shows under its Tabs.
    var place: String?
    @State private var notice: String?
    @State private var lost = false
    /// A message open on its own for selecting part of it, which the phone's list cannot do.
    @State private var selecting: String?
    /// The rows keep their drawing until their version changes, so a time stamped today
    /// changes to a date only when the day is part of it.
    @State private var today = Calendar.current.startOfDay(for: .now)

    private var working: Bool { conversation.state == "working" }
    private static let rowSpacing: CGFloat = 14
    private static let focusTint = 0.15
    static let shadow: CGFloat = 3

    var body: some View {
        chat
        .overlay(alignment: .bottom) {
            if !conversation.loaded { Skeleton(exchanges: 3).padding(Metric.wide) }
        }
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
                    MessageBox(conversation: conversation, outbox: conversation.outbox, fresh: fresh, hidden: hidden)
                } else if let resume {
                    EndedBar(agent: conversation.agent?.title ?? "The Agent", resume: resume)
                }
            }
        }
        // Not sideways: under macOS 26's floating sidebar the colour would tint its glass.
        .background(flavour(.base), ignoresSafeAreaEdges: .vertical)
        #if os(macOS)
        .navigationTitle(title)
        .navigationSubtitle(stateLabel)
        .buttonStyle(.plain)
        #else
        // The bar is too narrow for the state beside its buttons, and keeps the Task's title.
        .safeAreaInset(edge: .top, spacing: 0) { status }
        #endif
        .toolbar {
            if conversation.agent == .claude, resume == nil {
                ToolbarItem(placement: .primaryAction) {
                    Button { Task { await openInClaude() } } label: { Label("Open in Claude", image: "external-link") }
                }
            }
        }
        .onAppear { conversation.shown() }
        .onDisappear { conversation.hidden() }
        .environment(\.openURL, links)
        .onReceive(NotificationCenter.default.publisher(for: .NSCalendarDayChanged).receive(on: RunLoop.main)) { _ in
            today = Calendar.current.startOfDay(for: .now)
        }
        #if os(macOS)
        .background {
            if !hidden, let permission = conversation.permissions.first {
                PermissionKeys(permission: permission, conversation: conversation).id(permission.id)
            }
        }
        #endif
        .cover(isPresented: Binding(
            get: { conversation.viewing != nil }, set: { if !$0 { conversation.viewing = nil } }
        )) {
            if let image = conversation.viewing { ImageViewer(image: image) }
        }
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
            hasher.combine(row.entries.contains { conversation.answered.contains($0.id) })
            items.append(item(row.id, hasher.finalize(), rowView(row)))
        }
        for permission in conversation.permissions {
            items.append(item("permission." + permission.id, permission.hashValue ^ conversation.answered.contains(permission.id).hashValue, PermissionCard(permission: permission, conversation: conversation)))
        }
        for message in conversation.queued {
            var hasher = Hasher()
            hasher.combine(message)
            hasher.combine(conversation.starting)
            items.append(item(message.id, hasher.finalize(), QueuedBubble(message: message, conversation: conversation)))
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

    #if os(iOS)
    private var status: some View {
        let state = Text(stateLabel)
        let place = place.map { place in
            Label { Text(place).truncationMode(.middle) } icon: { Image.lucide("git-branch", size: Metric.small) }
        }
        return ViewThatFits(in: .horizontal) {
            HStack(spacing: Metric.pad) { state; place }
            VStack(alignment: .leading, spacing: 2) { state; place }
        }
        .lineLimit(1)
        .font(.ui(Metric.small))
        .foregroundStyle(flavour(.subtext0))
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, Metric.wide)
        .padding(.vertical, Metric.tiny)
        .background(flavour(.mantle))
    }
    #endif

    private var stateLabel: String {
        let agent = conversation.agent?.title ?? "Agent"
        if resume != nil { return "Ended" }
        if conversation.starting { return "Starting \(agent)" }
        let over = conversation.runner?.carrier.map { " over \($0)" } ?? ""
        if conversation.syncing { return "Syncing…" + over }
        let state = switch conversation.state {
        case "working": "\(agent) is working"
        case "background": "\(agent) is waiting on background work"
        case "blocked": "\(agent) needs you"
        case "done", "idle": "\(agent) is ready"
        default: agent
        }
        guard let time = conversation.syncTime else { return state }
        let seconds = Double(time.components.seconds) + Double(time.components.attoseconds) / 1e18
        return "\(state). Last sync took \(String(format: "%.3g", seconds))s\(over)"
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
    @Environment(\.flavour) private var flavour
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
                .foregroundStyle(flavour(.overlay1))
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
    @Environment(\.flavour) private var flavour
    let text: String

    var body: some View {
        NavigationStack {
            ScrollView { Prose(text: text).frame(maxWidth: .infinity, alignment: .leading).padding(Metric.wide) }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(flavour(.base), ignoresSafeAreaEdges: .all)
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
    @Environment(\.flavour) private var flavour
    @ObservedObject var conversation: Conversation
    @State private var listing = false

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
    @Environment(\.flavour) private var flavour
    let row: SessionLog.Row
    @ObservedObject var conversation: Conversation

    var body: some View {
        switch row.content {
        case .lookups(let entries): Lookups(entries: entries, open: conversation.isOpen(entries[0].id), openPath: conversation.openPath)
        case .switched(let reason): SwitchDivider(reason: reason)
        case .entry(let entry):
            switch entry.kind {
            case "user": UserBubble(entry: entry, conversation: conversation)
            case "text":
                let text = PathLinks.link(entry.text ?? entry.summary, repo: conversation.repo,
                                          paths: conversation.openPath != nil, rules: conversation.links)
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

/// An image from a message or a Document, whole, with pinch to zoom.
struct ImageViewer: View {
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

/// An Agent's markdown, GitHub-flavoured, in the app's colours.
struct Markdown: View {
    @Environment(\.flavour) private var flavour
    let text: String

    var body: some View {
        MarkdownUI.Markdown(Cmark.linked(text))
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
    @Environment(\.flavour) private var flavour
    let entry: SessionLog.Entry
    @Binding var open: Bool

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
    @Environment(\.flavour) private var flavour
    let agent: String
    let resume: () async -> Void
    @State private var resuming = false

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
    @Environment(\.flavour) private var flavour
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
    @Environment(\.flavour) private var flavour
    let reason: String

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

private struct TodoBar: View {
    @Environment(\.flavour) private var flavour
    let items: [SessionLog.Todo]
    @State private var open = false

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

/// Bars in the shape of a chat, standing in for messages still on their way.
struct Skeleton: View {
    var exchanges: Int
    @Environment(\.flavour) private var flavour
    @State private var faded = false

    private static let line: CGFloat = 12
    private static let reply: [CGFloat] = [0.92, 0.8, 0.55]
    private static let message: CGFloat = 0.4
    private static let pulse = 0.9
    private static let dim = 0.45

    var body: some View {
        let tint = flavour(.surface0)
        VStack(alignment: .leading, spacing: Metric.wide) {
            ForEach(0..<exchanges, id: \.self) { _ in
                bar(Self.message, height: Metric.control, tint, alignment: .trailing)
                VStack(alignment: .leading, spacing: Metric.gap) {
                    ForEach(Self.reply, id: \.self) { bar($0, height: Self.line, tint, alignment: .leading) }
                }
            }
        }
        .opacity(faded ? Self.dim : 1)
        .onAppear { withAnimation(.easeInOut(duration: Self.pulse).repeatForever()) { faded = true } }
        .allowsHitTesting(false)
        .accessibilityLabel("Loading messages")
    }

    private func bar(_ share: CGFloat, height: CGFloat, _ tint: Color, alignment: Alignment) -> some View {
        GeometryReader { box in
            RoundedRectangle(cornerRadius: min(height / 2, Metric.corner)).fill(tint)
                .frame(width: box.size.width * share)
                .frame(maxWidth: .infinity, alignment: alignment)
        }
        .frame(height: height)
    }
}

