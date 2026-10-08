import SwiftUI

extension View {
    /// Asks before a Tab closes with unsaved edits or a running shell in it.
    func confirmsClosing(_ library: Library) -> some View {
        alert("Close this Tab?", isPresented: Binding(get: { library.closing != nil }, set: { if !$0 { library.closing = nil } }),
              presenting: library.closing) { closing in
            Button("Close", role: .destructive) { library.close(closing.tab, in: closing.task, confirmed: true) }
            Button("Cancel", role: .cancel) {}
        } message: { closing in
            Text(closing.loss)
        }
    }
}
