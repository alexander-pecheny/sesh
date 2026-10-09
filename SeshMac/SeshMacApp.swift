import SwiftUI

@main
struct SeshMacApp: App {
    @StateObject private var library = Library()

    var body: some Scene {
        WindowGroup(id: "main") {
            root
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

    @ViewBuilder private var root: some View {
        #if DEBUG
        if let file = UserDefaults.standard.string(forKey: "markdown") {
            MarkdownWindow(file: file)
        } else {
            RootView().environmentObject(library)
        }
        #else
        RootView().environmentObject(library)
        #endif
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
/// `-markdown FILE` shows the file as an Agent's reply in a Conversation, `-width` points wide,
/// so a reply's drawing can be checked with `-snapshot` and no Host.
private struct MarkdownWindow: View {
    @StateObject private var conversation = Conversation(pane: "markdown", agent: nil, runner: nil)
    let file: String

    var body: some View {
        let width = UserDefaults.standard.double(forKey: "width")
        ConversationView(conversation: conversation, title: file, fresh: false)
            .frame(width: width > 0 ? width : 900, height: 1000)
            .task {
                let text = (try? String(contentsOfFile: file, encoding: .utf8)) ?? ""
                let entry: [String: Any] = ["t": "entry", "id": "md", "kind": "text", "summary": "", "at": "2026-10-09T12:00:00Z", "text": text]
                guard let data = try? JSONSerialization.data(withJSONObject: entry) else { return }
                conversation.receive(String(decoding: data, as: UTF8.self))
                conversation.receive(#"{"t": "cursor", "cursor": "0"}"#)
            }
    }
}

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

extension Library {
    /// The selected Task's current Tab, unless it is the Journal, which never closes.
    func closeCurrent() {
        guard let task = selection, let tab = current[task], tab != .journal else { return }
        close(tab, in: task)
    }

    /// Moves to the selected Task's next or previous Tab, round from the last to the Journal.
    func cycle(by step: Int) {
        guard let task = selection else { return }
        let all = [TabItem.journal] + (tabs[task] ?? [])
        let index = all.firstIndex(of: current[task] ?? .journal) ?? 0
        current[task] = all[(index + step + all.count) % all.count]
    }
}
