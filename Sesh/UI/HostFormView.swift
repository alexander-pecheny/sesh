import SwiftUI

struct HostFormView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var host: Host
    @State private var error: String?

    init(host: Host) { _host = State(initialValue: host) }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledField("Name", host.title, text: $host.name)
                    LabeledField("Address", "example.com", text: $host.address)
                    LabeledField("Port", "22", value: $host.port)
                    LabeledField("User", NSUserName(), text: $host.user)
                }
                Section {
                    Picker("Transport", selection: $host.transport) {
                        ForEach(Host.Transport.allCases) { transport in
                            Text(transport.label).tag(transport)
                        }
                    }
                    Picker("Key", selection: $host.keyID) {
                        Text("None").tag(UUID?.none)
                        ForEach(store.keys) { Text($0.name).tag(UUID?.some($0.id)) }
                    }
                    if host.transport == .ssh {
                        Toggle("Agent forwarding", isOn: $host.agentForwarding)
                    }
                }
                Section {
                    LabeledField("Extra flags ssh", "-o ServerAliveInterval=30", text: $host.sshFlags)
                    LabeledField("Extra flags mosh", "--predict=adaptive", text: $host.moshFlags)
                    LabeledField("Remote command", "tmux attach", text: $host.remoteCommand)
                } footer: {
                    if let error { Text(error).foregroundStyle(.red) }
                }
            }
            .font(.mono(14))
            .navigationTitle(host.name.isEmpty ? "New Host" : host.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Save", action: save) }
            }
        }
    }

    private func save() {
        error = Flags.validate(.ssh, host.sshFlags) ?? Flags.validate(.mosh, host.moshFlags)
        guard error == nil else { return }
        store.upsert(host)
        dismiss()
    }
}

struct LabeledField: View {
    let label: String
    let placeholder: String
    @Binding var text: String

    init(_ label: String, _ placeholder: String, text: Binding<String>) {
        self.label = label
        self.placeholder = placeholder
        _text = text
    }

    init(_ label: String, _ placeholder: String, value: Binding<Int>) {
        self.label = label
        self.placeholder = placeholder
        _text = Binding(get: { String(value.wrappedValue) }, set: { value.wrappedValue = Int($0) ?? 0 })
    }

    var body: some View {
        LabeledContent(label) {
            TextField(placeholder, text: $text)
                .multilineTextAlignment(.trailing)
                .autocorrectionDisabled()
                .textInputAutocapitalization(.never)
        }
    }
}
