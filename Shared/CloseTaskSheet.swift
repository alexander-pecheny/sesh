import SwiftUI

/// What closing a Task will do, said before it is done; uncommitted work in any Worktree it
/// removes must be given up explicitly.
struct CloseTaskSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    @ObservedObject var vault: Vault
    let task: Record
    @State private var plan: Close.Plan?
    @State private var discard = false

    private var ready: Bool { plan.map { $0.dirty.isEmpty || discard } ?? false }

    var body: some View {
        #if os(macOS)
        Form { sections }
            .formStyle(.grouped)
            .frame(width: 460)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Close Task", role: .destructive, action: close).disabled(!ready) }
            }
            .task { plan = await Close(task, in: vault).plan() }
        #else
        SheetForm(title: "Close Task", action: "Close", role: .destructive, enabled: ready, working: false, perform: close) { sections }
            .task { plan = await Close(task, in: vault).plan() }
        #endif
    }

    @ViewBuilder private var sections: some View {
        Section(task.body.title ?? "Task") {
            if let plan {
                Text(plan.sessions.count == 1 ? "Stops its Agent session." : "Stops its \(plan.sessions.count) Agent sessions.")
                ForEach(plan.removed) {
                    Text("Removes the Worktree at \($0.path) on \($0.place)" + ($0.branch.map { "; the branch \($0) stays." } ?? "."))
                }
                Text("Archives the Task. Its Journal, Documents and Conversations stay searchable.")
            } else {
                HStack { ProgressView().controlSize(.small); Text("Looking for uncommitted work…") }
            }
        }
        ForEach(plan?.dirty ?? []) { worktree in
            Section("Uncommitted changes in \(worktree.path)") {
                Text(worktree.uncommitted).font(.system(size: Metric.small, design: .monospaced)).textSelection(.enabled)
            }
        }
        if plan?.dirty.isEmpty == false {
            Section { Toggle("Discard them", isOn: $discard) }
        }
    }

    private func close() {
        guard let plan else { return }
        TaskActions.close(Close(task, in: vault), as: plan, in: vault, library: library, discard: discard)
        dismiss()
    }
}
