import SwiftUI
import GhosttyKit

/// One Tab. Until there is a transport, the surface talks to itself.
struct TerminalTab: View {
    @StateObject private var loopback: Loopback

    init(app: ghostty_app_t) {
        _loopback = StateObject(wrappedValue: Loopback(app: app))
    }

    var body: some View {
        Ghostty.Terminal(view: loopback.view)
    }
}

final class Loopback: ObservableObject {
    let view: Ghostty.TerminalView

    init(app: ghostty_app_t) {
        view = Ghostty.TerminalView(app: app)
        view.onWrite = { [weak view] data in
            view?.process(Data(data.flatMap { $0 == 0x0d ? [0x0d, 0x0a] : [$0] }))
        }
    }
}
