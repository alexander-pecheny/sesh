import Foundation

/// The coding agents Sesh can start and show. Anything else herdr finds stays hidden.
enum Agent: String, CaseIterable, Identifiable {
    case claude, codex, pi

    var id: String { rawValue }
    var title: String { self == .pi ? "pi" : rawValue.capitalized }

    /// Permissions are always bypassed: she cannot be at the terminal to approve them.
    func flags(_ name: String) -> String {
        switch self {
        case .claude: "--remote-control \(quote(name)) --dangerously-skip-permissions"
        case .codex: "--dangerously-bypass-approvals-and-sandbox"
        case .pi: ""
        }
    }

    var install: String {
        switch self {
        case .claude: "curl -fsSL https://claude.ai/install.sh | bash"
        case .codex: "npm install -g @openai/codex"
        case .pi: "npm install -g @mariozechner/pi-coding-agent"
        }
    }
}

/// Where Sesh's transcript helper lives on every machine (ADR 0006).
enum Helper {
    static let path = "~/.sesh/bin/sesh-transcript"

    /// The published helpers this build trusts: their version, release and SHA-256 by platform.
    struct Published: Decodable {
        let version: String
        let url: String
        let sha256: [String: String]
    }
    static let published = Bundle.main.url(forResource: "helpers", withExtension: "json")
        .flatMap { try? Data(contentsOf: $0) }
        .flatMap { try? JSONDecoder().decode(Published.self, from: $0) }

    /// The build's name for what `uname -sm` printed, as in `linux-x86_64`.
    static func name(_ platform: String) -> String { platform.lowercased().replacingOccurrences(of: " ", with: "-") }

    static let gz = "$HOME/.sesh/bin/sesh-transcript.gz"
    static let unpack = "gunzip -f \(gz) && chmod 755 \(path)"

    /// Fetches the pinned build onto the machine and installs it only if its hash matches.
    static func download(_ name: String) -> String? {
        guard let published, let sha = published.sha256[name] else { return nil }
        let url = quote(published.url + "sesh-transcript-\(name).gz")
        let check = "echo '\(sha)  '\(gz) | { sha256sum -c - || shasum -a 256 -c -; } >/dev/null 2>&1"
        return "mkdir -p ~/.sesh/bin && { curl -fsSL \(url) -o \(gz) || wget -qO \(gz) \(url); } && \(check) && \(unpack)"
    }
}

/// What one shell command left behind.
struct Ran {
    let status: Int32
    let out: String
    let err: String
    var ok: Bool { status == 0 }

    /// herdr prints its errors as JSON on stderr; anything else is shown as it came.
    var problem: String {
        let herdr = try? JSONDecoder().decode(HerdrError.self, from: Data(err.utf8))
        let text = herdr?.error.message ?? err.trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? "the command failed with status \(status)" : text
    }
}

struct HerdrError: Decodable {
    struct Detail: Decodable { let message: String }
    let error: Detail
}

/// A machine Sesh runs shell commands on: a Host over SSH, or the Mac itself.
@MainActor
protocol Runner: AnyObject {
    func run(_ command: String) async -> Ran
    /// Runs `command` until it exits or the calling task is cancelled, handing each line of
    /// its stdout to `line` as it arrives.
    func stream(_ command: String, line: @escaping (String) -> Void) async -> Ran
}

/// `path` quoted for the shell, with a leading `~/` still meaning the home folder.
func shellPath(_ path: String) -> String {
    path.hasPrefix("~/") ? "\"$HOME\"/" + quote(String(path.dropFirst(2))) : quote(path)
}

func quote(_ text: String) -> String { "'" + text.replacingOccurrences(of: "'", with: "'\\''") + "'" }

enum Names {
    /// A name herdr accepts as an agent name and git as a branch.
    static func slug(_ text: String) -> String {
        var slug = ""
        for character in text.lowercased() {
            let keep = character.isASCII && (character.isLetter || character.isNumber || character == "_")
            if keep { slug.append(character) } else if !slug.isEmpty, slug.last != "-" { slug.append("-") }
        }
        while slug.last == "-" { slug.removeLast() }
        if let first = slug.first, !first.isLetter { slug = "c-" + slug }
        return String(slug.prefix(32))
    }
}
