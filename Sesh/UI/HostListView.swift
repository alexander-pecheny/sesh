import SwiftUI

struct HostListView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var tabs: Tabs
    let open: (Host) -> Void
    @State private var editing: Host?
    @State private var showingKeys = false
    @State private var showingSettings = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        NavigationStack {
            Group {
                if store.hosts.isEmpty { empty } else { list }
            }
            .background(flavour(.base))
            .navigationTitle("Hosts")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { showingKeys = true } label: { Label("Keys", systemImage: "key") }
                }
                ToolbarItem(placement: .topBarLeading) {
                    Button { showingSettings = true } label: {
                        Label("Settings", systemImage: "gearshape")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { editing = Host() } label: { Label("Add Host", systemImage: "plus") }
                }
                if let session = tabs.sessions.last {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button { tabs.show(session) } label: {
                            Label("Tabs", systemImage: "rectangle.stack")
                        }
                    }
                }
            }
        }
        .tint(flavour(.mauve))
        .sheet(item: $editing) { HostFormView(host: $0) }
        .sheet(isPresented: $showingKeys) { KeysView() }
        .sheet(isPresented: $showingSettings) { SettingsView() }
    }

    private var empty: some View {
        VStack(spacing: 16) {
            Image(systemName: "terminal").font(.system(size: 48)).foregroundStyle(flavour(.overlay1))
            Text("No Hosts yet").font(.mono(17)).foregroundStyle(flavour(.text))
            Button("Add a Host") { editing = Host() }
                .font(.mono(15))
                .buttonStyle(.borderedProminent)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var list: some View {
        List {
            ForEach(store.hosts) { host in
                Button { open(host) } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(host.title).font(.mono(16)).foregroundStyle(flavour(.text))
                        Text(host.subtitle).font(.mono(12)).foregroundStyle(flavour(.subtext0))
                    }
                }
                .listRowBackground(flavour(.mantle))
                .contextMenu { Button("Edit") { editing = host } }
                .swipeActions {
                    Button("Delete", role: .destructive) { store.remove(host) }
                }
            }
        }
        .scrollContentBackground(.hidden)
    }
}

extension Font {
    static func mono(_ size: CGFloat) -> Font { .custom("JetBrainsMono NF", size: size) }
}
