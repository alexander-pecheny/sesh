import Foundation

/// This device's copy of every Transcript its Vault's Host changed in the last week, kept so a
/// Host that lost its disk can be given them back (ADR 0016).
@MainActor
final class Mirror {
    private static let days = 7
    private static let chunk = 4 << 20

    private weak var vault: Vault?
    private let folder: URL

    init(vault: Vault, folder: URL) {
        self.vault = vault
        self.folder = folder
    }

    /// Fetches what the Host's recent Transcripts gained since the last round, then drops copies
    /// that stopped growing a week ago.
    func update() async {
        guard let vault, vault.online, vault.place.alias != nil else { return }
        let listed = await vault.machine.run(
            "cd \(vault.folder)/transcripts 2>/dev/null && find . -type f -mtime -\(Self.days) -exec wc -c {} +")
        if listed.ok {
            for (name, size) in Self.sizes(listed.out) { await fetch(name, size: size, from: vault) }
        }
        prune()
    }

    /// Appends every copy kept here to the Host's shorter one, for a Host that lost them.
    func giveBack() async {
        guard let vault else { return }
        let helper = "\(Helper.path) vault"
        for (name, size) in kept() {
            let parts = name.split(separator: "/").map(String.init)
            guard parts.count == 2 else { continue }
            struct Size: Decodable { let size: Int }
            let have = await vault.machine.run("\(helper) size \(vault.folder) \(quote(parts[0])) \(quote(parts[1]))")
            guard var offset = try? JSONDecoder().decode(Size.self, from: Data(have.out.utf8)).size,
                  let handle = try? FileHandle(forReadingFrom: folder.appending(path: name)) else { continue }
            defer { try? handle.close() }
            while offset < size {
                try? handle.seek(toOffset: UInt64(offset))
                guard let bytes = try? handle.read(upToCount: Self.chunk), !bytes.isEmpty else { break }
                let staged = "\(vault.folder)/inbox/\(UUID().uuidString.lowercased()).bytes"
                guard (await vault.machine.put(bytes, to: staged)).ok else { return }
                let ran = await vault.machine.run(
                    "\(helper) append \(vault.folder) \(quote(parts[0])) \(quote(parts[1])) --offset \(offset) \(staged); rm -f \(staged)")
                guard ran.ok else { break }
                offset += bytes.count
            }
        }
    }

    private func fetch(_ name: String, size: Int, from vault: Vault) async {
        let file = folder.appending(path: name)
        var have = kept()[name] ?? 0
        if have > size {
            try? FileManager.default.removeItem(at: file)
            have = 0
        }
        guard have < size else { return }
        try? FileManager.default.createDirectory(at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        if have == 0 { FileManager.default.createFile(atPath: file.path, contents: nil) }
        guard let handle = try? FileHandle(forWritingTo: file) else { return }
        defer { try? handle.close() }
        let path = "\(vault.folder)/transcripts/\(quote(name))"
        while have < size {
            let read = await vault.machine.run("tail -c +\(have + 1) \(path) | head -c \(Self.chunk) | base64 | tr -d '\\n'")
            guard read.ok, let bytes = Data(base64Encoded: read.out), !bytes.isEmpty else { return }
            try? handle.seekToEnd()
            try? handle.write(contentsOf: bytes)
            have += bytes.count
        }
    }

    private func prune() {
        let stale = Date.now.addingTimeInterval(-Double(Self.days) * 86_400)
        for (name, _) in kept() {
            let file = folder.appending(path: name)
            let changed = (try? file.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
            if let changed, changed < stale { try? FileManager.default.removeItem(at: file) }
        }
    }

    /// Every copy here by its `session/file` name, with its length.
    private func kept() -> [String: Int] {
        let sessions = (try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)) ?? []
        var sizes: [String: Int] = [:]
        for session in sessions {
            let files = (try? FileManager.default.contentsOfDirectory(at: session, includingPropertiesForKeys: [.fileSizeKey])) ?? []
            for file in files {
                sizes["\(session.lastPathComponent)/\(file.lastPathComponent)"] =
                    (try? file.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
            }
        }
        return sizes
    }

    /// `wc -c` lines such as "  1234 ./session/file", leaving out its totals.
    static func sizes(_ out: String) -> [(String, Int)] {
        out.split(separator: "\n").compactMap { line in
            let parts = line.trimmingCharacters(in: .whitespaces).split(separator: " ", maxSplits: 1)
            guard parts.count == 2, let size = Int(parts[0]), parts[1].hasPrefix("./") else { return nil }
            let name = String(parts[1].dropFirst(2))
            return name.split(separator: "/").count == 2 ? (name, size) : nil
        }
    }
}
