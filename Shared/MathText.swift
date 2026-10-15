import Foundation

/// LaTeX formulas in Markdown: `$…$` and `\(…\)` in a line, `$$…$$` and `\[…\]` on their own.
/// Each is swapped for a placeholder before cmark parses the rest, whose emphasis would eat a
/// formula's underscores and stars, and swapped back where it is drawn.
enum MathText {
    struct Formula: Equatable {
        let latex: String
        let display: Bool
        /// The formula as written, delimiters included, for putting it back.
        let source: String
    }

    static let open: Character = "\u{E000}"
    static let close: Character = "\u{E001}"

    /// The Markdown with every formula outside code a placeholder, and the formulas in order.
    static func extract(_ markdown: String) -> (text: String, formulas: [Formula]) {
        guard markdown.contains("$") || markdown.contains("\\(") || markdown.contains("\\[") else { return (markdown, []) }
        var out = "", formulas: [Formula] = []
        let chars = Array(markdown)
        var index = 0, fence: String?, lineStart = true, verbatim = false
        func starts(_ text: String, at at: Int) -> Bool {
            at + text.count <= chars.count && String(chars[at..<at + text.count]) == text
        }
        func find(_ text: String, from: Int, line: Bool) -> Int? {
            var at = from
            while at + text.count <= chars.count {
                if line, chars[at] == "\n" || chars[at] == "`" { return nil }
                if chars[at] == "\\", text != "\\)", text != "\\]" { at += 2; continue }
                if starts(text, at: at) { return at }
                at += 1
            }
            return nil
        }
        func take(_ from: Int, _ end: Int, open: Int, close: Int, display: Bool) {
            let latex = String(chars[from + open..<end]).trimmingCharacters(in: .whitespacesAndNewlines)
            formulas.append(Formula(latex: latex, display: display, source: String(chars[from..<end + close])))
            out += "\(Self.open)\(formulas.count - 1)\(Self.close)"
            index = end + close
        }
        while index < chars.count {
            let char = chars[index]
            if lineStart {
                let rest = String(chars[index...].prefix { $0 != "\n" }).trimmingCharacters(in: .whitespaces)
                verbatim = fence != nil || rest.hasPrefix("```") || rest.hasPrefix("~~~")
                if let marker = fence {
                    if rest.hasPrefix(marker) { fence = nil }
                } else if verbatim {
                    fence = String(rest.prefix(3))
                }
            }
            lineStart = char == "\n"
            if verbatim || char == "\n" {
                out.append(char)
                index += 1
                continue
            }
            if char == "`" {
                let run = chars[index...].prefix { $0 == "`" }.count
                let ticks = String(repeating: "`", count: run)
                let end = find(ticks, from: index + run, line: false) ?? index
                let stop = end == index ? index + run : end + run
                out += String(chars[index..<stop])
                index = stop
                continue
            }
            if char == "\\", index + 1 < chars.count {
                let next = chars[index + 1]
                if next == "[" || next == "(", let end = find(next == "[" ? "\\]" : "\\)", from: index + 2, line: next == "(") {
                    take(index, end, open: 2, close: 2, display: next == "[")
                } else {
                    out += String(chars[index...index + 1])
                    index += 2
                }
                continue
            }
            if char == "$" {
                if starts("$$", at: index), let end = find("$$", from: index + 2, line: false), end > index + 2 {
                    take(index, end, open: 2, close: 2, display: true)
                    continue
                }
                // As pandoc reads it, so "costs $5 and $10" stays text: no space inside either
                // dollar, and no digit right after the closing one.
                if index + 1 < chars.count, !chars[index + 1].isWhitespace, chars[index + 1] != "$",
                   let end = find("$", from: index + 1, line: true), !chars[end - 1].isWhitespace,
                   end + 1 >= chars.count || !chars[end + 1].isNumber {
                    take(index, end, open: 1, close: 1, display: false)
                    continue
                }
            }
            out.append(char)
            index += 1
        }
        return (out, formulas)
    }

    /// Text with placeholders put back as the formulas were written.
    static func restore(_ text: String, _ formulas: [Formula]) -> String {
        guard !formulas.isEmpty else { return text }
        return pieces(text).map { piece in
            switch piece {
            case .text(let text): text
            case .formula(let index): formulas.indices.contains(index) ? formulas[index].source : ""
            }
        }.joined()
    }

    enum Piece: Equatable {
        case text(String)
        case formula(Int)
    }

    /// Text split around its placeholders.
    static func pieces(_ text: String) -> [Piece] {
        guard text.contains(open) else { return [.text(text)] }
        var pieces: [Piece] = [], rest = text[...]
        while let start = rest.firstIndex(of: open), let end = rest[start...].firstIndex(of: close),
              let number = Int(rest[rest.index(after: start)..<end]) {
            if start > rest.startIndex { pieces.append(.text(String(rest[..<start]))) }
            pieces.append(.formula(number))
            rest = rest[rest.index(after: end)...]
        }
        if !rest.isEmpty { pieces.append(.text(String(rest))) }
        return pieces
    }
}
