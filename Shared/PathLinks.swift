import Foundation
import cmark_gfm
import cmark_gfm_extensions

/// Turns code spans that name a file, such as `docs/plan.md`, into links Sesh can open in a
/// Tab. The Markdown is parsed and rewritten with cmark, so code blocks and links stay intact.
enum PathLinks {
    static let scheme = "sesh-path"

    static func link(_ markdown: String) -> String {
        guard markdown.contains("`"), let root = Cmark.parse(markdown) else { return markdown }
        defer { cmark_node_free(root) }

        var spans: [UnsafeMutablePointer<cmark_node>] = []
        let walk = cmark_iter_new(root)
        while cmark_iter_next(walk) != CMARK_EVENT_DONE {
            guard let node = cmark_iter_get_node(walk), cmark_node_get_type(node) == CMARK_NODE_CODE,
                  cmark_node_get_type(cmark_node_parent(node)) != CMARK_NODE_LINK,
                  let literal = cmark_node_get_literal(node), isPath(String(cString: literal))
            else { continue }
            spans.append(node)
        }
        cmark_iter_free(walk)
        guard !spans.isEmpty else { return markdown }
        for span in spans {
            let path = String(cString: cmark_node_get_literal(span))
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
