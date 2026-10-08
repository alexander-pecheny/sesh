#if os(macOS)
import AppKit
import SwiftUI
import cmark_gfm
import cmark_gfm_extensions

/// An Agent's Markdown as one native text view, so a selection can run across paragraphs,
/// which SwiftUI's one-view-per-block rendering cannot do. Parsed with cmark, as the links are.
struct Prose: NSViewRepresentable {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.openURL) private var openURL
    let text: String

    func makeNSView(context: Context) -> NSTextView {
        let view = LinkTextView()
        view.isEditable = false
        view.isSelectable = true
        view.usesFindBar = true
        view.drawsBackground = false
        view.textContainerInset = .zero
        view.textContainer?.lineFragmentPadding = 0
        view.textContainer?.widthTracksTextView = true
        view.delegate = context.coordinator
        return view
    }

    func updateNSView(_ view: NSTextView, context: Context) {
        context.coordinator.openURL = openURL
        let flavour: Catppuccin.Flavour = colorScheme == .dark ? .mocha : .latte
        let key = "\(colorScheme)\(text)"
        guard context.coordinator.shown != key else { return }
        context.coordinator.shown = key
        view.textStorage?.setAttributedString(Renderer(flavour: flavour).render(text))
        view.linkTextAttributes = [.foregroundColor: NSColor(flavour(.blue)), .cursor: NSCursor.pointingHand]
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView view: NSTextView, context: Context) -> CGSize? {
        guard let container = view.textContainer, let layout = view.layoutManager else { return nil }
        // Asked for its ideal size, the text is as wide as its longest line, up to the measure.
        let width = proposal.width.flatMap { $0.isFinite && $0 > 0 ? $0 : nil } ?? Metric.measure
        container.containerSize = NSSize(width: width, height: .greatestFiniteMagnitude)
        layout.ensureLayout(for: container)
        let used = layout.usedRect(for: container)
        // As wide as the longest line, so a short message makes a small bubble; the layout
        // manager reports the container's width here, the string its own.
        let lines = view.textStorage?.boundingRect(
            with: NSSize(width: width, height: .greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading])
        return CGSize(width: min(width, ceil(lines?.width ?? width)), height: ceil(used.height))
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var shown = ""
        var openURL: OpenURLAction?

        func textView(_ view: NSTextView, clickedOnLink link: Any, at index: Int) -> Bool {
            guard let url = (link as? URL) ?? (link as? String).flatMap(URL.init(string:)) else { return false }
            openURL?(url)
            return true
        }
    }
}

/// A read-only text view whose links answer the pointer: a hand and an underline while over one.
private final class LinkTextView: NSTextView {
    private var lit: NSRange?

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect], owner: self))
        super.updateTrackingAreas()
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        let point = convert(event.locationInWindow, from: nil)
        var range = NSRange()
        let index = characterIndexForInsertion(at: point)
        let link = index < textStorage?.length ?? 0 ? textStorage?.attribute(.link, at: index, effectiveRange: &range) : nil
        light(link == nil ? nil : range)
        (link == nil ? NSCursor.iBeam : NSCursor.pointingHand).set()
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        light(nil)
        NSCursor.arrow.set()
    }

    private func light(_ range: NSRange?) {
        guard range != lit, let layout = layoutManager else { return }
        if let lit { layout.removeTemporaryAttribute(.underlineStyle, forCharacterRange: lit) }
        if let range { layout.addTemporaryAttribute(.underlineStyle, value: NSUnderlineStyle.single.rawValue, forCharacterRange: range) }
        lit = range
    }
}

/// Walks cmark's tree into one attributed string, in the app's colours and sizes.
@MainActor
private struct Renderer {
    let flavour: Catppuccin.Flavour
    private var body: NSFont { .systemFont(ofSize: Metric.body) }
    private var mono: NSFont { .monospacedSystemFont(ofSize: Metric.note, weight: .regular) }

    func render(_ markdown: String) -> NSAttributedString {
        let out = NSMutableAttributedString()
        guard let root = Cmark.parse(markdown) else { return NSAttributedString(string: markdown) }
        defer { cmark_node_free(root) }
        blocks(of: root, into: out, depth: 0)
        while out.string.hasSuffix("\n") { out.deleteCharacters(in: NSRange(location: out.length - 1, length: 1)) }
        return out
    }

    private func paragraph(indent: CGFloat = 0, first: CGFloat? = nil, after: CGFloat = Metric.gap) -> NSParagraphStyle {
        let style = NSMutableParagraphStyle()
        style.headIndent = indent
        style.firstLineHeadIndent = first ?? indent
        // A list marker's tab lands exactly where the item's wrapped lines start.
        style.tabStops = [NSTextTab(textAlignment: .left, location: indent)]
        style.paragraphSpacing = after
        style.lineSpacing = 2
        return style
    }

