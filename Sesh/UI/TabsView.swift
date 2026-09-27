import SwiftUI

struct TabsView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var tabs: Tabs
    @State private var switching = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(spacing: 0) {
            if let tab = tabs.active {
                TabBar(tabs: tabs, tab: tab) { switching = true }
                switch tab {
                case .terminal(let session): SessionTab(session: session).id(tab.id)
                case .projects(let projects): ProjectsView(projects: projects).id(tab.id)
                }
            }
        }
        .background(flavour(.base))
        .sheet(isPresented: $switching) { SwitcherView(tabs: tabs) }
    }
}

private struct TabBar: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var tabs: Tabs
    let tab: Tab
    let switcher: () -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        HStack(spacing: 8) {
            TabTitle(tab: tab) { name, status in
                Text(name).font(.ui(14)).lineLimit(1).foregroundStyle(flavour(.text))
                Spacer(minLength: 4)
                Text(status).font(.ui(11)).lineLimit(1).foregroundStyle(flavour(.subtext0))
            }
            Button(action: switcher) {
                Text("\(tabs.all.count)")
                    .font(.ui(12))
                    .frame(minWidth: 22, minHeight: 22)
                    .background(flavour(.surface0), in: .rect(cornerRadius: 5))
                    .foregroundStyle(flavour(.text))
            }
            .accessibilityLabel("Tabs")
            .accessibilityValue("\(tabs.all.count)")
            Button { tabs.close(tab) } label: {
                Image.lucide("circle-x").foregroundStyle(flavour(.overlay1))
            }
            .accessibilityLabel("Close")
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(flavour(.mantle))
    }
}

struct SwitcherView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var tabs: Tabs

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        NavigationStack {
            List {
                ForEach(tabs.all) { tab in
                    HStack {
                        Button {
                            tabs.show(tab)
                            dismiss()
                        } label: {
                            TabTitle(tab: tab) { name, status in
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(name).font(.ui(15)).foregroundStyle(flavour(.text))
                                    Text(status).font(.ui(11)).foregroundStyle(flavour(.subtext0))
                                }
                                .frame(maxWidth: .infinity, alignment: .leading)
                            }
                        }
                        Button {
                            tabs.close(tab)
                            if tabs.all.isEmpty { dismiss() }
                        } label: {
                            Image.lucide("x", size: 15).foregroundStyle(flavour(.overlay1))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Close \(tab.content.name)")
                    }
                    .listRowBackground(flavour(.mantle))
                }
                Button("Open another Host") {
                    tabs.showHosts()
                    dismiss()
                }
                .font(.ui(15))
                .listRowBackground(flavour(.mantle))
            }
            .scrollContentBackground(.hidden)
            .background(flavour(.base))
            .navigationTitle("Tabs")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .tint(flavour(.mauve))
    }
}
