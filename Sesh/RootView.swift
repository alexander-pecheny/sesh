import SwiftUI

struct RootView: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var ghostty: Ghostty.App
    @StateObject private var store = Store()
    @StateObject private var tabs = Tabs()

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        ZStack {
            flavour(.base).ignoresSafeArea()
            #if DEBUG
            if let fixture = UserDefaults.standard.string(forKey: "fixture") {
                FixtureView(name: fixture)
            } else {
                content
            }
            #else
            content
            #endif
        }
        .environmentObject(store)
    }

    @ViewBuilder private var content: some View {
        if let app = ghostty.app {
            if tabs.active != nil {
                TabsView(tabs: tabs)
            } else {
                HostListView(tabs: tabs) { tabs.open($0, $1, store: store, app: app) }
            }
        } else {
            Text("libghostty failed to start").foregroundStyle(flavour(.red))
        }
    }
}
