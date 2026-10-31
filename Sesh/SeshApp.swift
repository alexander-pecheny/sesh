import SwiftUI

@main
struct SeshApp: App {
    @StateObject private var ghostty = Ghostty.App()
    @StateObject private var library = Library()

    init() { Device.name() }

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(ghostty).environmentObject(library).environmentObject(Store.shared)
                .onOpenURL { _ = library.follow($0) }
        }
    }
}
