import Combine
import CoreGraphics
import Foundation

/// What `ChatScroll` reads of a native list and moves. Offsets are the list's own scroll
/// offsets, so a row's `top(of:)` less `offset` is how far it sits below the top of the view.
@MainActor
protocol ChatSurface: AnyObject {
    /// The width rows are measured at and the height they are shown in; zero before layout.
    var size: CGSize { get }
    var offset: CGFloat { get }
    /// The offsets that show the first row at the top and the last row at the bottom.
    var start: CGFloat { get }
    var end: CGFloat { get }
    /// The rows in view, in order.
    var visible: Range<Int> { get }
    func top(of row: Int) -> CGFloat
    func scroll(to offset: CGFloat)
    func show(_ shown: Bool)
}

/// Where the reader is in a Conversation's list and the rules that keep them there, for both
/// platforms' lists (ADR 0013). At the end, new rows are followed; anywhere else, the row at
/// the top of the view stays where it is. It outlives the list, so a chat reopens where it was.
@MainActor
final class ChatScroll: ObservableObject {
    enum Phase {
        /// The rows cannot be measured yet, so where the reader left the chat is not taken yet.
        case opening
        case following
        case anchored
    }

    typealias Anchor = (id: String, offset: CGFloat)

    /// The row that shows earlier pages are loading.
    static let earlier = "earlier"
    /// How close to the end still counts as at the end, and to the top as near it.
    static let edge: CGFloat = 40
    static let near: CGFloat = 400
    /// Rows measured at a narrower width would keep heights that are wrong.
    static let narrowest: CGFloat = 120

    /// Whether the end is in view; published just after the list's update, which it may not change.
    @Published private(set) var atBottom = true
    private(set) var phase = Phase.opening
    /// Whether the rows are shown: an opening batch arrives in parts, each moving the end, so
    /// the list stays hidden until it is in and then appears at its place at once.
    private(set) var shown = false
    /// The row at the top of the view and its offset, or nil at the end.
    private(set) var spot: Anchor?
    /// Asks for the page before the first row, once the reader nears the top.
    var page: (() -> Void)?

    private(set) weak var surface: ChatSurface?
    private var ids: [String] = []
    private var ready = false
    private var nearTop = false
    private var holding = false
    private var revealing: String?

    var following: Bool { phase != .anchored }
    private var measurable: Bool { (surface?.size.width ?? 0) >= Self.narrowest }

    /// A new list shows the chat; it opens at the end or at the spot.
    func attach(_ surface: ChatSurface) {
        self.surface = surface
        phase = .opening
        shown = false
        ids = []
        nearTop = false
        holding = false
    }

    /// Takes the list's new rows, which `apply` puts in, and keeps the reader where they were.
    /// `ready` says the Conversation has its opening batch, so the rows can be shown.
    func update(_ ids: [String], ready: Bool, changed: Bool, apply: () -> Void) {
        self.ready = ready
        let holding = self.holding
        self.holding = false
        var anchor = following && !holding ? nil : topRow()
        if let kept = restore(ids) { anchor = kept }
        let reveal = revealing.flatMap { ids.contains($0) ? $0 : nil }
        guard changed || reveal != nil else { return settle() }
        self.ids = ids
        apply()
        if let reveal {
            revealing = nil
            phase = .anchored
            place((reveal, 0))
        } else if following && !holding {
            toEnd()
        } else if let anchor {
            place(anchor)
        }
        report(paging: true)
        settle()
    }

    /// The list took a new size; `relayout` measures its rows anew when the width changed.
    func resized(_ relayout: () -> Void = {}) {
        keep(restore(ids), relayout)
        report()
        settle()
    }

    /// A row took a height of its own, as a card opening does.
    func grew(_ apply: () -> Void) { keep(nil, apply) }

    /// The list moved. Rows measured late also move it, so only the reader leaves the end.
    func scrolled(byReader reader: Bool) {
        if following, !reader { return toEnd() }
        report()
    }

    /// "Move to bottom", or a send: the end, wherever the reader had scrolled.
    func jump() {
        spot = nil
        revealing = nil
        guard phase != .opening else { return }
        phase = .following
        toEnd()
        report()
    }

    /// Brings a row to the top of the view, now or once the list has it.
    func reveal(_ id: String) {
        revealing = id
        guard phase != .opening, ids.contains(id) else { return }
        revealing = nil
        phase = .anchored
        place((id, 0))
        report()
    }

    /// The reader opened or closed a card: the next rows keep the view still, even at the end,
    /// so the card opens under the pointer rather than scrolling away.
    func hold() { holding = true }

    /// Where the reader left this chat, taken once, when the rows can first be measured.
    private func restore(_ ids: [String]) -> Anchor? {
        guard phase == .opening, !ids.isEmpty, measurable else { return nil }
        guard let spot, ids.contains(spot.id) else {
            phase = .following
            return nil
        }
        phase = .anchored
        return spot
    }

    /// Shows the rows, at their place, once the opening batch is in and they can be measured.
    private func settle() {
        guard !shown, let surface else { return }
        guard ready, measurable else { return surface.show(ids.isEmpty) }
        shown = true
        if following { toEnd() }
        surface.show(true)
        report()
    }

    private func keep(_ kept: Anchor?, _ apply: () -> Void) {
        let anchor = kept ?? (following ? nil : topRow())
        apply()
        if following { toEnd() } else if let anchor { place(anchor) }
    }

    /// The first row in view and how far its top sits below the view's. The spinner above the
    /// first message never counts: held in place, it would keep pages loading.
    private func topRow() -> Anchor? {
        guard let surface, let row = surface.visible.first(where: { $0 < ids.count && ids[$0] != Self.earlier }) else { return nil }
        return (ids[row], surface.top(of: row) - surface.offset)
    }

    private func place(_ anchor: Anchor) {
        guard let surface, let row = ids.firstIndex(of: anchor.id) else { return }
        go(surface.top(of: row) - anchor.offset)
    }

    private func toEnd() {
        guard let surface else { return }
        go(surface.end)
    }

    private func go(_ offset: CGFloat) {
        guard let surface else { return }
        surface.scroll(to: min(max(offset, surface.start), max(surface.start, surface.end)))
    }

    /// Takes where the reader is from the list; near the top, asks for the page before, again
    /// after each page while the top stays near. Earlier pages wait until the rows are shown.
    private func report(paging: Bool = false) {
        guard let surface, surface.size.height > 0 else { return }
        let bottom = surface.offset >= surface.end - Self.edge
        let top = shown && surface.offset - surface.start < Self.near
        if phase != .opening {
            phase = bottom ? .following : .anchored
            spot = bottom ? nil : topRow()
        }
        if top, !nearTop || paging { page?() }
        nearTop = top
        trace("report bottom=\(bottom) top=\(top) spot=\(spot.map { "\($0.id)@\($0.offset)" } ?? "-") offset=\(surface.offset)/\(surface.end)")
        guard atBottom != following else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, atBottom != following else { return }
            atBottom = following
        }
    }

    /// `-listLog PATH` traces every report, for finding where the reader was moved.
    private static let log = UserDefaults.standard.string(forKey: "listLog").flatMap { path -> FileHandle? in
        FileManager.default.createFile(atPath: path, contents: nil)
        return FileHandle(forWritingAtPath: path)
    }

    private func trace(_ line: @autoclosure () -> String) {
        Self.log?.write(Data("\(Date().timeIntervalSince1970) \(line())\n".utf8))
    }
}
