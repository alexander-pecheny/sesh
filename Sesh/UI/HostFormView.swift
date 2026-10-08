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
                } footer: {
                    if let error { Text(error).foregroundStyle(.red) }
                }
            }
            .font(.ui(14))
            .navigationTitle(host.name.isEmpty ? "New Host" : host.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save", action: save).disabled(host.address.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            }
        }
    }

    private func save() {
        error = Flags.validate(.ssh, host.sshFlags) ?? Flags.validate(.mosh, host.moshFlags)
        guard error == nil else { return }
        if host.port == 0 { host.port = 22 }
        store.upsert(host)
        dismiss()
    }
}

struct LabeledField: View {
    let label: String
    let placeholder: String
    @Binding var text: String
    var secure = false
    var number = false

    init(_ label: String, _ placeholder: String, text: Binding<String>, secure: Bool = false) {
        self.label = label
        self.placeholder = placeholder
        _text = text
        self.secure = secure
    }

    /// An empty field shows the placeholder rather than a 0.
    init(_ label: String, _ placeholder: String, value: Binding<Int>) {
        self.label = label
        self.placeholder = placeholder
        _text = Binding(get: { value.wrappedValue == 0 ? "" : String(value.wrappedValue) }, set: { value.wrappedValue = Int($0) ?? 0 })
        number = true
    }

    var body: some View {
        LabeledContent(label) {
            Group {
                if secure { SecureField(placeholder, text: $text) } else { TextField(placeholder, text: $text) }
            }
            .multilineTextAlignment(.trailing)
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            .keyboardType(number ? .numberPad : .default)
        }
    }
}
