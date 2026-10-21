#if DEBUG
import SwiftUI

/// `-terminal` shows a terminal that echoes what is typed into it, with no Host, so a new
/// libghostty can be seen to draw and take input on iOS.
struct TerminalFixture: View {
    @EnvironmentObject private var ghostty: Ghostty.App
    @State private var view: Ghostty.TerminalView?

    var body: some View {
        ZStack {
            Color.clear
            if let view { Ghostty.Terminal(view: view) }
        }
        .onAppear {
            guard view == nil, let app = ghostty.app else { return }
            let view = Ghostty.TerminalView(app: app, input: InputState(), fontSize: 14)
            view.onWrite = { [weak view] data in
                view?.process(Data(String(decoding: data, as: UTF8.self).replacingOccurrences(of: "\r", with: "\r\n").utf8))
            }
            view.process(Data("\u{1b}[1;32mlibghostty draws\u{1b}[0m\r\n$ ".utf8))
            self.view = view
        }
    }
}
#endif
