import SwiftUI

/// A rounded row that opens to show more. Every tool, question and permission is one.
private struct Card<Header: View, Detail: View>: View {
    @Environment(\.flavour) private var flavour
    let icon: String
    var tint = Catppuccin.Swatch.overlay1
    var opens = true
    /// A button beside the header, outside the one that opens the card.
    var side: (label: String, icon: String, run: () -> Void)?
    @Binding var open: Bool
    @ViewBuilder let header: () -> Header
    @ViewBuilder let detail: () -> Detail
    var opened: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.pad) {
            HStack(alignment: .top, spacing: Metric.pad) {
                Button {
                    guard opens else { return }
                    open.toggle()
                    if open { opened() }
                } label: {
                    HStack(alignment: .top, spacing: Metric.pad) {
                        Image.lucide(icon, size: Metric.body).foregroundStyle(flavour(tint)).padding(.top, 1)
                        header().frame(maxWidth: .infinity, alignment: .leading)
                        if opens {
                            Image.lucide(open ? "chevron-down" : "chevron-right", size: Metric.note).foregroundStyle(flavour(.overlay1))
                        }
                    }
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                if let side {
                    Button(action: side.run) {
                        Image.lucide(side.icon, size: Metric.label).foregroundStyle(flavour(.overlay1))
                            .padding(Metric.gap)
                            .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .padding(-Metric.gap)
                    .accessibilityLabel(side.label)
                    .help(side.label)
                }
            }
            if open { detail().frame(maxWidth: .infinity, alignment: .leading) }
        }
        .padding(Metric.pad)
        .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
    }
}

struct ToolCard: View {
    @Environment(\.flavour) private var flavour
    let entry: SessionLog.Entry
    let result: SessionLog.Entry?
    let conversation: Conversation

    private var failed: Bool { result?.error == true }
    private var open: Binding<Bool> { conversation.isOpen(entry.id) }

    var body: some View {
        switch entry.tool {
        case "edit", "write":
            Card(icon: entry.tool == "edit" ? "file-pen" : "file-plus", tint: failed ? .red : .blue,
                 opens: result?.diff != nil || result?.text != nil, side: openSide, open: open) {
                HStack(spacing: Metric.gap) {
                    Text(entry.file.map { ($0 as NSString).lastPathComponent } ?? entry.summary)
                        .font(.ui(Metric.label).weight(.medium)).foregroundStyle(flavour(.text)).lineLimit(1)
                    Spacer(minLength: Metric.tiny)
                    if let added = result?.added, added > 0 { Text("+\(added)").foregroundStyle(flavour(.green)) }
                    if let removed = result?.removed, removed > 0 { Text("\u{2212}\(removed)").foregroundStyle(flavour(.red)) }
                }
                .font(.system(size: Metric.caption, design: .monospaced))
            } detail: {
                if let diff = result?.diff { Diff(text: diff, truncated: result?.truncated == true) } else { output }
            } opened: { expand() }
        case "bash":
            Card(icon: "terminal", tint: failed ? .red : .green, opens: result != nil, open: open) {
                VStack(alignment: .leading, spacing: Metric.tiny) {
                    if let description = entry.description {
                        Text(description).font(.ui(Metric.note)).foregroundStyle(flavour(.subtext0))
                    }
                    Text(entry.command ?? entry.summary)
                        .font(.system(size: Metric.caption, design: .monospaced)).foregroundStyle(flavour(.text)).lineLimit(3)
                }
            } detail: {
                VStack(alignment: .leading, spacing: Metric.gap) {
                    // The header cuts a long command after three lines.
                    if let command = entry.command, command.split(separator: "\n").count > 3 || command.count > 240 {
                        Text(command).font(.system(size: Metric.caption, design: .monospaced)).foregroundStyle(flavour(.text))
                            .textSelection(.enabled)
                    }
                    output
                }
            } opened: { expand() }
        case "task":
            Card(icon: "bot", tint: .mauve, opens: result != nil, side: subagentSide, open: open) {
                VStack(alignment: .leading, spacing: Metric.tiny) {
                    Text("Task").font(.ui(Metric.caption)).foregroundStyle(flavour(.subtext0))
                    Text(entry.description ?? entry.summary).font(.ui(Metric.label)).foregroundStyle(flavour(.text))
                }
            } detail: {
                Markdown(text: result?.text ?? "")
            } opened: { expand() }
        default:
            Card(icon: "wrench", tint: failed ? .red : .overlay1, opens: result?.text != nil, open: open) {
                Text(entry.summary).font(.ui(Metric.label)).foregroundStyle(flavour(.text))
            } detail: { output } opened: { expand() }
        }
    }

