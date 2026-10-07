import SwiftUI

@main
struct SeshMacApp: App {
    @StateObject private var library = Library()

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(library)
                .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in
                    Machine.stopAll()
                }
                #if DEBUG
                .background(Offscreen())
                #endif
        }
        .commands {
            // Find in whatever text has the focus: a file, a Document, an editor.
            CommandGroup(after: .textEditing) {
                Button("Find…") { find(.showFindInterface) }.keyboardShortcut("f")
                Button("Find Next") { find(.nextMatch) }.keyboardShortcut("g")
                Button("Find Previous") { find(.previousMatch) }.keyboardShortcut("g", modifiers: [.command, .shift])
            }
        }
    }

    private func find(_ action: NSTextFinder.Action) {
        let sender = NSMenuItem()
        sender.tag = action.rawValue
        NSApp.sendAction(#selector(NSResponder.performTextFinderAction(_:)), to: nil, from: sender)
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
