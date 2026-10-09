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

    /// Says what a closed Task's cleanup could not do, and offers to try it again.
    func reportsCleanup(_ library: Library) -> some View {
        alert("Sesh could not finish closing it", isPresented: Binding(
            get: { library.cleanupProblem != nil }, set: { if !$0 { library.cleanupProblem = nil } }
        ), presenting: library.cleanupProblem) { problem in
            Button("Retry", action: problem.retry)
            Button("OK", role: .cancel) {}
        } message: { Text($0.message) }
    }
}