    private func attributes(font: NSFont, style: NSParagraphStyle, colour: Catppuccin.Swatch = .text) -> [NSAttributedString.Key: Any] {
        [.font: font, .paragraphStyle: style, .foregroundColor: NSColor(flavour(colour))]
    }

    private func children(_ node: UnsafeMutablePointer<cmark_node>) -> [UnsafeMutablePointer<cmark_node>] {
        var list: [UnsafeMutablePointer<cmark_node>] = []
        var child = cmark_node_first_child(node)
        while let current = child {
            list.append(current)
            child = cmark_node_next(current)
        }
        return list
    }

    private func kind(_ node: UnsafeMutablePointer<cmark_node>) -> String { String(cString: cmark_node_get_type_string(node)) }

    private func blocks(of node: UnsafeMutablePointer<cmark_node>, into out: NSMutableAttributedString, depth: Int) {
        for child in children(node) { block(child, into: out, depth: depth) }
    }

    private func block(_ node: UnsafeMutablePointer<cmark_node>, into out: NSMutableAttributedString, depth: Int) {
        let indent = CGFloat(depth) * Metric.wide
        switch kind(node) {
        case "paragraph":
            out.append(inlines(node, attributes(font: body, style: paragraph(indent: indent))))
            out.append(NSAttributedString(string: "\n", attributes: attributes(font: body, style: paragraph(indent: indent))))
        case "heading":
            let level = Int(cmark_node_get_heading_level(node))
            let size = [Metric.body + 7, Metric.body + 4, Metric.body + 2][min(level, 3) - 1]
            let style = attributes(font: .boldSystemFont(ofSize: size), style: paragraph(indent: indent, after: Metric.tiny))
            out.append(inlines(node, style))
            out.append(NSAttributedString(string: "\n", attributes: style))
        case "code_block":
            let code = String(cString: cmark_node_get_literal(node)).trimmingCharacters(in: .newlines)
            var style = attributes(font: mono, style: paragraph(indent: indent + Metric.pad))
            style[.backgroundColor] = NSColor(flavour(.mantle))
            let block = NSMutableAttributedString(string: code + "\n", attributes: style)
            #if canImport(Highlightr)
            let language = cmark_node_get_fence_info(node).map { String(cString: $0) }
            if let coloured = Highlight.code(code, language: language, dark: flavour == .mocha, size: Metric.note) {
                coloured.enumerateAttribute(.foregroundColor, in: NSRange(location: 0, length: coloured.length)) { colour, range, _ in
                    if let colour { block.addAttribute(.foregroundColor, value: colour, range: range) }
                }
            }
            #endif
            out.append(block)
        case "block_quote":
            let start = out.length
            blocks(of: node, into: out, depth: depth + 1)
            out.addAttribute(.foregroundColor, value: NSColor(flavour(.subtext0)), range: NSRange(location: start, length: out.length - start))
        case "list":
            let ordered = cmark_node_get_list_type(node) == CMARK_ORDERED_LIST
            var number = Int(cmark_node_get_list_start(node))
            for item in children(node) {
                let marker = ordered ? "\(number)." : "•"
                number += 1
                let style = paragraph(indent: indent + Metric.wide + Metric.gap, first: indent + Metric.tiny, after: Metric.tiny)
                out.append(NSAttributedString(string: marker + "\t", attributes: attributes(font: body, style: style, colour: .overlay2)))
                let start = out.length - (marker.count + 1)
                blocks(of: item, into: out, depth: depth + 1)
                // The item's first paragraph shares the marker's line, so it takes the marker's style.
                let rest = (out.string as NSString).substring(from: start)
                let end = rest.firstIndex(of: "\n").map { rest.distance(from: rest.startIndex, to: $0) + 1 } ?? rest.count
                out.addAttribute(.paragraphStyle, value: style, range: NSRange(location: start, length: (rest.prefix(end) as Substring).utf16.count))
            }
            out.append(NSAttributedString(string: "\n", attributes: attributes(font: .systemFont(ofSize: Metric.tiny), style: paragraph())))
        case "thematic_break":
            out.append(NSAttributedString(string: "———\n", attributes: attributes(font: body, style: paragraph(indent: indent), colour: .overlay0)))
        case "table":
            out.append(table(node, indent: indent))
        default:
            out.append(NSAttributedString(string: plain(node) + "\n", attributes: attributes(font: body, style: paragraph(indent: indent))))
        }
    }

