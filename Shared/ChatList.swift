#if os(macOS)
import AppKit
import SwiftUI

/// The Conversation as a native list (ADR 0013): a table whose rows host the chat's SwiftUI
/// views, each measured once at the table's width and kept, and scrolled by `ChatScroll`.
struct ChatList: NSViewRepresentable {
    let items: [Item]
    let scroll: ChatScroll
    /// Whether the Conversation has its opening batch, so the rows can be shown.
    var ready = true
    let spacing: CGFloat
    let inset: CGFloat

    func makeCoordinator() -> Coordinator { Coordinator(spacing: spacing) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = Scroll()
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        let table = NSTableView()
        let column = NSTableColumn(identifier: Coordinator.column)
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.backgroundColor = .clear
        table.style = .plain
        table.selectionHighlightStyle = .none
        table.allowsColumnSelection = false
        table.intercellSpacing = .zero
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.gridStyleMask = []
        scroll.contentInsets = NSEdgeInsets(top: inset, left: 0, bottom: inset, right: 0)
        scroll.documentView = table
        context.coordinator.parent = self
        context.coordinator.attach(scroll: scroll, table: table)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.parent = self
        context.coordinator.update()
    }

    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate, ChatSurface {
        static let column = NSUserInterfaceItemIdentifier("row")
        private static let cell = NSUserInterfaceItemIdentifier("cell")

        var parent: ChatList?
        private weak var scrollView: NSScrollView?
        private weak var table: NSTableView?
        private var items: [Item] = []
        private let heights: Heights
        private var adjusting = false
        /// The size the rows were last laid out at.
        private var measured = CGSize.zero
        /// Whether the reader is moving the view: a wheel tick not yet seen, or a scroller dragged.
        private var wheeled = false
        private var live = false

        init(spacing: CGFloat) { heights = Heights(spacing: spacing) }

        /// The keeper, while this list is the one showing its chat.
        private var keeper: ChatScroll? { parent.flatMap { $0.scroll.surface === self ? $0.scroll : nil } }

        func attach(scroll: NSScrollView, table: NSTableView) {
            scrollView = scroll
            self.table = table
            table.dataSource = self
            table.delegate = self
            scroll.contentView.postsBoundsChangedNotifications = true
            NotificationCenter.default.addObserver(self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification, object: scroll.contentView)
            NotificationCenter.default.addObserver(self, selector: #selector(resized), name: NSView.frameDidChangeNotification, object: scroll)
            NotificationCenter.default.addObserver(self, selector: #selector(resized), name: NSTableView.columnDidResizeNotification, object: table)
            NotificationCenter.default.addObserver(self, selector: #selector(dragging), name: NSScrollView.willStartLiveScrollNotification, object: scroll)
            NotificationCenter.default.addObserver(self, selector: #selector(dropped), name: NSScrollView.didEndLiveScrollNotification, object: scroll)
            (scroll as? Scroll)?.wheeled = { [weak self] in self?.wheeled = true }
            parent?.scroll.attach(self)
        }

        @objc private func dragging() { live = true }
        @objc private func dropped() { live = false }

        private var width: CGFloat { max(1, table?.tableColumns.first?.width ?? scrollView?.contentSize.width ?? 1) }

        func update() {
            guard let parent, let table, let keeper else { return }
            if size != measured { resized() }
            let new = parent.items
            let change = Change(old: items.map { ($0.id, $0.version) }, new: new.map { ($0.id, $0.version) })
            keeper.update(new.map(\.id), ready: parent.ready, changed: change != .none) {
                items = new
                quietly {
                    guard case .rows(let changed, let added, let removed) = change else { return }
                    table.beginUpdates()
                    if !removed.isEmpty { table.removeRows(at: removed, withAnimation: []) }
                    if !added.isEmpty { table.insertRows(at: added, withAnimation: []) }
                    table.endUpdates()
                    if !changed.isEmpty {
                        table.reloadData(forRowIndexes: changed, columnIndexes: [0])
                        table.noteHeightOfRows(withIndexesChanged: changed)
                    }
                    // The frame takes the new rows' heights only on layout; a scroll before it is clamped.
                    table.tile()
                }
            }
            items = new
        }

        private func quietly(_ change: () -> Void) {
            adjusting = true
            change()
            adjusting = false
        }

        @objc private func scrolled() {
            guard !adjusting else { return }
            let reader = live || wheeled
            wheeled = false
            keeper?.scrolled(byReader: reader)
        }

        /// The window, the sidebar or the message box changed the size; a new width wraps every
        /// row anew. The scroll view resizes before its column does, and only the column counts.
        @objc private func resized() {
            guard size != measured, let keeper else { return }
            let wider = size.width != measured.width
            measured = size
            keeper.resized {
                guard wider, let table else { return }
                quietly {
                    table.noteHeightOfRows(withIndexesChanged: IndexSet(integersIn: 0..<items.count))
                    table.tile()
                }
            }
        }

        // MARK: ChatSurface

        var size: CGSize { CGSize(width: width, height: scrollView?.contentView.bounds.height ?? 0) }
        var offset: CGFloat { scrollView?.contentView.bounds.minY ?? 0 }
        var start: CGFloat { -(scrollView?.contentInsets.top ?? 0) }

        var end: CGFloat {
            guard let table, let scrollView else { return 0 }
            return table.bounds.height - scrollView.contentView.bounds.height + scrollView.contentInsets.bottom
        }

        var visible: Range<Int> {
            guard let table, let scrollView else { return 0..<0 }
            return Range(table.rows(in: scrollView.contentView.bounds)) ?? 0..<0
        }

        func top(of row: Int) -> CGFloat { table?.rect(ofRow: row).minY ?? 0 }

        func scroll(to offset: CGFloat) {
            guard let scrollView else { return }
            quietly { scrollView.contentView.scroll(to: NSPoint(x: 0, y: offset)) }
            scrollView.reflectScrolledClipView(scrollView.contentView)
        }

        func show(_ shown: Bool) { scrollView?.alphaValue = shown ? 1 : 0 }

        // MARK: rows

        func numberOfRows(in tableView: NSTableView) -> Int { items.count }

        func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
            row < items.count ? heights.of(items[row], width: width) : 1
        }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard row < items.count else { return nil }
            let item = items[row]
            let cell = tableView.makeView(withIdentifier: Self.cell, owner: self) as? Cell ?? Cell()
            cell.identifier = Self.cell
            cell.show(ChatList.laidOut(item.view) { [weak self] size in self?.grew(item.id, to: size) })
            return cell
        }

        func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }

        /// A row that changed its own size, such as a card opened, keeps the size it took.
        private func grew(_ id: String, to size: CGSize) {
            guard let row = items.firstIndex(where: { $0.id == id }), heights.grew(id, to: size), let table else { return }
            keeper?.grew {
                quietly {
                    table.noteHeightOfRows(withIndexesChanged: IndexSet(integer: row))
                    table.tile()
                }
            }
        }
    }

    /// Tells a wheel or trackpad scroll, which only the reader makes, from a layout's.
    final class Scroll: NSScrollView {
        var wheeled: (() -> Void)?

        override func scrollWheel(with event: NSEvent) {
            wheeled?()
            super.scrollWheel(with: event)
        }
    }

    /// A row's SwiftUI view, filling the row the list measured for it.
    final class Cell: NSView {
        private let host = NSHostingView(rootView: AnyView(EmptyView()))

        override init(frame: NSRect) {
            super.init(frame: frame)
            host.sizingOptions = []
            host.translatesAutoresizingMaskIntoConstraints = false
            addSubview(host)
            NSLayoutConstraint.activate([
                host.leadingAnchor.constraint(equalTo: leadingAnchor),
                host.trailingAnchor.constraint(equalTo: trailingAnchor),
                host.topAnchor.constraint(equalTo: topAnchor),
                host.bottomAnchor.constraint(equalTo: bottomAnchor),
            ])
        }

        required init?(coder: NSCoder) { fatalError("not from a nib") }

        func show(_ view: AnyView) { host.rootView = view }
    }
}

private typealias Hosting = NSHostingController<AnyView>
#else
import SwiftUI
import UIKit

/// The Conversation as a native list (ADR 0013): a table whose rows host the chat's SwiftUI
/// views, each measured once at the table's width and kept, and scrolled by `ChatScroll`.
struct ChatList: UIViewRepresentable {
    let items: [Item]
    let scroll: ChatScroll
    /// Whether the Conversation has its opening batch, so the rows can be shown.
    var ready = true
    let spacing: CGFloat
    let inset: CGFloat

