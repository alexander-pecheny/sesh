import XCTest

final class LinkRuleTests: XCTestCase {
    func testTheMatchFillsTheLink() {
        let rule = LinkRule(pattern: "[A-Z]+-[0-9]+", url: "https://linear.app/perplexity/issue/$0")
        let text = "Fixed SRCH-9049 and SRCH-12."
        let links = rule.links(in: text)
        XCTAssertEqual(links.map { String(text[$0.0]) }, ["SRCH-9049", "SRCH-12"])
        XCTAssertEqual(links.first?.1.absoluteString, "https://linear.app/perplexity/issue/SRCH-9049")
    }

    func testGroupsFillTheirPlaces() {
        let rule = LinkRule(pattern: "([a-z]+)/([a-z]+)#([0-9]+)", url: "https://github.com/$1/$2/issues/$3")
        XCTAssertEqual(rule.links(in: "see foo/bar#7").first?.1.absoluteString, "https://github.com/foo/bar/issues/7")
    }

    func testABrokenPatternLinksNothing() {
        XCTAssertTrue(LinkRule(pattern: "[A-Z", url: "https://example.com/$0").links(in: "A-1").isEmpty)
    }

    func testTheRulesSurviveTheVault() throws {
        let body = Record.Body(links: [LinkRule(pattern: "X-1", url: "https://x/$0")])
        let decoded = try JSONDecoder().decode(Record.Body.self, from: JSONEncoder().encode(body))
        XCTAssertEqual(decoded.links?.map(\.pattern), ["X-1"])
        XCTAssertEqual(decoded.links?.map(\.url), ["https://x/$0"])
    }
}