    /// A table as AppKit draws one: bordered cells whose text wraps and keeps its marks, the
    /// header in bold, the whole width shared out by the cells' content.
    private func table(_ node: UnsafeMutablePointer<cmark_node>, indent: CGFloat) -> NSAttributedString {
        let rows = children(node)
        let table = NSTextTable()
        table.numberOfColumns = rows.map { children($0).count }.max() ?? 1
        table.layoutAlgorithm = .automaticLayoutAlgorithm
        table.collapsesBorders = true
        table.setContentWidth(100, type: .percentageValueType)
        let border = NSColor(flavour(.surface2))
        // Each column gets the share of the width its longest text asks for, so a column of
        // short labels stays narrow, as in Claude's own tables.
        let longest = (0..<table.numberOfColumns).map { column in
            let texts = rows.map { children($0) }.compactMap { $0.count > column ? plain($0[column]) : nil }
            let text = texts.map(\.count).max() ?? 1
            // Room for the longest word too, so no cell breaks one in half.
            let word = texts.flatMap { $0.split(separator: " ") }.map(\.count).max() ?? 1
            return max(min(text, 120), word * 2)
        }
        let total = Double(longest.reduce(0, +))
        let out = NSMutableAttributedString()
        for (row, cells) in rows.map(children).enumerated() {
            for (column, cell) in cells.enumerated() {
                let block = NSTextTableBlock(table: table, startingRow: row, rowSpan: 1, startingColumn: column, columnSpan: 1)
                block.setValue(100 * Double(longest[column]) / max(total, 1), type: .percentageValueType, for: .width)
                // A cell, not the table, keeps the prose measure.
                block.setValue(Metric.measure, type: .absoluteValueType, for: .maximumWidth)
                block.setBorderColor(border)
                block.setWidth(1, type: .absoluteValueType, for: .border)
                block.setWidth(Metric.gap, type: .absoluteValueType, for: .padding)
                if row == 0 { block.backgroundColor = NSColor(flavour(.mantle)) }
                let style = NSMutableParagraphStyle()
                style.textBlocks = [block]
                style.lineSpacing = 2
                let font = NSFont.monospacedDigitSystemFont(ofSize: Metric.label, weight: row == 0 ? .bold : .regular)
                let base = attributes(font: font, style: style)
                out.append(inlines(cell, base))
                out.append(NSAttributedString(string: "\n", attributes: base))
            }
        }
        out.append(NSAttributedString(string: "\n", attributes: attributes(font: .systemFont(ofSize: Metric.tiny), style: paragraph())))
        return out
    }

    private func plain(_ node: UnsafeMutablePointer<cmark_node>) -> String {
        if let literal = cmark_node_get_literal(node) { return String(cString: literal) }
        return children(node).map { kind($0) == "softbreak" ? " " : plain($0) }.joined()
    }

    private func inlines(_ node: UnsafeMutablePointer<cmark_node>, _ base: [NSAttributedString.Key: Any]) -> NSAttributedString {
        let out = NSMutableAttributedString()
        for child in children(node) {
            var style = base
            switch kind(child) {
            case "text": out.append(NSAttributedString(string: plain(child), attributes: style))
            // Agents write one line per thought and mean it as a line, as chat apps show it.
            case "softbreak": out.append(NSAttributedString(string: "\u{2028}", attributes: style))
            case "linebreak": out.append(NSAttributedString(string: "\u{2028}", attributes: style))
            case "code":
                style[.font] = mono
                style[.backgroundColor] = NSColor(flavour(.surface0))
                out.append(NSAttributedString(string: plain(child), attributes: style))
            case "emph", "strong", "strikethrough":
                let font = style[.font] as? NSFont ?? body
                let trait: NSFontDescriptor.SymbolicTraits = kind(child) == "emph" ? .italic : .bold
                if kind(child) == "strikethrough" {
                    style[.strikethroughStyle] = NSUnderlineStyle.single.rawValue
                } else {
                    style[.font] = NSFont(descriptor: font.fontDescriptor.withSymbolicTraits(font.fontDescriptor.symbolicTraits.union(trait)), size: font.pointSize) ?? font
                }
                out.append(inlines(child, style))
            case "link":
                if let url = cmark_node_get_url(child).flatMap({ URL(string: String(cString: $0)) }) { style[.link] = url }
                out.append(inlines(child, style))
            case "image": out.append(NSAttributedString(string: plain(child), attributes: style))
            default: out.append(NSAttributedString(string: plain(child), attributes: style))
            }
        }
        return out
    }
}
#endif
