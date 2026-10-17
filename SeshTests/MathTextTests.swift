import XCTest

final class MathTextTests: XCTestCase {
    private func formulas(_ markdown: String) -> [String] { MathText.extract(markdown).formulas.map(\.latex) }

    func testFormulasAreFoundOutsideCodeAndPricesAreLeftAlone() {
        XCTAssertEqual(formulas("силы $B$ и $Q_{i}^*$"), ["B", "Q_{i}^*"])
        XCTAssertEqual(formulas("it costs $5 and $10 today"), [])
        XCTAssertEqual(formulas("$10, and `$code$` stays code"), [])
        XCTAssertEqual(formulas("`$x$` and\n```\n$y$\n```\nthen $z$"), ["z"])
        XCTAssertEqual(formulas("$$\n\\frac{a}{b}\n$$ and \\(c\\) and \\[d\\]"), ["\\frac{a}{b}", "c", "d"])
        XCTAssertEqual(MathText.extract("$$x$$").formulas.map(\.display), [true])
    }

    func testPlaceholdersGoBackAsWritten() {
        let text = "a $b_1$ c \\(d\\)"
        let (protected, found) = MathText.extract(text)
        XCTAssertFalse(protected.contains("_"))
        XCTAssertEqual(MathText.restore(protected, found), text)
    }

    func testFormulasAreRewrittenIntoWhatSwiftMathReads() {
        XCTAssertEqual(MathText.forSwiftMath("\\Bigl(1 - \\bigl\\{x\\bigr\\}\\Bigr)"), "\\left(1 - \\left\\{x\\right\\}\\right)")
        XCTAssertEqual(MathText.forSwiftMath("\\Big| x \\Bigm| \\bigodot"), "| x | \\bigodot")
        XCTAssertEqual(MathText.forSwiftMath("\\text{возьмёт Ёлку}"), "\\text{возьмет Елку}")
    }
}
