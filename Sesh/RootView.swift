import SwiftUI

/// The Vaults and their Tasks, and the chosen Task's Tabs: side by side on an iPad, one
/// pushed over the other on the phone.
struct RootView: View {
    @Environment(\.colorScheme) private var colorScheme
    @Environment(\.scenePhase) private var scenePhase
    @EnvironmentObject private var ghostty: Ghostty.App
    @EnvironmentObject private var library: Library
    @ObservedObject private var asking = Asking.shared

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
        .background { if let link = asking.link { Questions(link: link) } }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background: Machine.pause()
            case .active: Machine.resume()
            default: break
            }
        }
    }

    @ViewBuilder private var content: some View {
        if ghostty.app != nil {
            NavigationSplitView {
                TasksView()
            } detail: {
                if let id = library.selection, let vault = library.vault(of: id) {
                    TaskScreen(vault: vault, id: id).id(id)
                } else {
                    Text(library.vaults.isEmpty ? "Add a Vault to keep Tasks in." : "Pick a Task")
                        .font(.ui(Metric.body))
                        .foregroundStyle(flavour(.subtext0))
                }
            }
            .tint(flavour(.mauve))
        } else {
            Text("libghostty failed to start").foregroundStyle(flavour(.red))
        }
    }
}

/// What a Link asks before it can reach its Host: a new or changed host key, or a password.
private struct Questions: View {
    @ObservedObject var link: HostLink

    var body: some View {
        Color.clear
            .sheet(item: $link.hostKeyQuestion) { question in
                HostKeySheet(question: question, host: link.host) { link.answerHostKey($0) }
            }
            .sheet(item: $link.authQuestion) { question in
                AuthSheet(question: question, savePassword: $link.savePassword) { link.answerPrompt(question, $0) }
            }
    }
}
