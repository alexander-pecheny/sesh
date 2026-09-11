import SwiftUI

extension Ghostty {
    struct Terminal: UIViewRepresentable {
        let view: TerminalView

        func makeUIView(context: Context) -> TerminalView { view }
        func updateUIView(_ view: TerminalView, context: Context) {}
    }
}
