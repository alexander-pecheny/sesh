import Foundation

/// Folders and Tasks: which sit where, and moving them between folders.
enum Tree {
    @MainActor
    static func children(of parent: String?, in vault: Vault) -> [Record] {
        (vault.all(.folder) + vault.all(.task).filter { $0.body.archived != true })
            .filter { $0.body.parent == parent }
            .sorted { ($0.body.position ?? 0, $0.id) < ($1.body.position ?? 0, $1.id) }
    }

    @MainActor
    static func next(in parent: String?, of vault: Vault) -> Double {
        (children(of: parent, in: vault).compactMap(\.body.position).max() ?? 0) + 1
    }

    /// Drops records into a folder, refusing to put a folder inside itself.
    @MainActor
    static func move(_ ids: [String], into parent: String?, in vault: Vault) -> Bool {
        var moved = false
        for id in ids {
            guard var record = vault.records[id], record.kind == .folder || record.kind == .task,
                  record.body.parent != parent, !contains(id, parent, in: vault) else { continue }
            record.body.parent = parent
            record.body.position = next(in: parent, of: vault)
            vault.write(record)
            moved = true
        }
        return moved
    }

    @MainActor
    private static func contains(_ folder: String, _ target: String?, in vault: Vault) -> Bool {
        var current = target
        while let id = current {
            if id == folder { return true }
            current = vault.records[id]?.body.parent
        }
        return false
    }

    /// Whether an archived Task still sits in the folder, which `children` leaves out.
    @MainActor
    static func holdsArchived(_ folder: String, in vault: Vault) -> Bool {
        vault.all(.task).contains { $0.body.parent == folder && $0.body.archived == true }
    }

    @MainActor
    static func deleteFolder(_ folder: Record, in vault: Vault) {
        guard children(of: folder.id, in: vault).isEmpty else { return }
        vault.delete(folder)
    }
}
