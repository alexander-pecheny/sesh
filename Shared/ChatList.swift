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
    let spacing: CGFloat
    let inset: CGFloat

    /// The row that shows earlier pages are loading.
    static let earlier = "earlier"

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
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

        var parent: ChatList?
        private weak var scroll: NSScrollView?
        private weak var table: NSTableView?
        private var items: [Item] = []
        private var heights: [String: (version: Int, width: CGFloat, height: CGFloat)] = [:]
        private let sizer = NSHostingController(rootView: AnyView(EmptyView()))
        private var jumps = 0
        private var revealed: String?
        private var atBottom = true
        private var adjusting = false

        func attach(scroll: NSScrollView, table: NSTableView) {
            self.scroll = scroll
            self.table = table
            table.dataSource = self
            table.delegate = self
            scroll.contentView.postsBoundsChangedNotifications = true
            NotificationCenter.default.addObserver(self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification, object: scroll.contentView)
            NotificationCenter.default.addObserver(self, selector: #selector(resized), name: NSView.frameDidChangeNotification, object: scroll)
        }

        private var width: CGFloat { max(1, table?.tableColumns.first?.width ?? scroll?.contentSize.width ?? 1) }

        /// Takes the new rows and keeps the reader where they were: at the end if they were
        /// there, else on the row at the top of the view.
        func update() {
            guard let parent, let table, let scroll else { return }
            let anchor = atBottom ? nil : topRow()
            items = parent.items
            adjusting = true
            table.reloadData()
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
            } else if atBottom {
                toEnd()
            } else if let anchor, let row = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: row, offset: anchor.offset)
            }
            if parent.reveal == nil { revealed = nil }
            scroll.reflectScrolledClipView(scroll.contentView)
            report()
            #if DEBUG
            Self.trace("update rows=\(items.count) anchor=\(anchor.map { "\($0.id)@\($0.offset)" } ?? "-") bottom=\(atBottom) table=\(table.bounds.height) view=\(scroll.contentView.bounds.minY) now=\(topRow().map { "\($0.id)@\($0.offset)" } ?? "-")")
            #endif
        }

        #if DEBUG
        /// `-listLog PATH` traces every update, for finding where the reader was moved.
        private static let log = UserDefaults.standard.string(forKey: "listLog").flatMap { path -> FileHandle? in
            FileManager.default.createFile(atPath: path, contents: nil)
            return FileHandle(forWritingAtPath: path)
        }

        static func trace(_ line: String) {
            log?.write(Data("\(Date().timeIntervalSince1970) \(line)\n".utf8))
        }
        #endif

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
            report()
            #if DEBUG
            Self.trace("scrolled view=\(scroll?.contentView.bounds.minY ?? 0) bottom=\(atBottom)")
            #endif
        }

        /// The window or the sidebar changed the width: every row wraps anew.
        @objc private func resized() {
            let anchor = atBottom ? nil : topRow()
            table?.noteHeightOfRows(withIndexesChanged: IndexSet(integersIn: 0..<items.count))
            table?.tile()
            if atBottom {
                toEnd()
            } else if let anchor, let row = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: row, offset: anchor.offset)
            }
        }

        /// Whether the reader is at the end and near the top, as the reader alone moves them.
        private func report() {
            guard let table, let scroll else { return }
            let visible = scroll.contentView.bounds
            let bottom = visible.maxY >= table.bounds.height + scroll.contentInsets.bottom - Self.edge
            let top = visible.minY < Self.top
            atBottom = bottom
            guard let parent else { return }
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
            if let known = heights[item.id], known.version == item.version, known.width == width { return known.height }
            sizer.rootView = item.view
            let height = max(1, ceil(sizer.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude)).height))
            heights[item.id] = (item.version, width, height)
            return height
        }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard row < items.count else { return nil }
            let item = items[row]
            let cell = tableView.makeView(withIdentifier: Self.cell, owner: self) as? Cell ?? Cell()
            cell.identifier = Self.cell
            cell.show(item.view, width: width) { [weak self] height in self?.grew(item.id, to: height) }
            return cell
        }

        func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }

        /// A row that changed its own size, such as a card opened, keeps the size it took.
        private func grew(_ id: String, to height: CGFloat) {
            guard let row = items.firstIndex(where: { $0.id == id }), let known = heights[id],
                  abs(known.height - height) > 0.5 else { return }
            let anchor = atBottom ? nil : topRow()
            heights[id] = (known.version, known.width, height)
            table?.noteHeightOfRows(withIndexesChanged: IndexSet(integer: row))
            table?.tile()
            if atBottom {
                toEnd()
            } else if let anchor, let top = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: top, offset: anchor.offset)
            }
        }
    }

    /// A row's SwiftUI view, which says when its content wants another height.
    final class Cell: NSView {
        private let host = Host(rootView: AnyView(EmptyView()))
        /// The table's width, held on the view itself so its fitting size answers for it.
        private lazy var width = host.widthAnchor.constraint(equalToConstant: 1)

        override init(frame: NSRect) {
            super.init(frame: frame)
            host.translatesAutoresizingMaskIntoConstraints = false
            addSubview(host)
            NSLayoutConstraint.activate([
                host.leadingAnchor.constraint(equalTo: leadingAnchor),
                host.topAnchor.constraint(equalTo: topAnchor),
                width,
            ])
        }

        required init?(coder: NSCoder) { fatalError("not from a nib") }

        func show(_ view: AnyView, width: CGFloat, grew: @escaping (CGFloat) -> Void) {
            self.width.constant = width
            host.rootView = view
            host.grew = { [weak self] in
                guard let self else { return }
                grew(ceil(self.host.fittingSize.height))
            }
        }
    }

    final class Host: NSHostingView<AnyView> {
        var grew: (() -> Void)?

        required init(rootView: AnyView) {
            super.init(rootView: rootView)
            sizingOptions = [.intrinsicContentSize]
        }

        @MainActor required init?(coder: NSCoder) { fatalError("not from a nib") }

        override func invalidateIntrinsicContentSize() {
            super.invalidateIntrinsicContentSize()
            let grew = grew
            DispatchQueue.main.async { grew?() }
        }
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
    let spacing: CGFloat
    let inset: CGFloat

    static let earlier = "earlier"

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
        private var revealed: String?
        private var atBottom = true
        private var adjusting = false
        private var width: CGFloat = 0

        func attach(_ table: UITableView) {
            self.table = table
            table.dataSource = self
            table.delegate = self
        }

        func update() {
            guard let parent, let table else { return }
            let anchor = atBottom ? nil : topRow()
            items = parent.items
            adjusting = true
            table.reloadData()
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
            } else if atBottom {
                toEnd()
            } else if let anchor, let row = items.firstIndex(where: { $0.id == anchor.id }) {
                place(row: row, offset: anchor.offset)
            }
            if parent.reveal == nil { revealed = nil }
            report()
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
            guard let table, let parent else { return }
            let bottom = table.contentOffset.y + table.bounds.height >= table.contentSize.height + table.adjustedContentInset.bottom - Self.edge
            let top = table.contentOffset.y + table.adjustedContentInset.top < Self.top
            atBottom = bottom
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
            sizer.rootView = item.view
            let height = max(1, ceil(sizer.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude)).height))
            heights[item.id] = (item.version, width, height)
            return height + spacing
        }

        private var spacing: CGFloat { parent?.spacing ?? 0 }

        func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
            let cell = tableView.dequeueReusableCell(withIdentifier: Self.cell, for: indexPath)
            guard let cell = cell as? Cell, indexPath.row < items.count else { return cell }
            let item = items[indexPath.row]
            cell.show(item.view) { [weak self] height in self?.grew(item.id, to: height) }
            return cell
        }

        /// A row that changed its own size, such as a card opened, keeps the size it took.
        private func grew(_ id: String, to height: CGFloat) {
            guard let table, let known = heights[id], abs(known.height - height) > 0.5 else { return }
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

    /// A row's SwiftUI view, which says when its content wants another height.
    final class Cell: UITableViewCell {
        private let host = UIHostingController(rootView: AnyView(EmptyView()))
        private var grew: ((CGFloat) -> Void)?
        private var reported: CGFloat = 0

        override init(style: UITableViewCell.CellStyle, reuseIdentifier: String?) {
            super.init(style: style, reuseIdentifier: reuseIdentifier)
            backgroundColor = .clear
            host.view.backgroundColor = .clear
            host.sizingOptions = [.intrinsicContentSize]
            host.view.translatesAutoresizingMaskIntoConstraints = false
            contentView.addSubview(host.view)
            NSLayoutConstraint.activate([
                host.view.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
                host.view.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
                host.view.topAnchor.constraint(equalTo: contentView.topAnchor),
            ])
        }

        required init?(coder: NSCoder) { fatalError("not from a nib") }

        func show(_ view: AnyView, grew: @escaping (CGFloat) -> Void) {
            host.rootView = view
            reported = 0
            self.grew = grew
        }

        /// The content asked for another size, which lays the cell out again: measure it.
        override func layoutSubviews() {
            super.layoutSubviews()
            let height = ceil(host.sizeThatFits(in: CGSize(width: contentView.bounds.width, height: .greatestFiniteMagnitude)).height)
            if reported == 0 {
                reported = height
            } else if abs(height - reported) > 0.5 {
                reported = height
                let grew = grew
                DispatchQueue.main.async { grew?(height) }
            }
        }
    }
}
#endif
