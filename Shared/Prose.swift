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
        // A table sizes its columns by the text view's frame, not the container's.
        if view.frame.width != width { view.setFrameSize(NSSize(width: width, height: view.frame.height)) }
        container.containerSize = NSSize(width: width, height: .greatestFiniteMagnitude)
        layout.ensureLayout(for: container)
        let used = layout.usedRect(for: container)
        // As wide as the longest line, so a short message makes a small bubble. Text that wraps,
        // or holds a table, keeps the whole width: narrower, it would wrap onto more lines than
        // the height measured here.
        var longest: CGFloat = 0, lines = 0
        layout.enumerateLineFragments(forGlyphRange: layout.glyphRange(for: container)) { _, line, _, _, _ in
            longest = max(longest, line.maxX)
            lines += 1
        }
        let paragraphs = view.string.utf16.reduce(1) { $1 == 10 ? $0 + 1 : $0 }
        let full = lines > paragraphs || Cmark.hasTable(text)
        return CGSize(width: full ? width : min(width, ceil(longest)), height: ceil(used.height))
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

    /// The renderer breaks lines with U+2028, which other apps paste as a stray character.
    override func writeSelection(to board: NSPasteboard, types: [NSPasteboard.PasteboardType]) -> Bool {
        guard super.writeSelection(to: board, types: types) else { return false }
        if let text = board.string(forType: .string) {
            board.setString(text.replacingOccurrences(of: "\u{2028}", with: "\n"), forType: .string)
        }
        return true
    }

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

#else
import SwiftUI
import UIKit

/// An Agent's Markdown as one native text view, drawn by the renderer the Mac uses, so a
/// reply looks the same on both and a selection can run across paragraphs.
struct Prose: UIViewRepresentable {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.openURL) private var openURL
    let text: String

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = false
        view.isSelectable = true
        view.isScrollEnabled = false
        view.backgroundColor = .clear
        view.textContainerInset = .zero
        view.textContainer.lineFragmentPadding = 0
        view.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        view.delegate = context.coordinator
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        context.coordinator.openURL = openURL
        let flavour: Catppuccin.Flavour = colorScheme == .dark ? .mocha : .latte
        let key = "\(colorScheme)\(text)"
        guard context.coordinator.shown != key else { return }
        context.coordinator.shown = key
        view.attributedText = Renderer(flavour: flavour).render(text)
        view.linkTextAttributes = [.foregroundColor: UIColor(flavour(.blue))]
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView view: UITextView, context: Context) -> CGSize? {
        let width = proposal.width.flatMap { $0.isFinite && $0 > 0 ? $0 : nil } ?? Metric.measure
        let height = view.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        return CGSize(width: Self.width(of: view.attributedText, in: width), height: ceil(height))
    }

    /// As wide as the text when it fits on its lines unwrapped, so a short message or cell
    /// takes no more room than it needs; text that wraps keeps the whole width, or it would wrap
    /// again onto more lines than were measured.
    static func width(of text: NSAttributedString, in width: CGFloat) -> CGFloat {
        let natural = text.boundingRect(
            with: CGSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil).width
        return natural > width ? width : ceil(natural)
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    final class Coordinator: NSObject, UITextViewDelegate {
        var shown = ""
        var openURL: OpenURLAction?

        func textView(_ textView: UITextView, primaryActionFor textItem: UITextItem, defaultAction: UIAction) -> UIAction? {
            guard case .link(let url) = textItem.content, let openURL else { return defaultAction }
            return UIAction { _ in openURL(url) }
        }
    }
}

/// A table on the phone, which cannot draw one inside text: a grid that scrolls sideways,
/// each cell drawn by the same renderer and no wider than the prose measure.
struct ProseTable: View {
    @Environment(\.colorScheme) private var colorScheme
    let text: String

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        let rows = Renderer(flavour: flavour).cells(text)
        ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .topLeading, horizontalSpacing: 0, verticalSpacing: 0) {
                ForEach(rows.indices, id: \.self) { row in
                    GridRow {
                        ForEach(rows[row].indices, id: \.self) { column in
                            Cell(text: rows[row][column])
                                .frame(maxWidth: Metric.measure, alignment: .leading)
                                .padding(.vertical, Metric.tiny)
                                .padding(.horizontal, Metric.gap)
                                .frame(maxHeight: .infinity, alignment: .topLeading)
                                .border(flavour(.surface1), width: 0.5)
                        }
                    }
                    .background(row == 0 ? flavour(.mantle) : .clear)
                }
            }
            .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.bottom, Metric.pad)
    }

    /// One cell's text, already styled.
    private struct Cell: UIViewRepresentable {
        let text: NSAttributedString

        func makeUIView(context: Context) -> UILabel {
            let label = UILabel()
            label.numberOfLines = 0
            return label
        }

        func updateUIView(_ label: UILabel, context: Context) {
            if label.attributedText != text { label.attributedText = text }
        }

        func sizeThatFits(_ proposal: ProposedViewSize, uiView label: UILabel, context: Context) -> CGSize? {
            let width = proposal.width.flatMap { $0.isFinite && $0 > 0 ? min($0, Metric.measure) : nil } ?? Metric.measure
            let fitted = Prose.width(of: text, in: width)
            let size = text.boundingRect(with: CGSize(width: fitted, height: .greatestFiniteMagnitude), options: [.usesLineFragmentOrigin, .usesFontLeading], context: nil)
            return CGSize(width: fitted, height: ceil(size.height))
        }
    }
}
#endif