    func makeCoordinator() -> Coordinator { Coordinator(spacing: spacing) }

    func makeUIView(context: Context) -> UITableView {
        let table = UITableView(frame: .zero, style: .plain)
        table.separatorStyle = .none
        table.backgroundColor = .clear
        table.allowsSelection = false
        table.estimatedRowHeight = 0
        table.estimatedSectionHeaderHeight = 0
        table.estimatedSectionFooterHeight = 0
        table.keyboardDismissMode = .interactive
        table.contentInset = UIEdgeInsets(top: inset, left: 0, bottom: inset, right: 0)
        table.register(Cell.self, forCellReuseIdentifier: Coordinator.cell)
        context.coordinator.parent = self
        context.coordinator.attach(table)
        return table
    }

    func updateUIView(_ table: UITableView, context: Context) {
        context.coordinator.parent = self
        context.coordinator.update()
    }

    @MainActor
    final class Coordinator: NSObject, UITableViewDataSource, UITableViewDelegate, ChatSurface {
        static let cell = "cell"

        var parent: ChatList?
        private weak var table: UITableView?
        private var items: [Item] = []
        private let heights: Heights
        private var adjusting = false
        private var sizing: NSKeyValueObservation?
        /// The size the rows were last laid out at.
        private var measured = CGSize.zero
        /// Whether a tap on the status bar is taking the reader to the top.
        private var toTop = false

