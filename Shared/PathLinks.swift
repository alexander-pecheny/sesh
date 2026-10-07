import Foundation
import cmark_gfm
import cmark_gfm_extensions

/// Turns code spans that name a file, such as `docs/plan.md`, into links Sesh can open in a
/// Tab. The Markdown is parsed and rewritten with cmark, so code blocks and links stay intact.
enum PathLinks {
    static let scheme = "sesh-path"

    /// Paths in code spans become file links, and `#213` a link to that pull request when the
    /// session's repository is known.
    static func link(_ markdown: String, repo: Repo? = nil) -> String {
        let numbers = repo != nil && markdown.contains("#")
        guard markdown.contains("`") || numbers, let root = Cmark.parse(markdown) else { return markdown }
        defer { cmark_node_free(root) }
        var changed = false
        if let repo, numbers { changed = linkNumbers(root, repo: repo) }

        // A bare name listed under a folder the message named, as in "Files are in `/a/b/`:",
        // lives in that folder.
        var spans: [(node: UnsafeMutablePointer<cmark_node>, path: String)] = []
        var folder: String?
        let walk = cmark_iter_new(root)
        while cmark_iter_next(walk) != CMARK_EVENT_DONE {
            guard let node = cmark_iter_get_node(walk), cmark_node_get_type(node) == CMARK_NODE_CODE,
                  cmark_node_get_type(cmark_node_parent(node)) != CMARK_NODE_LINK,
                  let literal = cmark_node_get_literal(node) else { continue }
            let text = String(cString: literal)
            if (text.hasPrefix("/") || text.hasPrefix("~/")) && text.hasSuffix("/") && !text.contains(" ") {
                folder = text
                continue
            }
            guard isPath(text) else { continue }
            let rooted = text.hasPrefix("/") || text.hasPrefix("~/")
            spans.append((node, rooted || folder == nil ? text : folder! + text))
        }
        cmark_iter_free(walk)
        guard !spans.isEmpty || changed else { return markdown }
        for (span, path) in spans {
            guard let link = cmark_node_new(CMARK_NODE_LINK),
                  let url = URL(string: "\(scheme):" + (path.addingPercentEncoding(withAllowedCharacters: CharacterSet.urlPathAllowed) ?? path))
            else { continue }
            cmark_node_set_url(link, url.absoluteString)
            cmark_node_insert_before(span, link)
            cmark_node_append_child(link, span)
        }
        guard let rendered = cmark_render_commonmark(root, CMARK_OPT_DEFAULT, 0) else { return markdown }
        defer { free(rendered) }
        return String(cString: rendered)
    }

    /// Splits text nodes around `#123` and puts each number in a link of its own.
    private static func linkNumbers(_ root: UnsafeMutablePointer<cmark_node>, repo: Repo) -> Bool {
        var texts: [UnsafeMutablePointer<cmark_node>] = []
        let walk = cmark_iter_new(root)
        while cmark_iter_next(walk) != CMARK_EVENT_DONE {
            guard let node = cmark_iter_get_node(walk), cmark_node_get_type(node) == CMARK_NODE_TEXT,
                  cmark_node_get_type(cmark_node_parent(node)) != CMARK_NODE_LINK else { continue }
            texts.append(node)
        }
        cmark_iter_free(walk)
        var changed = false
        for node in texts {
            let text = String(cString: cmark_node_get_literal(node))
            // `#213` alone, not `a#213`, `&#213;` or a URL's fragment.
            let matches = text.matches(of: /#(\d{1,7})\b/).filter { match in
                guard match.range.lowerBound > text.startIndex else { return true }
                let before = text[text.index(before: match.range.lowerBound)]
                return !(before.isLetter || before.isNumber || before == "&" || before == "/" || before == "_")
            }
            guard !matches.isEmpty else { continue }
            var rest = text[...]
            for match in matches {
                insertText(String(rest[rest.startIndex..<match.range.lowerBound]), before: node)
                if let link = cmark_node_new(CMARK_NODE_LINK), let label = cmark_node_new(CMARK_NODE_TEXT) {
                    cmark_node_set_url(link, repo.pull(String(match.output.1)).absoluteString)
                    cmark_node_set_literal(label, String(text[match.range]))
                    cmark_node_append_child(link, label)
                    cmark_node_insert_before(node, link)
                }
                rest = text[match.range.upperBound...]
            }
            cmark_node_set_literal(node, String(rest))
            changed = true
        }
        return changed
    }

