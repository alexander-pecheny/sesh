import SwiftUI

/// The Hosts Vaults and Agents run on, to add, edit and delete; any of them also opens a
/// plain shell, outside every Task.
struct HostsView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.colorScheme) private var colorScheme
    @State private var editing: Host?
    @State private var shell: Host?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        List {
            ForEach(store.hosts) { host in
                Button { editing = host } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(host.title).font(.ui(16)).foregroundStyle(flavour(.text))
                        Text(host.subtitle).font(.ui(12)).foregroundStyle(flavour(.subtext0))
                    }
                }
                .contextMenu {
                    Button("Edit") { editing = host }
                    Button("Open a shell") { shell = host }
                }
                .swipeActions {
                    Button("Delete", role: .destructive) { store.remove(host) }
                    Button("Shell") { shell = host }.tint(flavour(.blue))
                }
            }
            if store.hosts.isEmpty {
                Text("No Hosts yet").font(.ui(15)).foregroundStyle(flavour(.subtext0))
            }
        }
        .navigationTitle("Hosts")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { editing = Host() } label: { Label("Add Host", image: "plus") }
            }
        }
        .sheet(item: $editing) { HostFormView(host: $0) }
        .fullScreenCover(item: $shell) { ShellScreen(host: $0) }
    }
}

/// A login shell on a Host, as Sesh's terminal always was; it ends when the screen closes.
private struct ShellScreen: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var ghostty: Ghostty.App
    @EnvironmentObject private var store: Store
    let host: Host
    @State private var session: SeshSession?

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(session.map(\.name) ?? host.title).font(.ui(14)).foregroundStyle(flavour(.text)).lineLimit(1)
                Spacer()
                Button {
                    session?.close()
                    dismiss()
                } label: { Image.lucide("circle-x").foregroundStyle(flavour(.overlay1)) }
                    .accessibilityLabel("Close")
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(flavour(.mantle))
            if let session { SessionTab(session: session) }
            Spacer(minLength: 0)
        }
        .background(flavour(.base))
        .onAppear {
            guard session == nil, let app = ghostty.app else { return }
            session = SeshSession(host: host, store: store, app: app)
        }
    }
}
