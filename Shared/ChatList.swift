#if os(macOS)
import AppKit
import SwiftUI

/// The Conversation as a native list (ADR 0013): a table whose rows host the chat's SwiftUI
/// views, each measured once at the table's width and kept, and scrolled by rules of its own.
struct ChatList: NSViewRepresentable {
    struct Item: Identifiable {
        let id: String
        /// Changes whenever what the row shows changes, so its height is measured again.
        let version: Int
        let view: AnyView
    }

    let items: [Item]
    @Binding var atBottom: Bool
    @Binding var nearTop: Bool
    /// Counts the requests to go to the end, from "Move to bottom" and from sending.
    let jumps: Int
    /// The row to bring to the top, for a link or a search result.
    let reveal: String?
    let spot: Spot
    /// Changes when the reader opens or closes a card, which keeps the rows in view where they
    /// are, even at the end, so the card opens under the pointer rather than scrolling away.
    var hold = 0
    let spacing: CGFloat
    let inset: CGFloat

    /// The row that shows earlier pages are loading.
    static let earlier = "earlier"

    /// The row at the top of the view and its offset, or nil at the end.
    final class Spot {
        var anchor: (id: String, offset: CGFloat)?
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

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
        table.intercellSpacing = NSSize(width: 0, height: spacing)
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.gridStyleMask = []
        scroll.contentInsets = NSEdgeInsets(top: inset, left: 0, bottom: inset, right: 0)
        scroll.documentView = table
        context.coordinator.attach(scroll: scroll, table: table)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.parent = self
        context.coordinator.update()
    }

    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        static let column = NSUserInterfaceItemIdentifier("row")
        private static let cell = NSUserInterfaceItemIdentifier("cell")
        /// How close to the end still counts as at the end, and to the top as near it.
        private static let edge: CGFloat = 40
        private static let top: CGFloat = 400
        private static let narrowest: CGFloat = 120

        var parent: ChatList?
        private weak var scroll: NSScrollView?
        private weak var table: NSTableView?
        private var items: [Item] = []
        private var heights: [String: (version: Int, width: CGFloat, height: CGFloat)] = [:]
        /// Measures rows laid out as a cell lays them out, so a cached height is the height drawn.
        private let sizer = NSHostingController(rootView: AnyView(EmptyView()))
        private var jumps = 0
        private var held = 0
        private var revealed: String?
        private var atBottom = true
        private var restored = false
        private var adjusting = false

        func attach(scroll: NSScrollView, table: NSTableView) {
            self.scroll = scroll
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
        }

        /// Whether the reader is moving the view: a wheel tick not yet seen, or a scroller dragged.
        private var wheeled = false
        private var live = false
        @objc private func dragging() { live = true }
        @objc private func dropped() { live = false }

        /// The column width the rows' heights were last measured at.
        private var measured: CGFloat = 0

        private var width: CGFloat { max(1, table?.tableColumns.first?.width ?? scroll?.contentSize.width ?? 1) }

