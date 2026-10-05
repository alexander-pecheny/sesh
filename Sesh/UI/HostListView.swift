import SwiftUI

struct HostListView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var tabs: Tabs
    let open: (Host, Host.UIMode) -> Void
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
                    Button { showingKeys = true } label: { Label("Keys", image: "key-round") }
                }
                ToolbarItem(placement: .topBarLeading) {
                    Button { showingSettings = true } label: {
                        Label("Settings", image: "settings")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { editing = Host() } label: { Label("Add Host", image: "plus") }
                }
                if let tab = tabs.all.last {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button { tabs.show(tab) } label: {
                            Label("Tabs", image: "layers")
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
            Image.lucide("terminal", size: 48).foregroundStyle(flavour(.overlay1))
            Text("No Hosts yet").font(.ui(17)).foregroundStyle(flavour(.text))
            Button("Add a Host") { editing = Host() }
                .font(.ui(15))
                .buttonStyle(.borderedProminent)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var list: some View {
        List {
            ForEach(store.hosts) { host in
                Button { open(host, host.opens) } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(host.title).font(.ui(16)).foregroundStyle(flavour(.text))
                        Text(host.subtitle).font(.ui(12)).foregroundStyle(flavour(.subtext0))
                    }
                }
                .listRowBackground(flavour(.mantle))
                .contextMenu {
                    let other: Host.UIMode = host.opens == .terminal ? .projects : .terminal
                    Button("Open in \(other.label)") { open(host, other) }
                    Button("Edit") { editing = host }
                }
                .swipeActions {
                    Button("Delete", role: .destructive) { store.remove(host) }
                }
            }
        }
        .scrollContentBackground(.hidden)
    }
}

extension Image {
    /// A Lucide icon from the asset catalog, sized like a symbol of that point size.
    static func lucide(_ name: String, size: CGFloat = 17) -> some View {
        Image(name).resizable().frame(width: size, height: size)
    }
}

extension Font {
    static func ui(_ size: CGFloat) -> Font { .system(size: size) }
}

/// Sizes the Conversation and Projects share, so their rows and cards line up.
enum Metric {
    static let tiny: CGFloat = 4
    static let gap: CGFloat = 8
    static let pad: CGFloat = 12
    static let wide: CGFloat = 16
    static let corner: CGFloat = 10
    static let control: CGFloat = 38
    static let small: CGFloat = 11
    static let caption: CGFloat = 12
    static let note: CGFloat = 13
    static let label: CGFloat = 14
    static let body: CGFloat = 15
    static let title: CGFloat = 16
}