    /// Claude's subagents keep their own Transcripts, which open in a Tab of their own.
    private var subagentSide: (label: String, icon: String, run: () -> Void)? {
        guard let open = conversation.openSubagent, let range = entry.id.range(of: ".call.") else { return nil }
        let call = String(entry.id[range.upperBound...])
        let title = entry.description ?? entry.summary
        return ("Open the subagent", "external-link", { open(call, title) })
    }

    private var openSide: (label: String, icon: String, run: () -> Void)? {
        guard let open = conversation.openPath, let file = entry.file else { return nil }
        return ("Open \((file as NSString).lastPathComponent)", "external-link", { open(file) })
    }

    private var output: some View {
        Output(text: result?.text ?? "", failed: failed, truncated: result?.truncated == true)
    }

    private func expand() {
        guard let result else { return }
        Task { await conversation.expand(result) }
    }
}

private struct Output: View {
    @Environment(\.flavour) private var flavour
    let text: String
    let failed: Bool
    let truncated: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(text.isEmpty ? "No output" : PathLinks.link(plain: text.trimmingCharacters(in: .newlines)))
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(flavour(failed ? .red : .subtext1))
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            if truncated { Text("Some lines in the middle are not shown").font(.ui(11)).foregroundStyle(flavour(.overlay1)) }
        }
        .padding(10)
        .background(flavour(.base), in: .rect(cornerRadius: 6))
    }
}

private struct Diff: View {
    @Environment(\.flavour) private var flavour
    let text: String
    let truncated: Bool

    private var lines: [String] {
        let lines = text.trimmingCharacters(in: .newlines).components(separatedBy: "\n")
        return Array(lines.drop { !$0.hasPrefix("@@") })
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                Text(line.isEmpty ? " " : line)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(flavour(line.hasPrefix("@@") ? .overlay1 : .text))
                    .padding(.horizontal, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(background(line))
            }
            if truncated { Text("Some lines in the middle are not shown").font(.ui(11)).foregroundStyle(flavour(.overlay1)).padding(8) }
        }
        .padding(.vertical, 6)
        .background(flavour(.base), in: .rect(cornerRadius: 6))
        .clipShape(.rect(cornerRadius: 6))
    }

    private func background(_ line: String) -> Color {
        switch line.first {
        case "+": flavour(.green).opacity(0.18)
        case "-": flavour(.red).opacity(0.18)
        default: .clear
        }
    }
}

struct Lookups: View {
    @Environment(\.flavour) private var flavour
    let entries: [SessionLog.Entry]
    @Binding var open: Bool
    let openPath: ((String) -> Void)?

    private var line: String {
        let count = { (tool: String) in entries.filter { $0.tool == tool }.count }
        let parts = [
            (count("read"), "Read %d file", "Read %d files"),
            (count("search"), "searched once", "searched %d times"),
            (count("fetch"), "fetched a page", "fetched %d pages"),
        ]
        .filter { $0.0 > 0 }
        .map { String(format: $0.0 == 1 ? $0.1 : $0.2, $0.0) }
        let joined = parts.joined(separator: ", ")
        return joined.prefix(1).uppercased() + joined.dropFirst()
    }

    var body: some View {
        Card(icon: "search", open: $open) {
            Text(line).font(.ui(14)).foregroundStyle(flavour(.text))
        } detail: {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(entries) { entry in
                    let text = Text(entry.file ?? entry.command ?? entry.summary)
                        .font(.system(size: 11, design: .monospaced))
                        .lineLimit(2)
                    if let file = entry.file, let openPath {
                        Button { openPath(file) } label: { text.foregroundStyle(flavour(.blue)).multilineTextAlignment(.leading) }
                            .buttonStyle(.plain)
                            .help("Open \((file as NSString).lastPathComponent)")
                    } else {
                        text.foregroundStyle(flavour(.subtext1))
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
