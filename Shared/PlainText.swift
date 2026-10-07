import SwiftUI

#if os(iOS)

/// Text for a shell or an Agent, so no autocorrect and no typographic quotes or dashes. Unlike
/// TextEditor, it keeps the caret in sight when a taller keyboard shrinks it or an Upload path lands.
final class PlainField: UITextView, ObservableObject {
    private var height = 0.0

    override func layoutSubviews() {
        super.layoutSubviews()
        if bounds.height < height, isFirstResponder { scrollRangeToVisible(selectedRange) }
        height = bounds.height
    }

    /// At the caret, over any selection, and never glued to the word in front of it.
    func insertPaths(_ paths: String) {
        let before = (text as NSString).substring(to: selectedRange.location)
        let gap = before.last.map { $0.isWhitespace } ?? true
        insertText((gap ? "" : " ") + paths)
    }

    func focus() { becomeFirstResponder() }
}

struct PlainText: UIViewRepresentable {
    let field: PlainField
    @Binding var text: String
    var font = UIFont.preferredFont(forTextStyle: .body)
    /// Grows with its text up to this many lines; without it, fills what it is given.
    var lines: Int?

    func makeUIView(context: Context) -> PlainField {
        field.font = font
        field.adjustsFontForContentSizeCategory = true
        field.autocorrectionType = .no
        field.autocapitalizationType = .none
        field.spellCheckingType = .no
        field.smartQuotesType = .no
        field.smartDashesType = .no
        field.smartInsertDeleteType = .no
        field.backgroundColor = .clear
        field.delegate = context.coordinator
        if lines != nil {
            field.textContainerInset = .zero
            field.textContainer.lineFragmentPadding = 0
        }
        return field
    }

    func updateUIView(_ field: PlainField, context: Context) {
        if field.text != text { field.text = text }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, uiView field: PlainField, context: Context) -> CGSize? {
        guard let lines, let width = proposal.width else { return nil }
        let fit = field.sizeThatFits(CGSize(width: width, height: .greatestFiniteMagnitude)).height
        let most = font.lineHeight * CGFloat(lines)
        field.isScrollEnabled = fit > most
        return CGSize(width: width, height: min(fit, most))
    }

    var submit: (() -> Void)?

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, UITextViewDelegate {
        let text: Binding<String>
        init(text: Binding<String>) { self.text = text }
        func textViewDidChange(_ field: UITextView) { text.wrappedValue = field.text }
    }
}
#else
/// The Mac's text for a shell or an Agent: no autocorrect, no typographic quotes or dashes.
final class PlainField: NSTextView, ObservableObject {
    private static let returnKey: UInt16 = 36
    var submit: (() -> Void)?
    /// Takes a pasted image and returns where it now lives, for its path to go in the text.
    var pasteImage: ((Data, String) async -> String?)?

    override func paste(_ sender: Any?) {
        guard let pasteImage, let (data, ext) = PastedImage.read(.general) else { return super.paste(sender) }
        Task { @MainActor in
            if let path = await pasteImage(data, ext) { insertPaths(quote(path)) }
        }
    }

    convenience init() {
        self.init(frame: .zero)
        isRichText = false
        allowsUndo = true
        isAutomaticSpellingCorrectionEnabled = false
        isContinuousSpellCheckingEnabled = false
        isAutomaticQuoteSubstitutionEnabled = false
        isAutomaticDashSubstitutionEnabled = false
        isAutomaticTextReplacementEnabled = false
        usesFindBar = true
        isIncrementalSearchingEnabled = true
        smartInsertDeleteEnabled = false
        drawsBackground = false
        isVerticallyResizable = true
        autoresizingMask = [.width]
        textContainer?.widthTracksTextView = true
    }

    /// Return sends when there is somewhere to send; shift or option with it starts a new line.
    override func keyDown(with event: NSEvent) {
        let plain = event.modifierFlags.intersection([.shift, .option, .command, .control]).isEmpty
        if event.keyCode == Self.returnKey, plain, let submit { return submit() }
        super.keyDown(with: event)
    }

    func insertPaths(_ paths: String) {
        let before = (string as NSString).substring(to: selectedRange().location)
        let gap = before.last.map { $0.isWhitespace } ?? true
        insertText((gap ? "" : " ") + paths, replacementRange: selectedRange())
    }

    func focus() { window?.makeFirstResponder(self) }

    var lineHeight: CGFloat { layoutManager?.defaultLineHeight(for: font ?? .systemFont(ofSize: 13)) ?? 17 }

    /// Measured from the text, not the layout, so asking never resizes anything mid-layout.
    func fittingHeight(width: CGFloat) -> CGFloat {
        let text = string.hasSuffix("\n") || string.isEmpty ? string + " " : string
        let box = (text as NSString).boundingRect(
            with: NSSize(width: width, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: font ?? .systemFont(ofSize: NSFont.systemFontSize)])
        return max(lineHeight, ceil(box.height))
    }
}

struct PlainText: NSViewRepresentable {
    let field: PlainField
    @Binding var text: String
    var font = NSFont.systemFont(ofSize: NSFont.systemFontSize)
    /// Grows with its text up to this many lines; without it, fills what it is given.
    var lines: Int?
    var submit: (() -> Void)?

    func makeNSView(context: Context) -> NSScrollView {
        field.font = font
        field.delegate = context.coordinator
        field.textContainerInset = lines == nil ? NSSize(width: 4, height: 6) : .zero
        if lines != nil { field.textContainer?.lineFragmentPadding = 0 }
        let scroll = NSScrollView()
        scroll.documentView = field
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        if field.string != text { field.string = text }
        field.submit = submit
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView scroll: NSScrollView, context: Context) -> CGSize? {
        guard let lines, let width = proposal.width else { return nil }
        return CGSize(width: width, height: min(field.fittingHeight(width: width), field.lineHeight * CGFloat(lines)))
    }

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, NSTextViewDelegate {
        let text: Binding<String>
        init(text: Binding<String>) { self.text = text }
        func textDidChange(_ notification: Notification) {
            guard let field = notification.object as? NSTextView else { return }
            text.wrappedValue = field.string
        }
    }
}
#endif

#if os(macOS)
/// An image on the pasteboard, as PNG, or a copied image file as it is.
enum PastedImage {
    static func read(_ board: NSPasteboard) -> (Data, String)? {
        if let urls = board.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
           let url = urls.first, ["png", "jpg", "jpeg", "gif", "heic", "webp"].contains(url.pathExtension.lowercased()),
           let data = try? Data(contentsOf: url) {
            return (data, url.pathExtension.lowercased())
        }
        guard board.string(forType: .string) == nil else { return nil }
        if let png = board.data(forType: .png) { return (png, "png") }
        if let tiff = board.data(forType: .tiff), let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) {
            return (png, "png")
        }
        return nil
    }
}
#endif