        /// Takes the new rows and keeps the reader where they were: at the end if they were
        /// there, else on the row at the top of the view.
        func update() {
            guard let parent, let table, let scroll else { return }
            if width != measured { resized() }
            let holding = parent.hold != held
            held = parent.hold
            var anchor = atBottom && !holding ? nil : topRow()
            if let kept = restore(parent.items) { anchor = kept }
            let old = items
            items = parent.items
            let change = Change(old: old.map { ($0.id, $0.version) }, new: items.map { ($0.id, $0.version) })
            if case .none = change, parent.jumps == jumps, parent.reveal == revealed { return }
            adjusting = true
            switch change {
            case .none: break
            case .rows(let changed, let added, let removed):
                table.beginUpdates()
                if !removed.isEmpty { table.removeRows(at: removed, withAnimation: []) }
                if !added.isEmpty { table.insertRows(at: added, withAnimation: []) }
                table.endUpdates()
                if !changed.isEmpty {
                    table.reloadData(forRowIndexes: changed, columnIndexes: [0])
                    table.noteHeightOfRows(withIndexesChanged: changed)
                }
            }
            // The frame takes the new rows' heights only on layout; a scroll before it is clamped.
            table.tile()
            adjusting = false
            if parent.jumps != jumps {
                jumps = parent.jumps
                atBottom = true
            }
            if let reveal = parent.reveal, reveal != revealed, let row = items.firstIndex(where: { $0.id == reveal }) {
                revealed = reveal
                atBottom = false
                place(row: row, offset: 0)
            } else if atBottom && !holding {
                toEnd()
            } else if let anchor, let row = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: row, offset: anchor.offset)
            }
            if parent.reveal == nil { revealed = nil }
            scroll.reflectScrolledClipView(scroll.contentView)
            report()
            Self.trace("update rows=\(items.count) anchor=\(anchor.map { "\($0.id)@\($0.offset)" } ?? "-") bottom=\(atBottom) table=\(table.bounds.height) view=\(scroll.contentView.bounds.minY) now=\(topRow().map { "\($0.id)@\($0.offset)" } ?? "-")")
        }

        /// `-listLog PATH` traces every update, for finding where the reader was moved.
        private static let log = UserDefaults.standard.string(forKey: "listLog").flatMap { path -> FileHandle? in
            FileManager.default.createFile(atPath: path, contents: nil)
            return FileHandle(forWritingAtPath: path)
        }

        static func trace(_ line: @autoclosure () -> String) {
            guard let log else { return }
            log.write(Data("\(Date().timeIntervalSince1970) \(line())\n".utf8))
        }

        /// Where the reader left this chat, taken once, when the rows can first be measured.
        private func restore(_ items: [Item]) -> (id: String, offset: CGFloat)? {
            guard !restored, !items.isEmpty, width >= Self.narrowest else { return nil }
            restored = true
            Self.trace("restore rows=\(items.count) spot=\(parent?.spot.anchor.map { "\($0.id)@\($0.offset)" } ?? "-")")
            guard let kept = parent?.spot.anchor, items.contains(where: { $0.id == kept.id }) else {
                atBottom = true
                return nil
            }
            atBottom = false
            return kept
        }

        /// The first message in view and how far its top sits below the view's. The spinner
        /// above the first message never counts: held in place, it would keep pages loading.
        private func topRow() -> (id: String, offset: CGFloat)? {
            guard let table, let scroll else { return nil }
            let visible = scroll.contentView.bounds
            let rows = table.rows(in: visible)
            guard let row = (rows.lowerBound..<rows.upperBound).first(where: { $0 < items.count && items[$0].id != ChatList.earlier }) else { return nil }
            return (items[row].id, table.rect(ofRow: row).minY - visible.minY)
        }

        private func place(row: Int, offset: CGFloat) {
            guard let table, let scroll else { return }
            let y = table.rect(ofRow: row).minY - offset
            adjusting = true
            scroll.contentView.scroll(to: NSPoint(x: 0, y: max(-scroll.contentInsets.top, y)))
            adjusting = false
        }

        private func toEnd() {
            guard let table, let scroll else { return }
            let end = table.bounds.height - scroll.contentView.bounds.height + scroll.contentInsets.bottom
            adjusting = true
            scroll.contentView.scroll(to: NSPoint(x: 0, y: max(-scroll.contentInsets.top, end)))
            adjusting = false
        }

        @objc private func scrolled() {
            guard !adjusting else { return }
            let reader = live || wheeled
            wheeled = false
            // Rows measured late make the list taller and shift the view; only the reader leaves the end.
            if atBottom && !reader {
                toEnd()
                return
            }
            report()
            Self.trace("scrolled view=\(scroll?.contentView.bounds.minY ?? 0) bottom=\(atBottom)")
        }

        /// The window or the sidebar changed the width: every row wraps anew.
        @objc private func resized() {
            // The scroll view resizes before its column does; only the column's width counts.
            guard width != measured else { return }
            measured = width
            let kept = restore(items)
            let bottom = atBottom
            let anchor = kept ?? (bottom ? nil : topRow())
            adjusting = true
            table?.noteHeightOfRows(withIndexesChanged: IndexSet(integersIn: 0..<items.count))
            table?.tile()
            adjusting = false
            if bottom {
                toEnd()
            } else if let anchor, let row = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: row, offset: anchor.offset)
            }
            if kept != nil { report() }
        }

        /// Whether the reader is at the end and near the top, as the reader alone moves them.
        private func report() {
            guard let table, let scroll else { return }
            let visible = scroll.contentView.bounds
            guard visible.height > 0 else { return }
            let bottom = visible.maxY >= table.bounds.height + scroll.contentInsets.bottom - Self.edge
            let top = visible.minY < Self.top
            atBottom = bottom
            guard let parent else { return }
            if restored { parent.spot.anchor = bottom ? nil : topRow() }
            Self.trace("report bottom=\(bottom) restored=\(restored) spot=\(parent.spot.anchor.map { "\($0.id)@\($0.offset)" } ?? "-") table=\(table.bounds.height) view=\(visible.minY)/\(visible.height)")
            if parent.atBottom != bottom || parent.nearTop != top {
                DispatchQueue.main.async {
                    if parent.atBottom != bottom { parent.atBottom = bottom }
                    if parent.nearTop != top { parent.nearTop = top }
                }
            }
        }

        // MARK: rows

        func numberOfRows(in tableView: NSTableView) -> Int { items.count }

        func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
            guard row < items.count else { return 1 }
            let item = items[row]
            let width = width
            // Before the column has its width, any height would be wrong and kept.
            guard width >= Self.narrowest else { return 1 }
            if let known = heights[item.id], known.version == item.version, known.width == width { return known.height }
            sizer.rootView = ChatList.laidOut(item.view, report: nil)
            let height = max(1, ceil(sizer.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude)).height))
            heights[item.id] = (item.version, width, height)
            return height
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
            let height = ceil(size.height)
            // A cell laid out before it has its width, or for another row, reports nonsense.
            guard let row = items.firstIndex(where: { $0.id == id }), let known = heights[id],
                  abs(size.width - known.width) < 1, abs(known.height - height) > 0.5 else { return }
            Self.trace("grew \(id) \(known.height) -> \(height) at \(size.width)")
            let bottom = atBottom
            let anchor = bottom ? nil : topRow()
            heights[id] = (known.version, known.width, height)
            adjusting = true
            table?.noteHeightOfRows(withIndexesChanged: IndexSet(integer: row))
            table?.tile()
            adjusting = false
            if bottom {
                toEnd()
            } else if let anchor, let top = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: top, offset: anchor.offset)
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
#else
import SwiftUI
import UIKit

