import Foundation

/// A machine a Vault names, as the phone reaches it: a Host from the app's list, through one
/// Link per Host. Records name machines as the Mac does, by ssh alias, so each alias is matched
/// to a Host: the one a Vault was added with, else the Host of that name. The Mac itself, and
/// any alias no Host answers to, cannot be reached from the phone.
@MainActor
final class Machine: Runner, Identifiable, Hashable {
    /// The ssh alias, or nil for the Mac.
    let alias: String?

    nonisolated static func == (a: Machine, b: Machine) -> Bool { a.alias == b.alias }
    nonisolated func hash(into hasher: inout Hasher) { hasher.combine(alias) }

    private init(alias: String?) { self.alias = alias }

    nonisolated var id: String { alias ?? "" }

    static let mac = Machine(alias: nil)
    /// The phone runs nothing itself.
    static let here: [Machine] = []
    private static var known: [String: Machine] = [:]
    private static var bound: [String: UUID] = [:]
    private static var links: [UUID: HostLink] = [:]

    static func named(_ alias: String?) -> Machine {
        guard let alias else { return mac }
        if let machine = known[alias] { return machine }
        let machine = Machine(alias: alias)
        known[alias] = machine
        return machine
    }

    /// The Host a Vault was added with answers to the Vault's alias from then on.
    static func bind(_ alias: String, to host: UUID) { bound[alias] = host }

    var host: Host? {
        guard let alias else { return nil }
        let hosts = Store.shared.hosts
        if let id = Self.bound[alias], let host = hosts.first(where: { $0.id == id }) { return host }
        return hosts.first { $0.name.caseInsensitiveCompare(alias) == .orderedSame }
    }

    var title: String { host?.title ?? alias ?? "the Mac" }

    /// The Link to the Host, made again when the Host was edited since.
    var link: HostLink? {
        guard let host else { return nil }
        if let link = Self.links[host.id], link.host == host { return link }
        Self.links[host.id]?.close()
        let link = HostLink(host: host, store: Store.shared)
        Self.links[host.id] = link
        Asking.shared.links = { Array(Self.links.values) }
        return link
    }

    private var unreachable: Ran {
        let why = alias == nil ? "The phone cannot reach the Mac." : "No Host on this phone answers to \(alias ?? "")."
        return Ran(status: -1, out: "", err: why)
    }

    func run(_ command: String) async -> Ran {
        guard let link else { return unreachable }
        return await link.run(command)
    }

    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran {
        guard let link else { return unreachable }
        return await link.stream(command, line: line)
    }

    func put(_ data: Data, to path: String) async -> Ran {
        guard let link else { return unreachable }
        return await link.put(data, to: path)
    }

    func prepare() async -> String? {
        guard let link else { return unreachable.err }
        return await link.installer.prepare()
    }

    private(set) lazy var follower = FollowerLink(self)
    var uploads: HostLink? { link }
    private var claudeLinks: [String: URL] = [:]

    func claudeLink(for pane: String) async -> URL? {
        if let known = claudeLinks[pane] { return known }
        claudeLinks[pane] = await Herdr.claudeLink(for: pane, on: self)
        return claudeLinks[pane]
    }

    /// Every Link lets go of its socket on the way to the background and dials again on use.
    static func pause() { links.values.forEach { $0.close() } }
    static func resume() { links.values.forEach { $0.resume() } }
}
