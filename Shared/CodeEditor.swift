import SwiftUI

/// A source file to edit, numbered and coloured, with the line shortcuts of VS Code: Tab and ⇧Tab
/// or ⌘] and ⌘[ to indent, ⌘/ to comment, ⌥↑↓ to move lines, ⇧⌥↑↓ to copy them, ⇧⌘K to delete
/// them, ⌘L to select them, and ⌘Return or ⇧⌘Return to start a line below or above.
struct CodeEditor {
    @Environment(\.colorScheme) private var colorScheme
    @Binding var text: String
    let path: String
}

private enum CodeKey: String, CaseIterable {
    case indent, outdent, comment, moveUp, moveDown, copyUp, copyDown, delete, selectLine, below, above

    var title: String {
        switch self {
        case .indent: "Indent Lines"
        case .outdent: "Outdent Lines"
        case .comment: "Toggle Comment"
        case .moveUp: "Move Lines Up"
        case .moveDown: "Move Lines Down"
        case .copyUp: "Copy Lines Up"
        case .copyDown: "Copy Lines Down"
        case .delete: "Delete Lines"
        case .selectLine: "Select Line"
        case .below: "Insert Line Below"
        case .above: "Insert Line Above"
        }
    }
}

/// What both platforms' editors know of the file beyond its text.
@MainActor
private final class CodeModel {
    static let size: CGFloat = {
        #if os(macOS)
        Metric.note
        #else
        Metric.caption
        #endif
    }()
    static let font = PlatformFont.monospacedSystemFont(ofSize: size, weight: .regular)
    static let digits = PlatformFont.monospacedDigitSystemFont(ofSize: Metric.small, weight: .regular)

    let language: String
    let marker: String?
    var unit = "    "
    var starts = [0]
    var dark: Bool?
    var flavour: Catppuccin.Flavour { dark == true ? .mocha : .latte }

    init(path: String) {
        language = (path as NSString).pathExtension
        marker = CodeEdits.commentMarker(for: path)
    }

    func edit(_ key: CodeKey, _ s: NSString, _ sel: NSRange) -> CodeEdit? {
        switch key {
        case .indent: CodeEdits.indent(s, sel, unit: unit)
        case .outdent: CodeEdits.outdent(s, sel, unit: unit)
        case .comment: marker.map { CodeEdits.toggleComment(s, sel, marker: $0) }
        case .moveUp: CodeEdits.move(s, sel, up: true)
        case .moveDown: CodeEdits.move(s, sel, up: false)
        case .copyUp: CodeEdits.copy(s, sel, up: true)
        case .copyDown: CodeEdits.copy(s, sel, up: false)
        case .delete: CodeEdits.deleteLines(s, sel)
        case .selectLine: CodeEdit(range: NSRange(location: sel.location, length: 0), text: "", selection: CodeEdits.selectLine(s, sel))
        case .below: CodeEdits.openLine(s, sel, above: false)
        case .above: CodeEdits.openLine(s, sel, above: true)
        }
    }

    /// The edit Return or Tab makes in place of typing, if any.
    func typed(_ text: String, _ s: NSString, _ sel: NSRange) -> CodeEdit? {
        switch text {
        case "\n": CodeEdits.newline(s, sel, unit: unit)
        case "\t": CodeEdits.tab(s, sel, unit: unit)
        default: nil
        }
    }

    func line(at offset: Int) -> Int {
        var (low, high) = (0, starts.count)
        while high - low > 1 {
            let mid = (low + high) / 2
            if starts[mid] <= offset { low = mid } else { high = mid }
        }
        return low + 1
    }

    var gutterWidth: CGFloat {
        let digit = ("0" as NSString).size(withAttributes: [.font: Self.digits]).width
        return CGFloat(max(3, String(starts.count).count)) * digit + 2 * Metric.gap
    }

    /// Numbers the lines that start inside `visible`, a rectangle of the text container, at the
    /// top of their first line fragment.
    func numbers(_ layout: NSLayoutManager, _ container: NSTextContainer, visible: CGRect, caret: Int) -> [(NSAttributedString, CGRect)] {
        let length = layout.textStorage?.length ?? 0
        let chars = layout.characterRange(forGlyphRange: layout.glyphRange(forBoundingRect: visible, in: container), actualGlyphRange: nil)
        var index = line(at: chars.location) - 1
        if starts[index] < chars.location { index += 1 }
        let current = line(at: caret)
        var out: [(NSAttributedString, CGRect)] = []
        while index < starts.count, starts[index] <= min(NSMaxRange(chars), length) {
            let at = starts[index]
            let rect = at < length
                ? layout.lineFragmentRect(forGlyphAt: layout.glyphIndexForCharacter(at: at), effectiveRange: nil)
                : layout.extraLineFragmentRect
            index += 1
            let colour = PlatformColor(flavour(index == current ? .text : .overlay0))
            out.append((NSAttributedString(string: "\(index)", attributes: [.font: Self.digits, .foregroundColor: colour]), rect))
        }
        return out
    }

