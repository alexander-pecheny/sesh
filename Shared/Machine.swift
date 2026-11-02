import Foundation

/// A machine a Vault names: a Host, reached through the core's one Link per Host, or the Mac
/// itself. The Mac names a Host by its alias in the owner's `~/.ssh/config`. The phone matches
/// each alias to a Host from its own list: the one a Vault was added with, else the Host of
/// that name; it cannot reach the Mac, nor an alias no Host answers to.
@MainActor
final class Machine: Runner, Identifiable, Hashable {
    /// The ssh alias, or nil for the Mac.
    let alias: String?

    nonisolated static func == (a: Machine, b: Machine) -> Bool { a.alias == b.alias }
    nonisolated func hash(into hasher: inout Hasher) { hasher.combine(alias) }

    private init(alias: String?) { self.alias = alias }

    nonisolated var id: String { alias ?? "" }
    /// Streams run over mosh pipes wherever Sesh carries a mosh-server to put on the Host.
    var carrier: String? { alias == nil ? nil : Helper.moshServer == nil ? "ssh" : "mosh" }

    static let mac = Machine(alias: nil)
    /// One object per Host for the app's life: Conversations hold their machine weakly.
    private static var known: [String: Machine] = [:]

    static func named(_ alias: String?) -> Machine {
        guard let alias else { return mac }
        if let machine = known[alias] { return machine }
        let machine = Machine(alias: alias)
        known[alias] = machine
        return machine
    }

    func run(_ command: String) async -> Ran {
        guard let link else { return await here(command, line: nil) }
        return await link.run(command)
    }

    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran {
        await stream(command, opened: nil, line: line)
    }

    func fetch(_ command: String, line: @escaping (String) -> Void) async -> Ran {
        guard let link else { return await here(command, line: line) }
        return await link.stream(command, line: line)
    }

    func stream(_ command: String, opened: @escaping (@escaping (String) -> Void) -> Void, line: @escaping (String) -> Void) async -> Ran {
        await stream(command, opened: Optional(opened), line: line)
    }

    /// A Host's stream outlives sleep and roaming over its own mosh pipe, which the Link only starts.
    private func stream(_ command: String, opened: ((@escaping (String) -> Void) -> Void)?, line: @escaping (String) -> Void) async -> Ran {
        guard let link else { return await here(command, line: line, opened: opened) }
        guard let server = Helper.moshServer else { return await link.stream(command, line: line) }
        return await link.pipe(command, server: server, opened: opened, line: line)
    }

    func put(_ data: Data, to path: String) async -> Ran {
        guard let link else { return await here("mkdir -p \"$(dirname \(shellPath(path)))\" && cat > \(shellPath(path))", line: nil, input: data) }
        return await link.put(data, to: path)
    }

    private var home: String?
    private(set) lazy var follower = FollowerLink(self)

    /// Pasted images land in `~/.sesh/uploads` under the moment they were pasted, as the
    /// phone's Uploads do, so the Agent can be told where to look.
    func upload(_ data: Data, ext: String) async -> String? {
        if home == nil {
            let ran = await run("printf %s \"$HOME\"")
            if ran.ok, !ran.out.isEmpty { home = ran.out }
        }
        guard let home else { return nil }
        let stamp = Date().formatted(.iso8601.year().month().day().time(includingFractionalSeconds: false).timeSeparator(.omitted))
        let path = "\(home)/.sesh/uploads/pasted-\(stamp).\(ext)"
        return (await put(data, to: path)).ok ? path : nil
    }

    #if os(macOS)
    var title: String { alias ?? "This Mac" }
    var talks: Bool { true }
    /// The machines on this device itself, which serve every Vault.
    static let here = [mac]

    /// The connection to the Host; the Mac itself has none.
    private(set) lazy var link: CoreLink? = alias.map { alias in
        Asking.shared.links = { Self.known.values.compactMap(\.link) }
        return CoreLink(title: alias) { callbacks, userdata in sesh_link_connect_alias(alias, callbacks, userdata) }
    }

    private let local = Local()
    private lazy var installer = HelperInstaller(
        title: title, run: { [unowned self] in await run($0) }, put: { [unowned self] in await put($0, to: $1) })

    func prepare() async -> String? { await installer.prepare() }

    private func here(_ command: String, line: ((String) -> Void)?, input: Data? = nil,
                      opened: ((@escaping (String) -> Void) -> Void)? = nil) async -> Ran {
        await local.run(command, line: line, input: input, opened: opened)
    }

    /// A normal quit stops every command at once instead of leaving it to notice.
    static func stopAll() {
        mac.local.stop()
        known.values.forEach { $0.link?.close() }
    }

    /// Lets go of every Host's connection, as after sleep, when its socket may be dead; the next
    /// command dials again, and mosh pipes carry on by themselves.
    static func redial() {
        known.values.forEach { $0.link?.close(); $0.link?.resume() }
    }
    #else
    var title: String { host?.title ?? alias ?? "the Mac" }
    var talks: Bool { Helper.moshServer != nil }
    /// The phone runs nothing itself.
    static let here: [Machine] = []
    private static var bound: [String: UUID] = [:]
    private static var links: [UUID: HostLink] = [:]

    /// The Host a Vault was added with answers to the Vault's alias from then on.
    static func bind(_ alias: String, to host: UUID) { bound[alias] = host }

    /// The machine Simple mode reaches a Host as, since it has no Vault alias to go by.
    static func of(_ host: Host) -> Machine {
        let alias = "host-" + host.id.uuidString
        bind(alias, to: host.id)
        return named(alias)
    }

    var host: Host? {
        guard let alias else { return nil }
        let hosts = Store.shared.hosts
        if let id = Self.bound[alias], let host = hosts.first(where: { $0.id == id }) { return host }
        return hosts.first { $0.name.caseInsensitiveCompare(alias) == .orderedSame }
    }

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

    var uploads: HostLink? { link }

    func prepare() async -> String? {
        guard let link else { return unreachable.err }
        return await link.installer.prepare()
    }

    private var unreachable: Ran {
        let why = alias == nil ? "The phone cannot reach the Mac." : "No Host on this phone answers to \(alias ?? "")."
        return Ran(status: -1, out: "", err: why)
    }

    private func here(_ command: String, line: ((String) -> Void)?, input: Data? = nil,
                      opened: ((@escaping (String) -> Void) -> Void)? = nil) async -> Ran {
        unreachable
    }

    private var claudeLinks: [String: URL] = [:]

    func claudeLink(for pane: String) async -> URL? {
        if let known = claudeLinks[pane] { return known }
        claudeLinks[pane] = await Mux.claudeLink(for: pane, on: self)
        return claudeLinks[pane]
    }

    /// Every Link lets go of its socket on the way to the background and dials again on use.
    static func pause() { links.values.forEach { $0.close() } }
    static func resume() { links.values.forEach { $0.resume() } }
    #endif
}
