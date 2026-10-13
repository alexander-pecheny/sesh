import XCTest

/// Applies an edit to text where `|` marks the caret and `[` `]` a selection, and marks the result the same way.
private func apply(_ marked: String, _ edit: (NSString, NSRange) -> CodeEdit?) -> String? {
    let (text, sel) = parse(marked)
    guard let change = edit(text as NSString, sel) else { return nil }
    let out = (text as NSString).replacingCharacters(in: change.range, with: change.text) as NSString
    return mark(out, change.selection)
}

private func parse(_ marked: String) -> (String, NSRange) {
    let s = marked as NSString
    if s.range(of: "|").location != NSNotFound {
        let at = s.range(of: "|").location
        return (s.replacingCharacters(in: NSRange(location: at, length: 1), with: ""), NSRange(location: at, length: 0))
    }
    let start = s.range(of: "[").location
    let end = s.range(of: "]").location - 1
    let text = marked.replacingOccurrences(of: "[", with: "").replacingOccurrences(of: "]", with: "")
    return (text, NSRange(location: start, length: end - start))
}

private func mark(_ s: NSString, _ sel: NSRange) -> String {
    guard sel.length > 0 else { return s.replacingCharacters(in: sel, with: "|") }
    let closed = s.replacingCharacters(in: NSRange(location: NSMaxRange(sel), length: 0), with: "]") as NSString
    return closed.replacingCharacters(in: NSRange(location: sel.location, length: 0), with: "[")
}

final class CodeEditTests: XCTestCase {
    func testTabFillsToTheNextStop() {
        XCTAssertEqual(apply("ab|c") { CodeEdits.tab($0, $1, unit: "    ") }, "ab  |c")
        XCTAssertEqual(apply("ab|c") { CodeEdits.tab($0, $1, unit: "\t") }, "ab\t|c")
    }

    func testTabOverLinesIndentsThemAndSkipsBlankOnes() {
        XCTAssertEqual(apply("[a\n\nb]\nc") { CodeEdits.tab($0, $1, unit: "  ") }, "[  a\n\n  b]\nc")
    }

    func testOutdentTakesOneLevelAndKeepsTheCaretOnItsText() {
        XCTAssertEqual(apply("      a|b") { CodeEdits.outdent($0, $1, unit: "    ") }, "  a|b")
        XCTAssertEqual(apply("\tx\n\t[y]") { CodeEdits.outdent($0, $1, unit: "\t") }, "\tx\n[y]")
    }

    func testCommentGoesAtTheShallowestIndentAndComesOffAgain() {
        let commented = apply("[  a\n    b]") { CodeEdits.toggleComment($0, $1, marker: "//") }
        XCTAssertEqual(commented, "[  // a\n  //   b]")
        XCTAssertEqual(apply(commented!) { CodeEdits.toggleComment($0, $1, marker: "//") }, "[  a\n    b]")
    }

    func testMixedLinesAreAllCommented() {
        XCTAssertEqual(apply("[# a\nb]") { CodeEdits.toggleComment($0, $1, marker: "#") }, "[# # a\n# b]")
    }

    func testMoveSwapsWithNeighboursIncludingTheLastLine() {
        XCTAssertEqual(apply("a\nb|\nc") { CodeEdits.move($0, $1, up: true) }, "b|\na\nc")
        XCTAssertEqual(apply("a\nb|\nc") { CodeEdits.move($0, $1, up: false) }, "a\nc\nb|")
        XCTAssertEqual(apply("a\nc|") { CodeEdits.move($0, $1, up: true) }, "c|\na")
        XCTAssertNil(apply("a|\nb") { CodeEdits.move($0, $1, up: true) })
        XCTAssertNil(apply("a\nb|") { CodeEdits.move($0, $1, up: false) })
    }

    func testCopyDownMovesTheCaretToTheCopy() {
        XCTAssertEqual(apply("a|b\nc") { CodeEdits.copy($0, $1, up: false) }, "ab\na|b\nc")
        XCTAssertEqual(apply("x\na|b") { CodeEdits.copy($0, $1, up: false) }, "x\nab\na|b")
        XCTAssertEqual(apply("a|b") { CodeEdits.copy($0, $1, up: true) }, "a|b\nab")
    }

    func testDeleteLinesTakesTheNewlineWithThem() {
        XCTAssertEqual(apply("a\nb|\nc") { CodeEdits.deleteLines($0, $1) }, "a\n|c")
        XCTAssertEqual(apply("a\nb|") { CodeEdits.deleteLines($0, $1) }, "|a")
        XCTAssertEqual(apply("a\n|") { CodeEdits.deleteLines($0, $1) }, "|a")
    }

    func testReturnKeepsIndentAndOpensBrackets() {
        XCTAssertEqual(apply("  x|") { CodeEdits.newline($0, $1, unit: "  ") }, "  x\n  |")
        XCTAssertEqual(apply("if x:|") { CodeEdits.newline($0, $1, unit: "    ") }, "if x:\n    |")
        XCTAssertEqual(apply("  f {|}") { CodeEdits.newline($0, $1, unit: "  ") }, "  f {\n    |\n  }")
    }

    func testOpenLineIgnoresWhereTheCaretIs() {
        XCTAssertEqual(apply("  a|b\nc") { CodeEdits.openLine($0, $1, above: false) }, "  ab\n  |\nc")
        XCTAssertEqual(apply("x\n  a|b") { CodeEdits.openLine($0, $1, above: true) }, "x\n  |\n  ab")
    }

    func testBackspaceInIndentGoesToTheLastStop() {
        XCTAssertEqual(apply("      |x") { CodeEdits.backspace($0, $1, unit: "    ") }, "    |x")
        XCTAssertNil(apply("  a |") { CodeEdits.backspace($0, $1, unit: "    ") })
    }

    func testSelectLineGrowsByALineEachTime() {
        let s = "a\nb\nc" as NSString
        let first = CodeEdits.selectLine(s, NSRange(location: 0, length: 0))
        XCTAssertEqual(first, NSRange(location: 0, length: 2))
        XCTAssertEqual(CodeEdits.selectLine(s, first), NSRange(location: 0, length: 4))
    }

    func testUnitFollowsTheFile() {
        XCTAssertEqual(CodeEdits.unit(of: "a\n\tb"), "\t")
        XCTAssertEqual(CodeEdits.unit(of: "a\n  b\n    c"), "  ")
        XCTAssertEqual(CodeEdits.unit(of: "a\nb"), "    ")
    }

    func testLineStarts() {
        XCTAssertEqual(CodeEdits.lineStarts("ab\n\nc\n"), [0, 3, 4, 6])
    }
}