    /// highlight.js's colours for the text, or none for a language it does not know.
    func colours(_ text: String) -> [(PlatformColor, NSRange)] {
        guard let coloured = Highlight.colour(text, language: language, dark: dark == true, size: Self.size) else { return [] }
        var out: [(PlatformColor, NSRange)] = []
        coloured.enumerateAttribute(.foregroundColor, in: NSRange(location: 0, length: coloured.length)) { value, range, _ in
            if let colour = value as? PlatformColor { out.append((colour, range)) }
        }
        return out
    }
}

#if os(macOS)
extension CodeEditor: NSViewRepresentable {
    func makeNSView(context: Context) -> NSScrollView {
        let view = CodeTextView(path: path)
        let scroll = NSScrollView()
        scroll.documentView = view
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.verticalRulerView = Gutter(view: view, scroll: scroll)
        scroll.hasVerticalRuler = true
        scroll.rulersVisible = true
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let view = scroll.documentView as? CodeTextView else { return }
        view.changed = { text = $0 }
        view.show(text, dark: colorScheme == .dark)
    }
}

private final class CodeTextView: NSTextView {
    private static let up: UInt16 = 126
    private static let down: UInt16 = 125
    private static let returns: Set<UInt16> = [36, 76]

    let model: CodeModel
    var changed: (String) -> Void = { _ in }
    weak var gutter: Gutter?
    private var pending: DispatchWorkItem?

    init(path: String) {
        model = CodeModel(path: path)
        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(size: NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layout.addTextContainer(container)
        super.init(frame: .zero, textContainer: container)
        font = CodeModel.font
        isRichText = false
        allowsUndo = true
        isAutomaticSpellingCorrectionEnabled = false
        isContinuousSpellCheckingEnabled = false
        isAutomaticQuoteSubstitutionEnabled = false
        isAutomaticDashSubstitutionEnabled = false
        isAutomaticTextReplacementEnabled = false
        smartInsertDeleteEnabled = false
        usesFindBar = true
        isIncrementalSearchingEnabled = true
        drawsBackground = false
        isHorizontallyResizable = true
        isVerticallyResizable = true
        maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude)
        autoresizingMask = [.width, .height]
    }

    required init?(coder: NSCoder) { nil }

    func show(_ text: String, dark: Bool) {
        let restyle = dark != model.dark
        if restyle {
            model.dark = dark
            textColor = PlatformColor(model.flavour(.text))
            insertionPointColor = PlatformColor(model.flavour(.text))
        }
        if string != text {
            string = text
            model.unit = CodeEdits.unit(of: text)
            refresh(after: 0)
        } else if restyle {
            refresh(after: 0)
        }
    }

    override func didChangeText() {
        super.didChangeText()
        changed(string)
        refresh(after: 0.3)
    }

    override func setSelectedRanges(_ ranges: [NSValue], affinity: NSSelectionAffinity, stillSelecting: Bool) {
        super.setSelectedRanges(ranges, affinity: affinity, stillSelecting: stillSelecting)
        gutter?.needsDisplay = true
    }

