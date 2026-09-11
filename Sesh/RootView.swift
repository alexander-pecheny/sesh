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
            if let app = ghostty.app {
                if tabs.active != nil {
                    TabsView(tabs: tabs)
                } else {
                    HostListView(tabs: tabs) { tabs.open($0, store: store, app: app) }
                }
            } else {
                Text("libghostty failed to start").foregroundStyle(flavour(.red))
            }
        }
        .environmentObject(store)
    }
}
