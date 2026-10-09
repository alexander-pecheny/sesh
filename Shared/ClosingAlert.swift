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

    /// Asks before a busy Agent is ended, and says why a resume failed.
    func confirmsEnding(_ library: Library) -> some View {
        alert("End this Agent session?", isPresented: Binding(get: { library.ending != nil }, set: { if !$0 { library.ending = nil } })) {
            Button("End", role: .destructive) {
                if let session = library.ending { Task { await library.end(session) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("The Agent is still at work and stops at once. Its Conversation stays in the Task, and Resume picks it up again.")
        }
        .alert("Sesh could not resume it", isPresented: Binding(get: { library.resumeProblem != nil }, set: { if !$0 { library.resumeProblem = nil } })) {
            Button("OK", role: .cancel) {}
        } message: { Text(library.resumeProblem ?? "") }
    }

    /// Asks before an Unfiled Agent is stopped; nothing of it is kept.
    func confirmsStopping(_ item: Library.Unfiled, isPresented: Binding<Bool>, library: Library) -> some View {
        confirmationDialog("Stop \(item.agent.title) in \(item.cwd)?", isPresented: isPresented, titleVisibility: .visible) {
            Button("Stop and Close", role: .destructive) { Task { await library.stop(item) } }
        } message: {
            Text("The Agent is interrupted and its herdr pane closed. Nothing of it is kept in a Vault, as it was never adopted.")
        }
    }
}
