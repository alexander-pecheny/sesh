import SwiftUI

/// Moves a Vault to the Host the user names, showing each step and then what is left to do.
struct MoveVaultSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var library: Library
    let vault: Vault
    @State private var alias = ""
    @State private var step: String?
    @State private var outcome: VaultMove.Outcome?

    private var trimmed: String { alias.trimmingCharacters(in: .whitespaces) }
    private var running: Bool { step != nil && outcome == nil }

    var body: some View {
        Form {
            if let outcome {
                Text(outcome.place.map { "The Vault \(vault.name) is on \($0.alias ?? "") now." } ?? "The Vault \(vault.name) stays on \(vault.machine.title).")
                    .font(.headline)
                ForEach(outcome.notes, id: \.self) { Text($0).textSelection(.enabled) }
            } else {
                TextField("Host", text: $alias, prompt: Text("ssh alias"))
                    .disabled(running)
                Text("Sesh copies the Vault and its Transcripts to that Host, and each open Task's Worktrees with their uncommitted work. "
                     + "The Agents on \(vault.machine.title) stop and come back ended on the new Host, ready to resume. "
                     + "\(vault.machine.title) keeps its files, and other devices follow the Vault to its new Host.")
                    .font(.caption).foregroundStyle(.secondary)
                if let step {
                    HStack {
                        ProgressView().controlSize(.small)
                        Text(step).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .padding()
        .frame(width: 440)
        .interactiveDismissDisabled(running)
        .toolbar {
            if outcome != nil {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            } else {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(running) }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Move") { Task { await move() } }.disabled(trimmed.isEmpty || running)
                }
            }
        }
    }

    private func move() async {
        step = "Starting"
        let done = await VaultMove(vault, to: .named(trimmed)) { step = $0 }.run(beside: library.vaults.map(\.place))
        if let place = done.place { library.repoint(vault, to: place) }
        outcome = done
    }
}
