import SwiftUI

#if os(iOS)
typealias PlatformImage = UIImage

extension Image {
    init(platform image: PlatformImage) { self.init(uiImage: image) }
}

extension View {
    func inlineTitle() -> some View { navigationBarTitleDisplayMode(.inline) }

    func fitWidth() -> some View { containerRelativeFrame(.horizontal) }

    func readable(alignment: Alignment = .leading) -> some View { frame(maxWidth: .infinity, alignment: alignment) }

    /// The whole screen on the phone; a sheet on the Mac, where nothing covers the window.
    func cover<Content: View>(isPresented: Binding<Bool>, @ViewBuilder content: @escaping () -> Content) -> some View {
        fullScreenCover(isPresented: isPresented, content: content)
    }
}
#else
typealias PlatformImage = NSImage

extension Image {
    init(platform image: PlatformImage) { self.init(nsImage: image) }
}

extension View {
    func inlineTitle() -> some View { self }

    /// The window is the container on the Mac, wider than a split view's column.
    func fitWidth() -> some View { frame(maxWidth: .infinity) }

    /// Prose no wider than `Metric.measure`, which is hard to read beyond that on a wide
    /// window: the Agent's on the left, the user's on the right, as in a chat.
    func readable(alignment: Alignment = .leading) -> some View {
        frame(maxWidth: Metric.measure, alignment: alignment).frame(maxWidth: .infinity, alignment: alignment)
    }

    func cover<Content: View>(isPresented: Binding<Bool>, @ViewBuilder content: @escaping () -> Content) -> some View {
        sheet(isPresented: isPresented, content: content)
    }
}
#endif

extension Image {
    /// A Lucide icon from the asset catalog, sized like a symbol of that point size.
    static func lucide(_ name: String, size: CGFloat = 17) -> some View {
        Image(name).resizable().frame(width: size, height: size)
    }
}

extension Font {
    static func ui(_ size: CGFloat) -> Font { .system(size: size) }
}

/// Sizes the Conversation and Projects share, so their rows and cards line up.
enum Metric {
    static let tiny: CGFloat = 4
    static let gap: CGFloat = 8
    static let pad: CGFloat = 12
    static let wide: CGFloat = 16
    static let corner: CGFloat = 10
    static let control: CGFloat = 38
    static let small: CGFloat = 11
    static let caption: CGFloat = 12
    static let note: CGFloat = 13
    static let label: CGFloat = 14
    static let body: CGFloat = 15
    static let title: CGFloat = 16
    #if os(macOS)
    /// Room for 99 characters of ordinary prose, measured on a sentence rather than on "0",
    /// which is wider than the average letter.
    static let measure: CGFloat = {
        let sample = "The quick brown fox jumps over the lazy dog, then naps by the river."
        let width = (sample as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: body)]).width
        return (99 * width / CGFloat(sample.count)).rounded(.up)
    }()
    #endif
}

extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}
