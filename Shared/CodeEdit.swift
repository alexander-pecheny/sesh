import Foundation

/// What one editor shortcut does to the text: the range it replaces, the text it puts there,
/// and where the selection lands afterwards. Ranges count UTF-16 units, as text views do.
struct CodeEdit: Equatable {
    var range: NSRange
    var text: String
    var selection: NSRange
}

/// The line editing of VS Code's editor, on plain text, for the code editor on both platforms.
enum CodeEdits {
    /// The whole lines a selection touches. A selection that ends at a line's start leaves that line out.
    static func lines(_ s: NSString, _ sel: NSRange) -> NSRange {
        var end = NSMaxRange(sel)
        if sel.length > 0, s.character(at: end - 1) == newline { end -= 1 }
        return s.lineRange(for: NSRange(location: sel.location, length: end - sel.location))
    }

    /// Tab: indents every line of a selection that spans lines, and otherwise inserts up to the next stop.
    static func tab(_ s: NSString, _ sel: NSRange, unit: String) -> CodeEdit {
        if s.substring(with: sel).contains("\n") { return indent(s, sel, unit: unit) }
        let fill = unit == "\t" ? unit : String(repeating: " ", count: unit.count - column(s, sel.location) % unit.count)
        return CodeEdit(range: sel, text: fill, selection: NSRange(location: sel.location + fill.utf16.count, length: 0))
    }

    static func indent(_ s: NSString, _ sel: NSRange, unit: String) -> CodeEdit {
        perLine(s, sel) { $0.isEmpty ? $0 : unit + $0 }
    }

    static func outdent(_ s: NSString, _ sel: NSRange, unit: String) -> CodeEdit {
        perLine(s, sel) { line in
            if line.hasPrefix("\t") { return String(line.dropFirst()) }
            let spaces = line.prefix { $0 == " " }.count
            return String(line.dropFirst(min(spaces, max(unit.count, 1))))
        }
    }

    /// Comments out every line, at the shallowest indent among them, unless all are already commented.
    static func toggleComment(_ s: NSString, _ sel: NSRange, marker: String) -> CodeEdit {
        let block = s.substring(with: lines(s, sel)).components(separatedBy: "\n")
        let filled = block.filter { !$0.allSatisfy(\.isWhitespace) }
        let commented = !filled.isEmpty && filled.allSatisfy { $0.drop { $0 == " " || $0 == "\t" }.hasPrefix(marker) }
        let depth = filled.map { $0.prefix { $0 == " " || $0 == "\t" }.count }.min() ?? 0
        return perLine(s, sel) { line in
            guard !line.allSatisfy(\.isWhitespace) else { return line }
            if !commented { return String(line.prefix(depth)) + marker + " " + line.dropFirst(depth) }
            let indent = line.prefix { $0 == " " || $0 == "\t" }
            var rest = line.dropFirst(indent.count + marker.count)
            if rest.hasPrefix(" ") { rest = rest.dropFirst() }
            return String(indent + rest)
        }
    }

    /// Swaps the selected lines with the line above or below them, keeping them selected.
    static func move(_ s: NSString, _ sel: NSRange, up: Bool) -> CodeEdit? {
        let block = lines(s, sel)
        if up ? block.location == 0 : NSMaxRange(block) == s.length { return nil }
        let other = s.lineRange(for: NSRange(location: up ? block.location - 1 : NSMaxRange(block), length: 0))
        var (first, second) = up ? (s.substring(with: block), s.substring(with: other)) : (s.substring(with: other), s.substring(with: block))
        if !first.hasSuffix("\n") {
            first += "\n"
            second.removeLast()
        }
        let start = up ? other.location : block.location + first.utf16.count
        return CodeEdit(range: NSUnionRange(block, other), text: first + second,
                        selection: NSRange(location: sel.location - block.location + start, length: sel.length))
    }