        init(spacing: CGFloat) { heights = Heights(spacing: spacing) }

        /// The keeper, while this list is the one showing its chat.
        private var keeper: ChatScroll? { parent.flatMap { $0.scroll.surface === self ? $0.scroll : nil } }

        func attach(_ table: UITableView) {
            self.table = table
            table.dataSource = self
            table.delegate = self
            sizing = table.observe(\.bounds) { [weak self] _, _ in
                MainActor.assumeIsolated { self?.resized() }
            }
            parent?.scroll.attach(self)
        }

        /// The keyboard, a growing message box or a turn took some of the size. Scrolling moves
        /// the bounds too, so only a new size counts.
        private func resized() {
            guard size != measured, let keeper else { return }
            measured = size
            keeper.resized()
        }

        func scrollViewDidChangeAdjustedContentInset(_ scrollView: UIScrollView) { keeper?.resized() }

        func update() {
            guard let parent, let table, let keeper else { return }
            let old = items, new = parent.items
            let change = Change(old: old.map { ($0.id, $0.version) }, new: new.map { ($0.id, $0.version) })
            keeper.update(new.map(\.id), ready: parent.ready, changed: change != .none) {
                items = new
                quietly {
                    defer { table.layoutIfNeeded() }
                    guard case .rows(let changed, let added, let removed) = change else { return }
                    // UIKit aborts the app on a batch that does not add up, so anything it may not
                    // have counted the way the old rows did is reloaded whole instead.
                    guard table.window != nil, table.numberOfRows(inSection: 0) == old.count else { return table.reloadData() }
                    UIView.performWithoutAnimation {
                        if !added.isEmpty || !removed.isEmpty {
                            table.performBatchUpdates {
                                table.deleteRows(at: removed.map { IndexPath(row: $0, section: 0) }, with: .none)
                                table.insertRows(at: added.map { IndexPath(row: $0, section: 0) }, with: .none)
                            }
                        }
                        // Numbered after the insert, and refreshed in place, keeping their cells.
                        if !changed.isEmpty {
                            table.performBatchUpdates { table.reconfigureRows(at: changed.map { IndexPath(row: $0, section: 0) }) }
                        }
                    }
                }
            }
            items = new
        }

        private func quietly(_ change: () -> Void) {
            adjusting = true
            change()
            adjusting = false
        }

        func scrollViewDidScroll(_ scrollView: UIScrollView) {
            guard !adjusting else { return }
            keeper?.scrolled(byReader: scrollView.isTracking || scrollView.isDecelerating || toTop)
        }

        func scrollViewShouldScrollToTop(_ scrollView: UIScrollView) -> Bool {
            toTop = true
            return true
        }

        func scrollViewDidScrollToTop(_ scrollView: UIScrollView) { toTop = false }

        // MARK: ChatSurface

        var size: CGSize { table?.bounds.size ?? .zero }
        var offset: CGFloat { table?.contentOffset.y ?? 0 }
        var start: CGFloat { -(table?.adjustedContentInset.top ?? 0) }

        var end: CGFloat {
            guard let table else { return 0 }
            return table.contentSize.height + table.adjustedContentInset.bottom - table.bounds.height
        }

        var visible: Range<Int> {
            guard let table else { return 0..<0 }
            let rows = (table.indexPathsForRows(in: CGRect(origin: CGPoint(x: 0, y: offset - start), size: size)) ?? []).map(\.row)
            guard let first = rows.min(), let last = rows.max() else { return 0..<0 }
            return first..<last + 1
        }

        func top(of row: Int) -> CGFloat { (table?.rectForRow(at: IndexPath(row: row, section: 0)).minY ?? 0) + start }

        func scroll(to offset: CGFloat) {
            quietly { table?.contentOffset = CGPoint(x: 0, y: offset) }
        }

        func show(_ shown: Bool) { table?.alpha = shown ? 1 : 0 }

        // MARK: rows

        func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { items.count }

        func tableView(_ tableView: UITableView, heightForRowAt indexPath: IndexPath) -> CGFloat {
            indexPath.row < items.count ? heights.of(items[indexPath.row], width: tableView.bounds.width) : 1
        }

