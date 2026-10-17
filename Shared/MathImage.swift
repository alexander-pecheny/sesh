import SwiftMath
#if os(macOS)
import AppKit
#else
import UIKit
#endif

/// A LaTeX formula drawn by SwiftMath, and how far it reaches below the line it sits on.
@MainActor
enum MathImage {
    private static var drawn: [String: (image: PlatformImage, descent: CGFloat)] = [:]

    static func draw(_ latex: String, display: Bool, size: CGFloat, colour: PlatformColor) -> (image: PlatformImage, descent: CGFloat)? {
        let key = "\(display) \(size) \(colour) \(latex)"
        if let known = drawn[key] { return known }
        let latex = MathText.forSwiftMath(latex)
        let mode: MTMathUILabelMode = display ? .display : .text
        // The label lays the formula out, which tells the depth below the baseline that the
        // image alone does not.
        let label = MTMathUILabel()
        label.labelMode = mode
        label.fontSize = size
        label.latex = latex
        label.frame = CGRect(origin: .zero, size: label.intrinsicContentSize)
        #if os(macOS)
        label.layout()
        #else
        label.layoutSubviews()
        #endif
        guard label.error == nil, let descent = label.displayList?.descent,
              case (nil, let image?) = MTMathImage(latex: latex, fontSize: size, textColor: colour, labelMode: mode).asImage()
        else { return nil }
        drawn[key] = (image, descent)
        return (image, descent)
    }
}