/// The Conversation as a native list (ADR 0013): a table whose rows host the chat's SwiftUI
/// views, each measured once at the table's width and kept, and scrolled by rules of its own.
struct ChatList: UIViewRepresentable {
    struct Item: Identifiable {
        let id: String
        /// Changes whenever what the row shows changes, so its height is measured again.
        let version: Int
        let view: AnyView
    }

    let items: [Item]
    @Binding var atBottom: Bool
    @Binding var nearTop: Bool
    let jumps: Int
    let reveal: String?
    let spot: Spot
    var hold = 0
    let spacing: CGFloat
    let inset: CGFloat

    static let earlier = "earlier"

    final class Spot {
        var anchor: (id: String, offset: CGFloat)?
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

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
        context.coordinator.attach(table)
        return table
    }

    func updateUIView(_ table: UITableView, context: Context) {
        context.coordinator.parent = self
        context.coordinator.update()
    }

    @MainActor
    final class Coordinator: NSObject, UITableViewDataSource, UITableViewDelegate {
        static let cell = "cell"
        private static let edge: CGFloat = 40
        private static let top: CGFloat = 400

        var parent: ChatList?
        private weak var table: UITableView?
        private var items: [Item] = []
        private var heights: [String: (version: Int, width: CGFloat, height: CGFloat)] = [:]
        private let sizer = UIHostingController(rootView: AnyView(EmptyView()))
        private var jumps = 0
        private var held = 0
        private var revealed: String?
        private var atBottom = true
        private var restored = false
        private var adjusting = false
        private var width: CGFloat = 0

        func attach(_ table: UITableView) {
            self.table = table
            table.dataSource = self
            table.delegate = self
            sizing = table.observe(\.bounds) { [weak self] table, _ in
                MainActor.assumeIsolated { self?.sized(table.bounds.height) }
            }
        }

        private var sizing: NSKeyValueObservation?
        private var height: CGFloat = 0

        /// The keyboard or a growing message box took some of the height: the end stays in view.
        private func sized(_ height: CGFloat) {
            guard height != self.height else { return }
            self.height = height
            if atBottom { toEnd() }
        }

        func scrollViewDidChangeAdjustedContentInset(_ scrollView: UIScrollView) {
            if atBottom { toEnd() }
        }

        func update() {
            guard let parent, let table else { return }
            let holding = parent.hold != held
            held = parent.hold
            var anchor = atBottom && !holding ? nil : topRow()
            if let kept = restore(parent.items) { anchor = kept }
            let old = items
            items = parent.items
            let change = Change(old: old.map { ($0.id, $0.version) }, new: items.map { ($0.id, $0.version) })
            if case .none = change, parent.jumps == jumps, parent.reveal == revealed { return }
            adjusting = true
            switch change {
            case .none: break
            case .rows(let changed, let added, let removed):
                // UIKit aborts the app on a batch that does not add up, so anything it may not
                // have counted the way the old rows did is reloaded whole instead.
                guard table.window != nil, table.numberOfRows(inSection: 0) == old.count else {
                    table.reloadData()
                    break
                }
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
            table.layoutIfNeeded()
            adjusting = false
            if parent.jumps != jumps {
                jumps = parent.jumps
                atBottom = true
            }
            if let reveal = parent.reveal, reveal != revealed, let row = items.firstIndex(where: { $0.id == reveal }) {
                revealed = reveal
                atBottom = false
                place(row: row, offset: 0)
            } else if atBottom && !holding {
                toEnd()
            } else if let anchor, let row = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: row, offset: anchor.offset)
            }
            if parent.reveal == nil { revealed = nil }
            report()
        }