    private static func insertText(_ text: String, before node: UnsafeMutablePointer<cmark_node>) {
        guard !text.isEmpty, let piece = cmark_node_new(CMARK_NODE_TEXT) else { return }
        cmark_node_set_literal(piece, text)
        cmark_node_insert_before(node, piece)
    }

    /// A file name with an extension, alone or behind folders, and no spaces.
    static func isPath(_ text: String) -> Bool {
        guard !text.contains(where: \.isWhitespace), text.count < 300,
              let name = text.split(separator: "/").last, let dot = name.lastIndex(of: "."), dot != name.startIndex
        else { return false }
        let ext = name[name.index(after: dot)...].lowercased()
        guard !ext.isEmpty, ext.allSatisfy({ $0.isLetter || $0.isNumber }), !text.contains("://") else { return false }
        // `items.count` is code, not a file; a folder or a known extension says it is one.
        return text.contains("/") || extensions.contains(ext)
    }

    private static let extensions: Set<String> = [
        "md", "markdown", "txt", "swift", "rs", "py", "ts", "tsx", "js", "jsx", "go", "rb", "java", "kt", "c", "h",
        "cpp", "m", "json", "yml", "yaml", "toml", "sh", "zsh", "html", "css", "sql", "lock", "plist", "xml",
    ]

    static func path(from url: URL) -> String? {
        guard url.scheme == scheme else { return nil }
        return String(url.absoluteString.dropFirst(scheme.count + 1)).removingPercentEncoding
    }
}

/// GitHub-flavoured Markdown as cmark's tree; the caller frees the root.
enum Cmark {
    static func hasTable(_ markdown: String) -> Bool {
        guard markdown.contains("|"), let root = parse(markdown) else { return false }
        defer { cmark_node_free(root) }
        var child = cmark_node_first_child(root)
        while let node = child {
            if String(cString: cmark_node_get_type_string(node)) == "table" { return true }
            child = cmark_node_next(node)
        }
        return false
    }

    static func parse(_ markdown: String) -> UnsafeMutablePointer<cmark_node>? {
        cmark_gfm_core_extensions_ensure_registered()
        guard let parser = cmark_parser_new(CMARK_OPT_DEFAULT) else { return nil }
        defer { cmark_parser_free(parser) }
        for name in ["table", "strikethrough", "autolink", "tasklist"] {
            if let syntax = cmark_find_syntax_extension(name) { cmark_parser_attach_syntax_extension(parser, syntax) }
        }
        cmark_parser_feed(parser, markdown, markdown.utf8.count)
        return cmark_parser_finish(parser)
    }
}

/// Where a session's repository lives on the web, read from its `origin`.
struct Repo: Equatable {
    let web: URL

    /// `git@host:owner/repo.git`, `ssh://git@host:22/owner/repo` or an https URL.
    init?(remote: String) {
        var text = remote.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasSuffix(".git") { text.removeLast(4) }
        if let scp = text.wholeMatch(of: /[\w.-]+@([\w.-]+):(?!\/\/)(.+)/) {
            text = "https://\(scp.output.1)/\(scp.output.2)"
        } else if let ssh = text.wholeMatch(of: /ssh:\/\/(?:[\w.-]+@)?([\w.-]+)(?::\d+)?\/(.+)/) {
            text = "https://\(ssh.output.1)/\(ssh.output.2)"
        }
        guard var parts = URLComponents(string: text), parts.scheme?.hasPrefix("http") == true, parts.host != nil else { return nil }
        parts.user = nil
        parts.password = nil
        guard let web = parts.url else { return nil }
        self.web = web
    }

    /// GitHub's `/pull/N`, GitLab's merge requests, and Forgejo's and Gitea's `/pulls/N`.
    func pull(_ number: String) -> URL {
        let host = web.host() ?? ""
        let path = host == "github.com" ? "pull/\(number)" : host.contains("gitlab") ? "-/merge_requests/\(number)" : "pulls/\(number)"
        return web.appending(path: path)
    }
}
