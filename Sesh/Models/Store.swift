import Foundation

@MainActor
final class Store: ObservableObject {
    @Published var hosts: [Host] = [] { didSet { write(hosts, to: "hosts.json") } }
    @Published var keys: [Key] = [] { didSet { write(keys, to: "keys.json") } }

    private let directory: URL

    init() {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        directory = support.appendingPathComponent("Sesh", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        hosts = read("hosts.json") ?? []
        keys = read("keys.json") ?? []
    }

    var knownHostsPath: String { directory.appendingPathComponent("known_hosts").path }

    func upsert(_ host: Host) {
        if let index = hosts.firstIndex(where: { $0.id == host.id }) { hosts[index] = host } else { hosts.append(host) }
    }

    func remove(_ host: Host) {
        hosts.removeAll { $0.id == host.id }
        Keychain.write(nil, to: "password.\(host.id)")
    }

    func add(_ key: Key, material: String, passphrase: String?) {
        Keychain.write(material, to: "key.\(key.id)")
        Keychain.write(passphrase, to: "passphrase.\(key.id)")
        keys.append(key)
    }

    func remove(_ key: Key) {
        keys.removeAll { $0.id == key.id }
        Keychain.write(nil, to: "key.\(key.id)")
        Keychain.write(nil, to: "passphrase.\(key.id)")
    }

    func key(_ id: UUID?) -> Key? { keys.first { $0.id == id } }

    private func read<T: Decodable>(_ name: String) -> T? {
        guard let data = try? Data(contentsOf: directory.appendingPathComponent(name)) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    private func write<T: Encodable>(_ value: T, to name: String) {
        guard let data = try? JSONEncoder().encode(value) else { return }
        try? data.write(to: directory.appendingPathComponent(name), options: .atomic)
    }
}