    /// Duplicates the selected lines above or below themselves; the selection goes with the lower copy.
    static func copy(_ s: NSString, _ sel: NSRange, up: Bool) -> CodeEdit {
        let block = lines(s, sel)
        let text = s.substring(with: block)
        if text.hasSuffix("\n") {
            return CodeEdit(range: NSRange(location: up ? block.location : NSMaxRange(block), length: 0), text: text,
                            selection: up ? sel : NSRange(location: sel.location + block.length, length: sel.length))
        }
        return up
            ? CodeEdit(range: NSRange(location: block.location, length: 0), text: text + "\n", selection: sel)
            : CodeEdit(range: NSRange(location: NSMaxRange(block), length: 0), text: "\n" + text,
                       selection: NSRange(location: sel.location + block.length + 1, length: sel.length))
    }

    static func deleteLines(_ s: NSString, _ sel: NSRange) -> CodeEdit {
        var block = lines(s, sel)
        if block.location > 0, block.length == 0 || s.character(at: NSMaxRange(block) - 1) != newline {
            block = NSRange(location: block.location - 1, length: block.length + 1)
            let start = s.lineRange(for: NSRange(location: block.location, length: 0)).location
            return CodeEdit(range: block, text: "", selection: NSRange(location: start, length: 0))
        }
        return CodeEdit(range: block, text: "", selection: NSRange(location: block.location, length: 0))
    }

    /// Return keeps the line's indent, adds one after an opening bracket or colon, and puts a
    /// closing bracket right after the caret on a line of its own.
    static func newline(_ s: NSString, _ sel: NSRange, unit: String) -> CodeEdit {
        let line = s.lineRange(for: NSRange(location: sel.location, length: 0))
        let before = s.substring(with: NSRange(location: line.location, length: sel.location - line.location))
        let indent = String(before.prefix { $0 == " " || $0 == "\t" })
        let opens = before.trimmingCharacters(in: .whitespaces).last.map { "{[(:".contains($0) } ?? false
        let after = NSMaxRange(sel) < s.length ? s.substring(with: NSRange(location: NSMaxRange(sel), length: 1)) : ""
        let inner = "\n" + indent + (opens ? unit : "")
        let text = opens && "}])".contains(after) && !after.isEmpty ? inner + "\n" + indent : inner
        return CodeEdit(range: sel, text: text, selection: NSRange(location: sel.location + inner.utf16.count, length: 0))
    }

    /// Starts an empty line below or above the caret's, at its indent, whatever the caret is in.
    static func openLine(_ s: NSString, _ sel: NSRange, above: Bool) -> CodeEdit {
        let line = s.lineRange(for: NSRange(location: sel.location, length: 0))
        let indent = String(s.substring(with: line).prefix { $0 == " " || $0 == "\t" })
        if above {
            return CodeEdit(range: NSRange(location: line.location, length: 0), text: indent + "\n",
                            selection: NSRange(location: line.location + indent.utf16.count, length: 0))
        }
        var end = NSMaxRange(line)
        if end > line.location, s.character(at: end - 1) == newline { end -= 1 }
        return CodeEdit(range: NSRange(location: end, length: 0), text: "\n" + indent,
                        selection: NSRange(location: end + 1 + indent.utf16.count, length: 0))
    }

    /// Backspace in a run of leading spaces goes back to the previous indent stop.
    static func backspace(_ s: NSString, _ sel: NSRange, unit: String) -> CodeEdit? {
        guard sel.length == 0, unit != "\t" else { return nil }
        let start = s.lineRange(for: NSRange(location: sel.location, length: 0)).location
        let before = s.substring(with: NSRange(location: start, length: sel.location - start))
        guard !before.isEmpty, before.allSatisfy({ $0 == " " }) else { return nil }
        let gone = (before.count - 1) % unit.count + 1
        return CodeEdit(range: NSRange(location: sel.location - gone, length: gone), text: "",
                        selection: NSRange(location: sel.location - gone, length: 0))
    }

