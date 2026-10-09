import Foundation

/// One row of a Vault, as the helper stores it (docs/PLAN.md, Vault protocol).
struct Record: Codable, Identifiable, Equatable {
    enum Kind: String, Codable {
        case folder, task, entry, document, session, tab, conflict
    }

    /// Every field any kind uses; each kind fills its own.
    struct Body: Codable, Equatable {
        var title: String?
        var text: String?
        /// The Task an entry, document, session or conflict belongs to.
        var task: String?
        /// The folder a folder or Task sits in; nil at the Vault's top.
        var parent: String?
        var position: Double?
        /// When the text was last edited, in milliseconds since 1970.
        var edited: Int64?
        /// When an Entry happened, in milliseconds since 1970.
        var at: Int64?
        /// The ssh alias of the machine, or nil for the Mac.
        var machine: String?
        var path: String?
        var branch: String?
        var repo: String?
        var pane: String?
        /// The herdr Workspace a Task's Worktree opened as.
        var workspace: String?
        var agent: String?
        var archived: Bool?
        /// Transcript copies of a session, oldest first.
        var transcripts: [String]?
        var of: String?
        var kind: String?
    }

    let id: String
    var kind: Kind
    var body: Body
    var seq: Int = 0
    var deleted = false
}
