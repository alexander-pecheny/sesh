import XCTest

/// A machine whose commands run as the app runs them, in a plain `/bin/sh -c`, but in dash, the
/// strictest sh a Host has, with a fresh home folder.
@MainActor
private final class Dash: Runner {
    let title = "dash"
    let home = FileManager.default.temporaryDirectory.appending(path: "sesh-dash-\(UUID().uuidString)")
    private(set) lazy var follower = FollowerLink(self)
    private(set) var commands: [String] = []

    init() { try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true) }

    func run(_ command: String) async -> Ran { run(command, input: nil) }

    func run(_ command: String, input: Pipe?) -> Ran {
        commands.append(command)
        let process = Process()
        process.executableURL = URL(filePath: "/bin/dash")
        process.arguments = ["-c", command]
        process.environment = ["HOME": home.path, "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "GIT_CONFIG_NOSYSTEM": "1"]
        let out = Pipe(), err = Pipe()
        process.standardOutput = out
        process.standardError = err
        process.standardInput = input ?? FileHandle.nullDevice
        try? process.run()
        let output = out.fileHandleForReading.readDataToEndOfFile(), error = err.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return Ran(status: process.terminationStatus, out: String(decoding: output, as: UTF8.self), err: String(decoding: error, as: UTF8.self))
    }

    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran { await run(command) }
    func put(_ data: Data, to path: String) async -> Ran { Ran(status: 1, out: "", err: "") }
    func prepare() async -> String? { nil }

    func exists(_ path: String) -> Bool { FileManager.default.fileExists(atPath: home.appending(path: path).path) }
}

/// Sesh's own snippets run in `/bin/sh` on every machine, whatever the user's login shell is.
@MainActor
final class EmbeddedScriptsTests: XCTestCase {
    private func parses(_ script: String, file: StaticString = #filePath, line: UInt = #line) async {
        let dash = Dash()
        let parsed = await dash.run("sh -n -c \(quote(script)) && dash -n -c \(quote(script))")
        XCTAssertTrue(parsed.ok, "\(parsed.err)\n\(script)", file: file, line: line)
    }

    func testEverySnippetParsesInSh() async {
        let published = Helper.Published(version: "1", url: "https://example.com/", sha256: ["linux-x86_64": "abc"])
        let scripts = [
            Mux.importer, Mux.integrations, Mux.wake(upgrade: true), Mux.wake(upgrade: false), Helper.link, Helper.unpack("f"),
            Helper.download("sesh-transcript", as: "f", from: published, for: "linux-x86_64") ?? "", HelperInstaller.probe, tethered("true"),
        ]
        for script in scripts { await parses(script) }
    }

    func testTheInstallSnippetsRunInAFreshHome() async {
        let dash = Dash()
        _ = await dash.run("mkdir -p ~/.sesh/bin ~/.claude-work/projects && echo hi | gzip > ~/.sesh/bin/f.gz")
        let installed = await dash.run(Mux.importer + "\n" + Mux.integrations + "\n" + Helper.unpack("f") + " && " + Helper.link)
        XCTAssertTrue(installed.ok, installed.err)
        XCTAssertTrue(dash.exists(".sesh/bin/sesh-mux-import"))
        XCTAssertEqual(dash.run("cat ~/.sesh/bin/f", input: nil).out, "hi\n")
        XCTAssertTrue(dash.run("[ -L ~/.local/bin/sesh ]", input: nil).ok)
    }

    func testATetheredCommandEndsWhenItsStdinCloses() async {
        let dash = Dash()
        let stdin = Pipe()
        try? stdin.fileHandleForWriting.close()
        let started = Date()
        XCTAssertNotEqual(dash.run(tethered("sleep 5"), input: stdin).status, 0)
        XCTAssertLessThan(Date().timeIntervalSince(started), 3)
        let open = Pipe()
        XCTAssertEqual(dash.run(tethered("echo soon"), input: open).out, "soon\n")
        try? open.fileHandleForWriting.close()
    }

    func testGitsSnippetsRunInSh() async {
        let dash = Dash()
        let repo = dash.home.appending(path: "repo").path, tree = dash.home.appending(path: "tree").path
        let git = "git -c user.name=t -c user.email=t@t"
        _ = await dash.run("\(git) init -q \(quote(repo)) && \(git) -C \(quote(repo)) commit -q --allow-empty -m one")
        let added = await Git.addWorktree(tree, branch: "fix", from: nil, of: repo, on: dash)
        XCTAssertTrue(added.ok, added.err)
        let linked = await Git.linked(tree, on: dash)
        let isWorktree = await Git.isWorktree(tree, of: repo, on: dash)
        let clean = await Git.clean(tree, on: dash)
        XCTAssertTrue(linked && isWorktree && clean)
        let branches = await Git.branches(of: repo, on: dash)
        XCTAssertTrue(branches.contains("fix"))
        let survey = await Git.survey([tree, repo], on: dash)
        XCTAssertEqual(survey, [tree: ""])
        await Git.update(repo, on: dash)
        await Git.watch(repo, on: dash)
        let removed = await Git.removeWorktree(tree, of: repo, force: false, on: dash)
        XCTAssertTrue(removed.ok, removed.err)
        for command in dash.commands { await parses(command) }
    }
}