    /// The caret's whole line, or the next line too when whole lines are already selected.
    static func selectLine(_ s: NSString, _ sel: NSRange) -> NSRange {
        let block = lines(s, sel)
        guard sel.length > 0, block == sel, NSMaxRange(sel) < s.length else { return block }
        return NSUnionRange(sel, s.lineRange(for: NSRange(location: NSMaxRange(sel), length: 0)))
    }

    /// The file's own indent: a tab, or the narrowest run of leading spaces among its first lines.
    static func unit(of text: String) -> String {
        var narrowest = Int.max
        for line in text.split(separator: "\n", omittingEmptySubsequences: true).prefix(500) {
            if line.hasPrefix("\t") { return "\t" }
            let spaces = line.prefix { $0 == " " }.count
            if spaces > 1, spaces < line.count { narrowest = min(narrowest, spaces) }
        }
        return String(repeating: " ", count: narrowest == .max ? 4 : min(narrowest, 8))
    }

    /// The line comment of a file's language, if it has one.
    static func commentMarker(for path: String) -> String? {
        let name = (path as NSString).lastPathComponent.lowercased()
        if ["makefile", "dockerfile", "justfile", "gemfile", "rakefile", "podfile"].contains(name) || name.hasPrefix(".") { return "#" }
        switch (name as NSString).pathExtension {
        case "c", "h", "cc", "cpp", "hpp", "m", "mm", "swift", "rs", "go", "js", "jsx", "mjs", "ts", "tsx", "java", "kt", "kts",
             "scala", "cs", "dart", "zig", "php", "proto", "gradle", "groovy", "jsonc", "scss", "less", "glsl", "metal":
            return "//"
        case "py", "sh", "bash", "zsh", "fish", "rb", "pl", "yaml", "yml", "toml", "conf", "cfg", "r", "nix", "tf", "cmake",
             "ps1", "jl", "ex", "exs", "nim", "mk", "just", "env", "ini":
            return "#"
        case "sql", "lua", "hs", "elm": return "--"
        case "clj", "cljs", "el", "lisp", "scm", "asm", "s": return ";"
        case "tex", "erl": return "%"
        case "vim": return "\""
        default: return nil
        }
    }

    /// Where each line starts, for numbering them in the gutter.
    static func lineStarts(_ s: NSString) -> [Int] {
        var starts = [0]
        var at = 0
        while at < s.length {
            let found = s.range(of: "\n", range: NSRange(location: at, length: s.length - at))
            guard found.location != NSNotFound else { break }
            at = NSMaxRange(found)
            starts.append(at)
        }
        return starts
    }

    private static let newline: unichar = 10

    private static func column(_ s: NSString, _ at: Int) -> Int {
        at - s.lineRange(for: NSRange(location: at, length: 0)).location
    }

    /// Rewrites each selected line, keeping the selection on the same text.
    private static func perLine(_ s: NSString, _ sel: NSRange, _ change: (String) -> String) -> CodeEdit {
        let block = lines(s, sel)
        let old = s.substring(with: block).components(separatedBy: "\n")
        let new = old.map(change)
        var starts: [(old: Int, new: Int, delta: Int)] = []
        var (o, n) = (block.location, block.location)
        for (before, after) in zip(old, new) {
            starts.append((o, n, after.utf16.count - before.utf16.count))
            o += before.utf16.count + 1
            n += after.utf16.count + 1
        }
        func place(_ at: Int, keepsStart: Bool) -> Int {
            guard let line = starts.last(where: { $0.old <= at }) else { return at }
            if keepsStart, at == line.old { return line.new }
            return line.new + max(0, at - line.old + line.delta)
        }
        let start = place(sel.location, keepsStart: sel.length > 0)
        let end = sel.length == 0 ? start : place(NSMaxRange(sel), keepsStart: true)
        return CodeEdit(range: block, text: new.joined(separator: "\n"), selection: NSRange(location: start, length: end - start))
    }
}
