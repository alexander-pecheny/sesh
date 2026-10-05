import SwiftUI

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

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    final class Coordinator: NSObject, UITextViewDelegate {
        let text: Binding<String>
        init(text: Binding<String>) { self.text = text }
        func textViewDidChange(_ field: UITextView) { text.wrappedValue = field.text }
    }
}
