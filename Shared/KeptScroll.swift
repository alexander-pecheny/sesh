import SwiftUI

/// Where a Document was scrolled to. SwiftUI builds its Tab afresh on every switch back, so the
/// offset lives with the Document and is put back once the view has text to scroll.
extension View {
    func keepsScroll(_ offset: Binding<CGPoint>) -> some View { modifier(KeptScroll(offset: offset)) }
}

private struct KeptScroll: ViewModifier {
    @Binding var offset: CGPoint
    @State private var position = ScrollPosition()
    @State private var restored = false

    func body(content: Content) -> some View {
        content
            .scrollPosition($position)
            .onScrollGeometryChange(for: CGPoint.self, of: { $0.contentOffset }) { _, now in
                if restored { offset = now }
            }
            .onAppear {
                let saved = offset
                DispatchQueue.main.async {
                    position.scrollTo(point: saved)
                    restored = true
                }
            }
    }
}

/// The same for a scroll view of AppKit or UIKit.
@MainActor
final class ScrollMemory {
    private var started = false
    private var watch: NSObjectProtocol?
    #if os(iOS)
    private var observation: NSKeyValueObservation?
    #endif

    #if os(macOS)
    func keep(_ scroll: NSScrollView, in offset: Binding<CGPoint>) {
        guard !started else { return }
        started = true
        let saved = offset.wrappedValue
        DispatchQueue.main.async { [weak scroll] in
            guard let scroll else { return }
            if let text = scroll.documentView as? NSTextView, let container = text.textContainer {
                text.layoutManager?.ensureLayout(for: container)
            }
            scroll.contentView.scroll(to: saved)
            scroll.reflectScrolledClipView(scroll.contentView)
            scroll.contentView.postsBoundsChangedNotifications = true
            self.watch = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: scroll.contentView, queue: .main
            ) { [weak scroll] _ in
                MainActor.assumeIsolated { if let scroll { offset.wrappedValue = scroll.contentView.bounds.origin } }
            }
        }
    }
    #else
    func keep(_ scroll: UIScrollView, in offset: Binding<CGPoint>) {
        guard !started else { return }
        started = true
        let saved = offset.wrappedValue
        DispatchQueue.main.async { [weak scroll] in
            guard let scroll else { return }
            scroll.layoutIfNeeded()
            scroll.contentOffset = saved
            self.observation = scroll.observe(\.contentOffset) { scroll, _ in
                MainActor.assumeIsolated { offset.wrappedValue = scroll.contentOffset }
            }
        }
    }
    #endif

    deinit {
        if let watch { NotificationCenter.default.removeObserver(watch) }
    }
}
