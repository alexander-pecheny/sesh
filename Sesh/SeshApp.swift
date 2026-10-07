import SwiftUI

@main
struct SeshApp: App {
    @StateObject private var ghostty = Ghostty.App()
    @StateObject private var library = Library()

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(ghostty).environmentObject(library).environmentObject(Store.shared)
        }
    }
}
