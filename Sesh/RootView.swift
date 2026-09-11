import SwiftUI

struct RootView: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var ghostty: Ghostty.App

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        ZStack {
            flavour(.base).ignoresSafeArea()
            if let app = ghostty.app {
                TerminalTab(app: app)
            } else {
                Text("libghostty failed to start").foregroundStyle(flavour(.red))
            }
        }
    }
}
