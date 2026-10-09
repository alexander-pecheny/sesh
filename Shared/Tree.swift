import Foundation

/// Folders and Tasks: which sit where, moving them between folders, closing and reopening.
@MainActor
enum Tree {
    static func children(of parent: String?, in vault: some Records) -> [Record] {
        (vault.all(.folder) + open(in: vault))
            .filter { $0.body.parent == parent }
            .sorted { ($0.body.position ?? 0, $0.id) < ($1.body.position ?? 0, $1.id) }
    }

    static func next(in parent: String?, of vault: some Records) -> Double {
        (children(of: parent, in: vault).compactMap(\.body.position).max() ?? 0) + 1
    }

    /// Every Task not closed, by title, as the menus that pick one list them.
    static func open(in vault: some Records) -> [Record] {
        byTitle(vault.all(.task).filter { $0.body.archived != true })
    }

    static func archived(in vault: some Records) -> [Record] {
        byTitle(vault.all(.task).filter { $0.body.archived == true })
    }

    /// The Tasks an Agent session or Document can move to: every open one but its own.
    static func moveTargets(for record: Record, in vault: some Records) -> [Record] {
        open(in: vault).filter { $0.id != record.body.task }
    }

    private static func byTitle(_ records: [Record]) -> [Record] {
        records.sorted { ($0.body.title ?? "", $0.id) < ($1.body.title ?? "", $1.id) }
    }

    /// A Task from its title alone, last in its folder.
    static func newTask(_ title: String, parent: String?, in vault: some Records) -> Record {
        vault.create(.task, .init(
            title: title, parent: parent, position: next(in: parent, of: vault), edited: Int64(Date().timeIntervalSince1970 * 1000)))
    }

    /// Opens a closed Task again. Its folder may have gone while it was closed; it comes back at the top.
    static func reopen(_ task: Record, in vault: some Records) {
        var record = vault.records[task.id] ?? task
        record.body.archived = false
        if let parent = record.body.parent, vault.records[parent]?.deleted != false { record.body.parent = nil }
        vault.write(record)
    }

    /// Drops records into a folder, refusing to put a folder inside itself.
    static func move(_ ids: [String], into parent: String?, in vault: some Records) -> Bool {
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

    private static func contains(_ folder: String, _ target: String?, in vault: some Records) -> Bool {
        var current = target
        while let id = current {
            if id == folder { return true }
            current = vault.records[id]?.body.parent
        }
        return false
    }

    /// Only an empty folder goes, closed Tasks included, or they would be left in none.
    static func canDelete(_ folder: Record, in vault: some Records) -> Bool {
        children(of: folder.id, in: vault).isEmpty && !vault.all(.task).contains { $0.body.parent == folder.id }
    }

    static func deleteFolder(_ folder: Record, in vault: some Records) {
        guard canDelete(folder, in: vault) else { return }
        vault.delete(folder)
    }
}