    /// Numbers the lines at once, and colours them once typing pauses: highlight.js is too slow for every key.
    private func refresh(after delay: TimeInterval) {
        model.starts = CodeEdits.lineStarts(string as NSString)
        gutter?.fit()
        pending?.cancel()
        let paint = DispatchWorkItem { [weak self] in self?.paint() }
        pending = paint
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: paint)
    }

    /// Temporary attributes colour the text without editing it, so undo never sees them.
    private func paint() {
        guard let layout = layoutManager else { return }
        let all = NSRange(location: 0, length: (string as NSString).length)
        layout.removeTemporaryAttribute(.foregroundColor, forCharacterRange: all)
        for (colour, range) in model.colours(string) where NSMaxRange(range) <= all.length {
            layout.addTemporaryAttribute(.foregroundColor, value: colour, forCharacterRange: range)
        }
    }

    private func key(for event: NSEvent) -> CodeKey? {
        let mods = event.modifierFlags.intersection([.command, .shift, .option, .control])
        let char = event.charactersIgnoringModifiers?.lowercased()
        let enter = Self.returns.contains(event.keyCode)
        switch mods {
        case [.command] where char == "]": return .indent
        case [.command] where char == "[": return .outdent
        case [.command] where char == "/": return .comment
        case [.command] where char == "l": return .selectLine
        case [.command] where enter: return .below
        case [.command, .shift] where enter: return .above
        case [.command, .shift] where char == "k": return .delete
        case [.option] where event.keyCode == Self.up: return .moveUp
        case [.option] where event.keyCode == Self.down: return .moveDown
        case [.option, .shift] where event.keyCode == Self.up: return .copyUp
        case [.option, .shift] where event.keyCode == Self.down: return .copyDown
        default: return nil
        }
    }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard window?.firstResponder === self, isEditable, let key = key(for: event) else { return super.performKeyEquivalent(with: event) }
        apply(model.edit(key, string as NSString, selectedRange()))
        return true
    }

    override func keyDown(with event: NSEvent) {
        guard !hasMarkedText(), let key = key(for: event) else { return super.keyDown(with: event) }
        apply(model.edit(key, string as NSString, selectedRange()))
    }

    override func insertTab(_ sender: Any?) { apply(model.typed("\t", string as NSString, selectedRange())) }
    override func insertBacktab(_ sender: Any?) { apply(model.edit(.outdent, string as NSString, selectedRange())) }
    override func insertNewline(_ sender: Any?) { apply(model.typed("\n", string as NSString, selectedRange())) }

    override func deleteBackward(_ sender: Any?) {
        guard let edit = CodeEdits.backspace(string as NSString, selectedRange(), unit: model.unit) else { return super.deleteBackward(sender) }
        apply(edit)
    }

    private func apply(_ edit: CodeEdit?) {
        guard let edit else { return }
        if edit.range.length > 0 || !edit.text.isEmpty {
            breakUndoCoalescing()
            guard shouldChangeText(in: edit.range, replacementString: edit.text) else { return }
            textStorage?.replaceCharacters(in: edit.range, with: edit.text)
            didChangeText()
        }
        setSelectedRange(edit.selection)
        scrollRangeToVisible(edit.selection)
    }
}

private final class Gutter: NSRulerView {
    private weak var view: CodeTextView?

    init(view: CodeTextView, scroll: NSScrollView) {
        self.view = view
        super.init(scrollView: scroll, orientation: .verticalRuler)
        clientView = view
        reservedThicknessForMarkers = 0
        clipsToBounds = true
        view.gutter = self
        fit()
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(self, selector: #selector(scrolled), name: NSView.boundsDidChangeNotification, object: scroll.contentView)
    }

    required init(coder: NSCoder) { fatalError("Gutter is made in code") }

    override var isFlipped: Bool { true }

    @objc private func scrolled() { needsDisplay = true }

    func fit() {
        let width = view?.model.gutterWidth ?? 0
        if ruleThickness != width {
            ruleThickness = width
            // The ruler floats over the text rather than taking room beside it.
            view?.textContainerInset = NSSize(width: width + Metric.tiny, height: Metric.gap)
        }
        needsDisplay = true
    }

    override func draw(_ dirty: NSRect) {
        guard let view, let layout = view.layoutManager, let container = view.textContainer else { return }
        PlatformColor(view.model.flavour(.base)).setFill()
        bounds.intersection(dirty).fill()
        let origin = view.textContainerOrigin
        let visible = view.visibleRect.offsetBy(dx: -origin.x, dy: -origin.y)
        for (label, line) in view.model.numbers(layout, container, visible: visible, caret: view.selectedRange().location) {
            let top = convert(NSPoint(x: 0, y: line.minY + origin.y), from: view).y
            let size = label.size()
            label.draw(at: NSPoint(x: ruleThickness - size.width - Metric.gap, y: top + (line.height - size.height) / 2))
        }
    }
}
#else
extension CodeEditor: UIViewRepresentable {
    func makeUIView(context: Context) -> CodeField { CodeField(path: path) }

    func updateUIView(_ field: CodeField, context: Context) {
        field.changed = { text = $0 }
        field.show(text, dark: colorScheme == .dark)
    }
}

final class CodeField: UITextView, UITextViewDelegate {
    fileprivate let model: CodeModel
    var changed: (String) -> Void = { _ in }
    private let gutter = Gutter()
    private var pending: DispatchWorkItem?
    private var applying = false

    init(path: String) {
        model = CodeModel(path: path)
        let storage = NSTextStorage()
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let container = NSTextContainer(size: .zero)
        container.widthTracksTextView = true
        layout.addTextContainer(container)
        super.init(frame: .zero, textContainer: container)
        font = CodeModel.font
        autocorrectionType = .no
        autocapitalizationType = .none
        spellCheckingType = .no
        smartQuotesType = .no
        smartDashesType = .no
        smartInsertDeleteType = .no
        backgroundColor = .clear
        keyboardDismissMode = .interactive
        delegate = self
        gutter.field = self
        addSubview(gutter)
    }

    required init?(coder: NSCoder) { nil }

