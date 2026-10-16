#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// Lays out an Agent's Markdown with inline code drawn as a rounded chip, never broken across
/// lines. The renderer marks a chip with `key`, its border colour, and pads it with `pad`.
final class ChipLayout: NSLayoutManager, NSLayoutManagerDelegate {
    static let key = NSAttributedString.Key("chip")
    static let pad = "\u{202F}"
    private static let corner: CGFloat = 4

    override init() {
        super.init()
        delegate = self
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        delegate = self
    }

    override func fillBackgroundRectArray(_ rects: UnsafePointer<CGRect>, count: Int, forCharacterRange range: NSRange, color: PlatformColor) {
        #if os(macOS)
        let context = NSGraphicsContext.current?.cgContext
        #else
        let context = UIGraphicsGetCurrentContext()
        #endif
        guard let context, let border = chip(at: range.location) else {
            return super.fillBackgroundRectArray(rects, count: count, forCharacterRange: range, color: color)
        }
        for rect in UnsafeBufferPointer(start: rects, count: count) {
            let path = CGPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5), cornerWidth: Self.corner, cornerHeight: Self.corner, transform: nil)
            context.addPath(path)
            context.setFillColor(color.cgColor)
            context.fillPath()
            context.addPath(path)
            context.setStrokeColor(border.cgColor)
            context.setLineWidth(1)
            context.strokePath()
        }
    }

    func layoutManager(_ layoutManager: NSLayoutManager, shouldBreakLineByWordBeforeCharacterAt index: Int) -> Bool {
        index == 0 || chip(at: index) == nil || chip(at: index - 1) == nil
    }

    private func chip(at index: Int) -> PlatformColor? {
        guard let storage = textStorage, index < storage.length else { return nil }
        return storage.attribute(Self.key, at: index, effectiveRange: nil) as? PlatformColor
    }
}
