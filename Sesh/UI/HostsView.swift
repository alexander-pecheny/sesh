import SwiftUI

/// The Hosts Vaults and Agents run on, to add, edit and delete.
struct HostsView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.colorScheme) private var colorScheme
    @State private var editing: Host?

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
                }
                .swipeActions {
                    Button("Delete", role: .destructive) { store.remove(host) }
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
    }
}