    func show(_ text: String, dark: Bool) {
        let restyle = dark != model.dark
        if restyle {
            model.dark = dark
            textColor = PlatformColor(model.flavour(.text))
        }
        if self.text != text {
            self.text = text
            model.unit = CodeEdits.unit(of: text)
            refresh(after: 0)
        } else if restyle {
            refresh(after: 0)
        }
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        gutter.frame = CGRect(x: 0, y: contentOffset.y, width: model.gutterWidth, height: bounds.height)
        bringSubviewToFront(gutter)
        gutter.setNeedsDisplay()
    }

    override var keyCommands: [UIKeyCommand]? {
        let keys: [(CodeKey, String, UIKeyModifierFlags)] = [
            (.outdent, "\t", .shift), (.indent, "]", .command), (.outdent, "[", .command), (.comment, "/", .command),
            (.moveUp, UIKeyCommand.inputUpArrow, .alternate), (.moveDown, UIKeyCommand.inputDownArrow, .alternate),
            (.copyUp, UIKeyCommand.inputUpArrow, [.alternate, .shift]), (.copyDown, UIKeyCommand.inputDownArrow, [.alternate, .shift]),
            (.delete, "k", [.command, .shift]), (.selectLine, "l", .command), (.below, "\r", .command), (.above, "\r", [.command, .shift]),
        ]
        return (super.keyCommands ?? []) + keys.map { key, input, mods in
            let command = UIKeyCommand(title: key.title, action: #selector(shortcut), input: input, modifierFlags: mods, propertyList: key.rawValue)
            command.wantsPriorityOverSystemBehavior = true
            return command
        }
    }

    @objc private func shortcut(_ command: UIKeyCommand) {
        guard let name = command.propertyList as? String, let key = CodeKey(rawValue: name) else { return }
        apply(model.edit(key, text as NSString, selectedRange))
    }

    func textView(_ view: UITextView, shouldChangeTextIn range: NSRange, replacementText typed: String) -> Bool {
        guard !applying, markedTextRange == nil else { return true }
        let s = text as NSString
        let backspace = typed.isEmpty && range.length == 1 && selectedRange == NSRange(location: NSMaxRange(range), length: 0)
        guard let edit = backspace ? CodeEdits.backspace(s, selectedRange, unit: model.unit) : model.typed(typed, s, selectedRange) else { return true }
        apply(edit)
        return false
    }

    func textViewDidChange(_ view: UITextView) {
        changed(text)
        refresh(after: 0.3)
    }

    func textViewDidChangeSelection(_ view: UITextView) { gutter.setNeedsDisplay() }

    private func apply(_ edit: CodeEdit?) {
        guard let edit else { return }
        if edit.range.length > 0 || !edit.text.isEmpty,
           let start = position(from: beginningOfDocument, offset: edit.range.location),
           let end = position(from: start, offset: edit.range.length),
           let range = textRange(from: start, to: end) {
            applying = true
            replace(range, withText: edit.text)
            applying = false
            textViewDidChange(self)
        }
        selectedRange = edit.selection
        scrollRangeToVisible(edit.selection)
    }

    private func refresh(after delay: TimeInterval) {
        model.starts = CodeEdits.lineStarts(text as NSString)
        textContainerInset = UIEdgeInsets(top: Metric.gap, left: model.gutterWidth + Metric.tiny, bottom: Metric.gap, right: Metric.gap)
        setNeedsLayout()
        pending?.cancel()
        let paint = DispatchWorkItem { [weak self] in self?.paint() }
        pending = paint
        DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: paint)
    }

    private func paint() {
        let all = NSRange(location: 0, length: textStorage.length)
        let colours = model.colours(text)
        textStorage.beginEditing()
        textStorage.addAttribute(.foregroundColor, value: PlatformColor(model.flavour(.text)), range: all)
        for (colour, range) in colours where NSMaxRange(range) <= all.length {
            textStorage.addAttribute(.foregroundColor, value: colour, range: range)
        }
        textStorage.endEditing()
    }

    fileprivate final class Gutter: UIView {
        weak var field: CodeField?

        init() {
            super.init(frame: .zero)
            isOpaque = false
            isUserInteractionEnabled = false
            contentMode = .redraw
        }

        required init?(coder: NSCoder) { nil }

        override func draw(_ rect: CGRect) {
            guard let field else { return }
            PlatformColor(field.model.flavour(.base)).setFill()
            UIRectFill(rect)
            let inset = field.textContainerInset
            let visible = CGRect(x: 0, y: field.contentOffset.y - inset.top, width: field.bounds.width, height: field.bounds.height)
            for (label, line) in field.model.numbers(field.layoutManager, field.textContainer, visible: visible, caret: field.selectedRange.location) {
                let size = label.size()
                let top = line.minY + inset.top - field.contentOffset.y
                label.draw(at: CGPoint(x: bounds.width - size.width - Metric.gap, y: top + (line.height - size.height) / 2))
            }
        }
    }
}
#endif
