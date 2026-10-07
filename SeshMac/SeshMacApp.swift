import SwiftUI

@main
struct SeshMacApp: App {
    @StateObject private var library = Library()

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(library)
                #if DEBUG
                .background(Offscreen())
                #endif
        }
    }
}

#if DEBUG
/// `-snapshot PATH` draws the window into a PNG every two seconds, so a test can look at an
/// app launched hidden (`open -j`) without its window ever reaching the screen.
private struct Offscreen: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        guard let path = UserDefaults.standard.string(forKey: "snapshot") else { return view }
        Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { [weak view] _ in
            MainActor.assumeIsolated {
                guard let content = view?.window?.contentView,
                      let image = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { return }
                content.cacheDisplay(in: content.bounds, to: image)
                try? image.representation(using: .png, properties: [:])?.write(to: URL(filePath: path))
            }
        }
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {}
}
#endif
