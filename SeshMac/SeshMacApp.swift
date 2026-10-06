import SwiftUI

@main
struct SeshMacApp: App {
    @StateObject private var library = Library()

    var body: some Scene {
        WindowGroup {
            RootView().environmentObject(library)
        }
    }
}
