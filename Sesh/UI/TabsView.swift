import SwiftUI

struct TabsView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var tabs: Tabs
    @State private var switching = false

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(spacing: 0) {
            if let session = tabs.active {
                TabBar(tabs: tabs, session: session) { switching = true }
                SessionTab(session: session).id(session.id)
            }
        }
        .background(flavour(.base))
        .sheet(isPresented: $switching) { SwitcherView(tabs: tabs) }
    }
}

private struct TabBar: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var tabs: Tabs
    @ObservedObject var session: SeshSession
    let switcher: () -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        HStack(spacing: 8) {
            Text(session.name).font(.mono(14)).lineLimit(1).foregroundStyle(flavour(.text))
            Spacer(minLength: 4)
            Text(session.status).font(.mono(11)).lineLimit(1).foregroundStyle(flavour(.subtext0))
            Button(action: switcher) {
                Text("\(tabs.sessions.count)")
                    .font(.mono(12))
                    .frame(minWidth: 22, minHeight: 22)
                    .background(flavour(.surface0), in: .rect(cornerRadius: 5))
                    .foregroundStyle(flavour(.text))
            }
            .accessibilityLabel("Tabs")
            .accessibilityValue("\(tabs.sessions.count)")
            Button { tabs.close(session) } label: {
                Image(systemName: "xmark.circle.fill").foregroundStyle(flavour(.overlay1))
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
                ForEach(tabs.sessions) { session in
                    HStack {
                        Button {
                            tabs.show(session)
                            dismiss()
                        } label: {
                            Row(session: session)
                        }
                        Button {
                            tabs.close(session)
                            if tabs.sessions.isEmpty { dismiss() }
                        } label: {
                            Image(systemName: "xmark").foregroundStyle(flavour(.overlay1))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Close \(session.name)")
                    }
                    .listRowBackground(flavour(.mantle))
                }
                Button("Open another Host") {
                    tabs.showHosts()
                    dismiss()
                }
                .font(.mono(15))
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

private struct Row: View {
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var session: SeshSession

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(session.name).font(.mono(15)).foregroundStyle(flavour(.text))
            Text(session.status).font(.mono(11)).foregroundStyle(flavour(.subtext0))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
