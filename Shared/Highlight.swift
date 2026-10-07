#if os(macOS) && canImport(Highlightr)
import AppKit
import Highlightr

/// Code coloured by highlight.js, in the app's monospaced font, for Documents and code blocks.
@MainActor
enum Highlight {
    private static let highlighter = Highlightr()
    private static var theme = ""

    static func code(_ text: String, language: String?, dark: Bool, size: CGFloat) -> NSAttributedString? {
        guard let highlighter, let language = language.flatMap(name) else { return nil }
        let wanted = dark ? "atom-one-dark" : "atom-one-light"
        if theme != wanted {
            highlighter.setTheme(to: wanted)
            theme = wanted
        }
        highlighter.theme.setCodeFont(.monospacedSystemFont(ofSize: size, weight: .regular))
        guard let coloured = highlighter.highlight(text, as: language, fastRender: true) else { return nil }
        let out = NSMutableAttributedString(attributedString: coloured)
        // The app's own background shows through; highlight.js paints its theme's.
        out.removeAttribute(.backgroundColor, range: NSRange(location: 0, length: out.length))
        return out
    }

    /// highlight.js's name for a fence's info string or a file's extension.
    private static func name(_ hint: String) -> String? {
        let key = hint.lowercased().split(separator: " ").first.map(String.init) ?? ""
        let aliases = [
            "py": "python", "rs": "rust", "ts": "typescript", "tsx": "typescript", "js": "javascript", "jsx": "javascript",
            "sh": "bash", "zsh": "bash", "shell": "bash", "yml": "yaml", "md": "markdown", "kt": "kotlin", "rb": "ruby",
            "h": "c", "m": "objectivec", "toml": "ini", "plist": "xml", "html": "xml",
        ]
        let name = aliases[key] ?? key
        return highlighter?.supportedLanguages().contains(name) == true ? name : nil
    }
}
#endif
