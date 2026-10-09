import CoreGraphics
import XCTest

/// A list of rows stacked from offset 0, as tall as their heights, shown in a 500-point view.
@MainActor
private final class FakeList: ChatSurface {
    var rows: [(id: String, height: CGFloat)] = []
    var size = CGSize(width: 300, height: 500)
    var offset: CGFloat = 0
    var shown = false
    var start: CGFloat { 0 }
    var end: CGFloat { rows.map(\.height).reduce(0, +) - size.height }

    var visible: Range<Int> {
        let rows = rows.indices.filter { top(of: $0) + self.rows[$0].height > offset && top(of: $0) < offset + size.height }
        return (rows.first ?? 0)..<((rows.last ?? -1) + 1)
    }

    func top(of row: Int) -> CGFloat { rows.prefix(row).map(\.height).reduce(0, +) }
    func scroll(to offset: CGFloat) { self.offset = offset }
    func show(_ shown: Bool) { self.shown = shown }
    func top(of id: String) -> CGFloat { top(of: rows.firstIndex { $0.id == id }!) }
}

@MainActor
final class ChatScrollTests: XCTestCase {
    private let scroll = ChatScroll()
    private let list = FakeList()
    private var pages = 0

    override func setUp() async throws {
        scroll.attach(list)
        scroll.page = { [unowned self] in pages += 1 }
    }

    private func rows(_ names: [String], height: CGFloat = 100) -> [(id: String, height: CGFloat)] {
        names.map { ($0, height) }
    }

    private func numbered(_ range: ClosedRange<Int>) -> [String] { range.map { "r\($0)" } }

    private func show(_ rows: [(id: String, height: CGFloat)], on list: FakeList? = nil, ready: Bool = true) {
        let list = list ?? self.list
        scroll.update(rows.map(\.id), ready: ready, changed: true) { list.rows = rows }
    }

    /// Scrolls as the reader does, to `row`'s top less `offset`.
    private func read(_ row: String, at offset: CGFloat) {
        list.offset = list.top(of: row) - offset
        scroll.scrolled(byReader: true)
    }

    func testOpensHiddenUntilTheOpeningBatchIsInThenAtTheEnd() {
        show(rows(numbered(1...20)), ready: false)
        XCTAssertFalse(list.shown)
        show(rows(numbered(1...50)))
        XCTAssertTrue(list.shown)
        XCTAssertEqual(list.offset, list.end)
        XCTAssertEqual(scroll.phase, .following)
    }

    func testReopensAtTheSpotTheReaderLeft() {
        show(rows(numbered(1...50)))
        read("r10", at: -30)
        let again = FakeList()
        scroll.attach(again)
        show(rows(numbered(1...50)), on: again)
        XCTAssertEqual(again.offset, again.top(of: "r10") + 30)
        XCTAssertEqual(scroll.phase, .anchored)
    }

    func testRowsAddedAboveKeepTheReadRowStill() {
        show(rows(numbered(1...50)))
        read("r30", at: -30)
        show(rows(numbered(-4...50)))
        XCTAssertEqual(list.top(of: "r30") - list.offset, -30)
    }

    func testNewRowsAtTheEndAreFollowed() {
        show(rows(numbered(1...50)))
        show(rows(numbered(1...53)))
        XCTAssertEqual(list.offset, list.end)
        list.rows[52].height = 400
        list.offset -= 300
        scroll.scrolled(byReader: false)
        XCTAssertEqual(list.offset, list.end, "a row measured late does not take the reader off the end")
    }

    func testReaderLeavesTheEndAndMoveToBottomBringsThemBack() {
        show(rows(numbered(1...50)))
        read("r20", at: 0)
        XCTAssertEqual(scroll.phase, .anchored)
        show(rows(numbered(1...51)))
        XCTAssertEqual(list.offset, list.top(of: "r20"))
        scroll.jump()
        XCTAssertEqual(scroll.phase, .following)
        XCTAssertEqual(list.offset, list.end)
        XCTAssertNil(scroll.spot)
    }

    func testNearingTheTopAsksForEachPageWhileItStaysNear() {
        show(rows([ChatScroll.earlier] + numbered(1...10)))
        read("r1", at: 100)
        XCTAssertEqual(pages, 1)
        show(rows([ChatScroll.earlier] + numbered(-1...10), height: 50))
        XCTAssertEqual(pages, 2, "the page went in above, and the top is still near")
        show(rows([ChatScroll.earlier] + numbered(-20...10), height: 50))
        XCTAssertEqual(pages, 2)
    }

    func testRevealWaitsForItsRowToBeLoaded() {
        show(rows(numbered(1...10)))
        read("r1", at: 0)
        scroll.reveal("r-12")
        XCTAssertEqual(list.offset, 0)
        show(rows([ChatScroll.earlier] + numbered(-4...10)))
        XCTAssertEqual(list.top(of: "r1"), list.offset)
        show(rows([ChatScroll.earlier] + numbered(-14...10)))
        XCTAssertEqual(list.offset, list.top(of: "r-12"))
        XCTAssertEqual(scroll.phase, .anchored)
    }

    func testResizeKeepsTheReadRow() {
        show(rows(numbered(1...50)))
        read("r10", at: -30)
        list.size.width = 200
        scroll.resized { list.rows = list.rows.map { ($0.id, $0.height * 1.5) } }
        XCTAssertEqual(list.top(of: "r10") - list.offset, -30)
        list.size.height = 300
        scroll.resized()
        XCTAssertEqual(list.top(of: "r10") - list.offset, -30)
    }

    func testAShorterViewKeepsTheEndInView() {
        show(rows(numbered(1...50)))
        list.size.height = 200
        scroll.resized()
        XCTAssertEqual(list.offset, list.end)
    }

    func testOpeningACardAtTheEndKeepsTheViewStill() {
        show(rows(numbered(1...50)))
        let before = list.offset
        scroll.hold()
        scroll.update(numbered(1...50), ready: true, changed: true) { list.rows[49].height = 300 }
        XCTAssertEqual(list.offset, before)
    }
}