        func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
            let cell = tableView.dequeueReusableCell(withIdentifier: Self.cell, for: indexPath)
            guard let cell = cell as? Cell, indexPath.row < items.count else { return cell }
            let item = items[indexPath.row]
            cell.show(ChatList.laidOut(item.view) { [weak self] size in self?.grew(item.id, to: size) })
            return cell
        }

        /// A row that changed its own size, such as a card opened, keeps the size it took.
        private func grew(_ id: String, to size: CGSize) {
            guard let table, heights.grew(id, to: size) else { return }
            keeper?.grew {
                quietly {
                    UIView.performWithoutAnimation {
                        table.beginUpdates()
                        table.endUpdates()
                    }
                }
            }
        }
    }

    /// A row's SwiftUI view, filling the row the list measured for it.
    final class Cell: UITableViewCell {
        private let host = UIHostingController(rootView: AnyView(EmptyView()))

        override init(style: UITableViewCell.CellStyle, reuseIdentifier: String?) {
            super.init(style: style, reuseIdentifier: reuseIdentifier)
            backgroundColor = .clear
            host.view.backgroundColor = .clear
            host.sizingOptions = []
            host.view.translatesAutoresizingMaskIntoConstraints = false
            contentView.addSubview(host.view)
            NSLayoutConstraint.activate([
                host.view.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
                host.view.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
                host.view.topAnchor.constraint(equalTo: contentView.topAnchor),
                host.view.bottomAnchor.constraint(equalTo: contentView.bottomAnchor),
            ])
        }

        required init?(coder: NSCoder) { fatalError("not from a nib") }

        func show(_ view: AnyView) { host.rootView = view }
    }
}

private typealias Hosting = UIHostingController<AnyView>
#endif

extension ChatList {
    struct Item: Identifiable {
        let id: String
        /// Changes whenever what the row shows changes, so its height is measured again.
        let version: Int
        let view: AnyView
    }

    /// A row as both the measuring and the shown copy lay it out: the list's width, its own
    /// height, from the top. The shown copy reports the height it takes, which changes when a
    /// card opens or an image loads.
    static func laidOut(_ view: AnyView, report: ((CGSize) -> Void)?) -> AnyView {
        let fixed = view.fixedSize(horizontal: false, vertical: true)
        guard let report else { return AnyView(fixed) }
        return AnyView(
            fixed
                .onGeometryChange(for: CGSize.self) { $0.size } action: { size in report(size) }
                .frame(maxHeight: .infinity, alignment: .top))
    }

    /// Each row's height, measured once as a cell lays it out and kept under its id, its version
    /// and the width, with the spacing below it. Before the list has its width, nothing is kept.
    @MainActor
    final class Heights {
        private let spacing: CGFloat
        private var known: [String: (version: Int, width: CGFloat, height: CGFloat)] = [:]
        private let sizer = Hosting(rootView: AnyView(EmptyView()))

        init(spacing: CGFloat) { self.spacing = spacing }

        func of(_ item: Item, width: CGFloat) -> CGFloat {
            guard width >= ChatScroll.narrowest else { return 1 }
            if let known = known[item.id], known.version == item.version, known.width == width { return known.height + spacing }
            sizer.rootView = ChatList.laidOut(item.view, report: nil)
            let height = max(1, ceil(sizer.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude)).height))
            known[item.id] = (item.version, width, height)
            return height + spacing
        }

        /// Takes the size a shown row reports, and whether it differs from the kept one.
        func grew(_ id: String, to size: CGSize) -> Bool {
            let height = ceil(size.height)
            // A cell laid out before it has its width, or for another row, reports nonsense.
            guard let old = known[id], abs(size.width - old.width) < 1, abs(old.height - height) > 0.5 else { return false }
            known[id] = (old.version, old.width, height)
            return true
        }
    }
}

/// What a list must redo for new rows: nothing, or rows removed (by their old place), added
/// and redrawn (by their new place). Only those rows' cells are touched, so the others keep
/// their views, and with them a selection or a scroll inside them.
enum Change: Equatable {
    case none
    case rows(changed: IndexSet, added: IndexSet, removed: IndexSet)

    init(old: [(String, Int)], new: [(String, Int)]) {
        var added = IndexSet(), removed = IndexSet()
        for step in new.map(\.0).difference(from: old.map(\.0)) {
            switch step {
            case .insert(let offset, _, _): added.insert(offset)
            case .remove(let offset, _, _): removed.insert(offset)
            }
        }
        let versions = Dictionary(old.map { ($0.0, $0.1) }, uniquingKeysWith: { $1 })
        var changed = IndexSet()
        for (index, row) in new.enumerated() where !added.contains(index) && versions[row.0] != row.1 { changed.insert(index) }
        self = changed.isEmpty && added.isEmpty && removed.isEmpty ? .none : .rows(changed: changed, added: added, removed: removed)
    }
}
