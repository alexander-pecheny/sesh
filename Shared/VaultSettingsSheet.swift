import SwiftUI

/// Settings that every device opening the Vault shares, kept in the Vault itself.
struct VaultSettingsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var vault: Vault
    @State private var links: [LinkRule] = []
    @State private var remote = true

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }
    private var kept: [LinkRule] { links.filter { !$0.pattern.isEmpty && !$0.url.isEmpty } }
    private var valid: Bool { kept.allSatisfy { (try? Regex($0.pattern)) != nil } }

    var body: some View {
        #if os(macOS)
        Form { sections }
            .formStyle(.grouped)
            .frame(width: 560)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save", action: save).disabled(!valid) }
            }
            .onAppear(perform: load)
        #else
        SheetForm(title: vault.name, action: "Save", enabled: valid, working: false, perform: save) { sections }
            .onAppear(perform: load)
        #endif
    }

    @ViewBuilder private var sections: some View {
        Section {
            Toggle("Claude remote control", isOn: $remote)
        } footer: {
            Text("New Claude sessions can be reached from the Claude app. Each new session can still choose otherwise.")
        }
        Section {
            ForEach($links) { $rule in
                VStack(alignment: .leading) {
                    HStack {
                        TextField("Pattern", text: $rule.pattern, prompt: Text(verbatim: "[A-Z]+-[0-9]+"))
                        Button(role: .destructive) { links.removeAll { $0.id == rule.id } } label: { Image(systemName: "minus.circle") }
                            .buttonStyle(.borderless)
                            .accessibilityLabel("Remove link")
                    }
                    TextField("Link", text: $rule.url, prompt: Text(verbatim: "https://linear.app/team/issue/$0"))
                    if !rule.pattern.isEmpty, (try? Regex(rule.pattern)) == nil {
                        Text("Not a regular expression.").font(.ui(Metric.caption)).foregroundStyle(flavour(.red))
                    }
                }
                .font(.system(size: Metric.label, design: .monospaced))
                .autocorrectionDisabled()
                #if os(iOS)
                .textInputAutocapitalization(.never)
                #endif
            }
            Button("Add Link") { links.append(LinkRule()) }
        } header: {
            Text("Links in Agent messages")
        } footer: {
            Text("Text matching a pattern becomes a link. In the link, $0 stands for the matched text and $1 for its first group.")
        }
    }

    private func load() {
        links = vault.settings.body.links ?? []
        remote = vault.settings.body.remote ?? true
    }

    private func save() {
        var settings = vault.settings
        settings.body.links = kept
        settings.body.remote = remote
        vault.write(settings)
        dismiss()
    }
}
