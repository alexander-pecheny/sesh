import SwiftUI

struct RootView: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var ghostty: Ghostty.App
    @StateObject private var store = Store()
    @State private var connecting: Host?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        ZStack {
            flavour(.base).ignoresSafeArea()
            if ghostty.app != nil {
                HostListView(connecting: $connecting)
            } else {
                Text("libghostty failed to start").foregroundStyle(flavour(.red))
            }
        }
        .environmentObject(store)
        .fullScreenCover(item: $connecting) { host in
            SessionTab(host: host, store: store, app: ghostty.app!) { connecting = nil }
                .environmentObject(store)
        }
    }
}
