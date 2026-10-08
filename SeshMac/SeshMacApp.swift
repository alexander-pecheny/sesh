import SwiftUI

@main
struct SeshMacApp: App {
    @StateObject private var library = Library()

    var body: some Scene {
        WindowGroup(id: "main") {
            RootView().environmentObject(library)
                .onOpenURL { _ = library.follow($0) }
                // A link opens in the window that is already there, not a new one.
                .handlesExternalEvents(preferring: ["*"], allowing: ["*"])
                .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in
                    Machine.stopAll()
                }
                #if DEBUG
                .background(Offscreen())
                #endif
        }
        // A first window fills the screen; after that macOS restores the size it was left at.
        .defaultWindowPlacement { _, context in
            WindowPlacement(context.defaultDisplay.visibleRect.origin, size: context.defaultDisplay.visibleRect.size)
        }
        .commands {
            // One window: its sidebar and Tabs would only mirror a second's.
            CommandGroup(replacing: .newItem) {
                Button("New Task…") { NotificationCenter.default.post(name: .newTask, object: nil) }.keyboardShortcut("n")
            }
            // Command-W closes a Tab, as in a browser, never the window.
            CommandGroup(replacing: .saveItem) {
                Button("Close Tab") { library.closeCurrent() }.keyboardShortcut("w")
            }
            CommandGroup(before: .windowArrangement) {
                Button("Show Next Tab") { library.cycle(by: 1) }.keyboardShortcut("]", modifiers: [.command, .shift])
                Button("Show Previous Tab") { library.cycle(by: -1) }.keyboardShortcut("[", modifiers: [.command, .shift])
                Divider()
            }
            // Find in whatever text has the focus: a file, a Document, an editor.
            CommandGroup(after: .textEditing) {
                Button("Find…") { find(.showFindInterface) }.keyboardShortcut("f")
                Button("Find Next") { find(.nextMatch) }.keyboardShortcut("g")
                Button("Find Previous") { find(.previousMatch) }.keyboardShortcut("g", modifiers: [.command, .shift])
                Button("Search Everything…") { NotificationCenter.default.post(name: .searchAll, object: nil) }
                    .keyboardShortcut("f", modifiers: [.command, .option])
            }
        }
    }

    private func find(_ action: NSTextFinder.Action) {
        let sender = NSMenuItem()
        sender.tag = action.rawValue
        NSApp.sendAction(#selector(NSResponder.performTextFinderAction(_:)), to: nil, from: sender)
    }
}

extension Notification.Name {
    static let newTask = Notification.Name("newTask")
    static let searchAll = Notification.Name("searchAll")
}

#if DEBUG
/// `-snapshot PATH` draws the window into a PNG every two seconds, or every
/// `-snapshotEvery SECONDS`, so a test can look at an app launched hidden (`open -j`) without
/// its window ever reaching the screen. A `{t}` in PATH becomes the time in milliseconds.
private struct Offscreen: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        guard let path = UserDefaults.standard.string(forKey: "snapshot") else { return view }
        let every = UserDefaults.standard.double(forKey: "snapshotEvery")
        Timer.scheduledTimer(withTimeInterval: every > 0 ? every : 2, repeats: true) { [weak view] _ in
            MainActor.assumeIsolated {
                guard let content = view?.window?.contentView,
                      let image = content.bitmapImageRepForCachingDisplay(in: content.bounds) else { return }
                content.cacheDisplay(in: content.bounds, to: image)
                let file = path.replacingOccurrences(of: "{t}", with: String(Int(Date().timeIntervalSince1970 * 1000)))
                try? image.representation(using: .png, properties: [:])?.write(to: URL(filePath: file))
            }
        }
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {}
}
#endif
