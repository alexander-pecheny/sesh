import SwiftUI

@main
struct SeshApp: App {
    @StateObject private var ghostty = Ghostty.App()

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(ghostty)
        }
    }
}