        /// Where the reader left this chat, taken once, when the rows can first be measured.
        private func restore(_ items: [Item]) -> (id: String, offset: CGFloat)? {
            guard !restored, !items.isEmpty, (table?.bounds.width ?? 0) > 0 else { return nil }
            restored = true
            guard let kept = parent?.spot.anchor, items.contains(where: { $0.id == kept.id }) else {
                atBottom = true
                return nil
            }
            atBottom = false
            return kept
        }

        /// The first message in view and how far its top sits below the view's.
        private func topRow() -> (id: String, offset: CGFloat)? {
            guard let table else { return nil }
            let top = table.contentOffset.y + table.adjustedContentInset.top
            let visible = CGRect(x: 0, y: top, width: table.bounds.width, height: table.bounds.height)
            let rows = (table.indexPathsForRows(in: visible) ?? []).map(\.row).sorted()
            guard let row = rows.first(where: { $0 < items.count && items[$0].id != ChatList.earlier }) else { return nil }
            return (items[row].id, table.rectForRow(at: IndexPath(row: row, section: 0)).minY - top)
        }

        private func place(row: Int, offset: CGFloat) {
            guard let table else { return }
            let y = table.rectForRow(at: IndexPath(row: row, section: 0)).minY - offset - table.adjustedContentInset.top
            set(y)
        }

        private func toEnd() {
            guard let table else { return }
            set(table.contentSize.height + table.adjustedContentInset.bottom - table.bounds.height)
        }

        private func set(_ y: CGFloat) {
            guard let table else { return }
            let lowest = -table.adjustedContentInset.top
            let highest = max(lowest, table.contentSize.height + table.adjustedContentInset.bottom - table.bounds.height)
            adjusting = true
            table.contentOffset = CGPoint(x: 0, y: min(max(y, lowest), highest))
            adjusting = false
        }

        func scrollViewDidScroll(_ scrollView: UIScrollView) {
            guard !adjusting else { return }
            report()
        }

        private func report() {
            guard let table, let parent, table.bounds.height > 0 else { return }
            let bottom = table.contentOffset.y + table.bounds.height >= table.contentSize.height + table.adjustedContentInset.bottom - Self.edge
            let top = table.contentOffset.y + table.adjustedContentInset.top < Self.top
            atBottom = bottom
            if restored { parent.spot.anchor = bottom ? nil : topRow() }
            if parent.atBottom != bottom || parent.nearTop != top {
                DispatchQueue.main.async {
                    if parent.atBottom != bottom { parent.atBottom = bottom }
                    if parent.nearTop != top { parent.nearTop = top }
                }
            }
        }

        // MARK: rows

        func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { items.count }

        func tableView(_ tableView: UITableView, heightForRowAt indexPath: IndexPath) -> CGFloat {
            guard indexPath.row < items.count else { return 1 }
            let item = items[indexPath.row]
            let width = tableView.bounds.width
            if width != self.width { self.width = width }
            if let known = heights[item.id], known.version == item.version, known.width == width { return known.height + spacing }
            sizer.rootView = ChatList.laidOut(item.view, report: nil)
            let height = max(1, ceil(sizer.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude)).height))
            heights[item.id] = (item.version, width, height)
            return height + spacing
        }

        private var spacing: CGFloat { parent?.spacing ?? 0 }

        func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
            let cell = tableView.dequeueReusableCell(withIdentifier: Self.cell, for: indexPath)
            guard let cell = cell as? Cell, indexPath.row < items.count else { return cell }
            let item = items[indexPath.row]
            cell.show(ChatList.laidOut(item.view) { [weak self] size in self?.grew(item.id, to: size) })
            return cell
        }

        /// A row that changed its own size, such as a card opened, keeps the size it took.
        private func grew(_ id: String, to size: CGSize) {
            let height = ceil(size.height)
            // A cell laid out before it has its width, or for another row, reports nonsense.
            guard let table, let known = heights[id], abs(size.width - known.width) < 1, abs(known.height - height) > 0.5 else { return }
            let anchor = atBottom ? nil : topRow()
            heights[id] = (known.version, known.width, height)
            adjusting = true
            UIView.performWithoutAnimation {
                table.beginUpdates()
                table.endUpdates()
            }
            adjusting = false
            if atBottom {
                toEnd()
            } else if let anchor, let top = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: top, offset: anchor.offset)
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
#endif

/// What a list must redo for new rows: nothing, or rows removed (by their old place), added
/// and redrawn (by their new place). Only those rows' cells are touched, so the others keep
/// their views, and with them a selection or a scroll inside them.
enum Change {
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

extension ChatList {
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
}
