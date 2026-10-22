import Foundation

/// This device's copy of the newest Transcripts its Vault's Host changed in the last week, up to
/// a size, kept so a Host that lost its disk can be given them back (ADR 0016).
@MainActor
final class Mirror {
    private static let days = 7
    private static let chunk = 4 << 20
    private static let cap = 500 << 20

    private weak var vault: Vault?
    private let folder: URL

    init(vault: Vault, folder: URL) {
        self.vault = vault
        self.folder = folder
    }

    /// Fetches what the Host's newest Transcripts gained since the last round, then drops copies
    /// that stopped growing a week ago or no longer fit.
    func update() async {
        guard let vault, vault.online, vault.place.alias != nil else { return }
        let find = "find . -type f -mtime -\(Self.days) -exec stat"
        let listed = await vault.machine.run(
            "cd \(vault.folder)/transcripts 2>/dev/null && { \(find) -c '%Y %s %n' {} + 2>/dev/null || \(find) -f '%m %z %N' {} +; }")
        if listed.ok {
            for file in Self.fitting(Self.files(listed.out)) {
                await fetch(file, from: vault)
                try? FileManager.default.setAttributes([.modificationDate: file.changed], ofItemAtPath: folder.appending(path: file.name).path)
            }
        }
        prune()
    }

    /// Appends every copy kept here to the Host's shorter one, for a Host that lost them.
    func giveBack() async {
        guard let vault else { return }
        let helper = "\(Helper.path) vault"
        for file in kept() {
            let parts = file.name.split(separator: "/").map(String.init)
            guard parts.count == 2 else { continue }
            struct Size: Decodable { let size: Int }
            let have = await vault.machine.run("\(helper) size \(vault.folder) \(quote(parts[0])) \(quote(parts[1]))")
            guard var offset = try? JSONDecoder().decode(Size.self, from: Data(have.out.utf8)).size,
                  let handle = try? FileHandle(forReadingFrom: folder.appending(path: file.name)) else { continue }
            defer { try? handle.close() }
            while offset < file.size {
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

    private func fetch(_ remote: File, from vault: Vault) async {
        let (name, size) = (remote.name, remote.size)
        let file = folder.appending(path: name)
        var have = (try? file.resourceValues(forKeys: [.fileSizeKey]))?.fileSize ?? 0
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

    /// Judged by the copies alone, which carry the Host's times, so an emptied Host deletes none.
    private func prune() {
        let stale = Date.now.addingTimeInterval(-Double(Self.days) * 86_400)
        let fresh = kept().filter { $0.changed >= stale }
        let keep = Set(Self.fitting(fresh).map(\.name))
        for file in kept() where !keep.contains(file.name) {
            try? FileManager.default.removeItem(at: folder.appending(path: file.name))
        }
    }

    struct File {
        let name: String
        let size: Int
        let changed: Date
    }

    /// The newest files whose sizes add up to no more than the cap; one too big is passed over.
    static func fitting(_ files: [File]) -> [File] {
        var left = cap
        return files.sorted { $0.changed > $1.changed }.filter { file in
            guard file.size <= left else { return false }
            left -= file.size
            return true
        }
    }

    /// Every copy here by its `session/file` name.
    private func kept() -> [File] {
        let keys: Set<URLResourceKey> = [.fileSizeKey, .contentModificationDateKey]
        let sessions = (try? FileManager.default.contentsOfDirectory(at: folder, includingPropertiesForKeys: nil)) ?? []
        return sessions.flatMap { session in
            let files = (try? FileManager.default.contentsOfDirectory(at: session, includingPropertiesForKeys: Array(keys))) ?? []
            return files.map { file in
                let values = try? file.resourceValues(forKeys: keys)
                return File(name: "\(session.lastPathComponent)/\(file.lastPathComponent)",
                            size: values?.fileSize ?? 0, changed: values?.contentModificationDate ?? .distantPast)
            }
        }
    }

    /// `stat` lines such as "1792686701 1234 ./session/file".
    static func files(_ out: String) -> [File] {
        out.split(separator: "\n").compactMap { line in
            let parts = line.split(separator: " ", maxSplits: 2)
            guard parts.count == 3, let changed = Double(parts[0]), let size = Int(parts[1]), parts[2].hasPrefix("./") else { return nil }
            let name = String(parts[2].dropFirst(2))
            guard name.split(separator: "/").count == 2 else { return nil }
            return File(name: name, size: size, changed: Date(timeIntervalSince1970: changed))
        }
    }
}
