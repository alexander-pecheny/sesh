import SwiftUI
import UniformTypeIdentifiers

struct KeysView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var importing = false

    var body: some View {
        NavigationStack {
            List {
                ForEach(store.keys) { key in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(key.name).font(.ui(15))
                        Text(key.publicKey).font(.ui(10)).lineLimit(2).foregroundStyle(.secondary)
                        Button("Copy public key") { UIPasteboard.general.string = key.publicKey }
                            .font(.ui(12))
                    }
                    .swipeActions { Button("Delete", role: .destructive) { store.remove(key) } }
                }
                if store.keys.isEmpty {
                    Text("No Keys yet").font(.ui(14)).foregroundStyle(.secondary)
                }
            }
            .navigationTitle("Keys")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button { importing = true } label: { Label("Import", image: "plus") }
                }
            }
        }
        .sheet(isPresented: $importing) { KeyImportView() }
    }
}

struct KeyImportView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var material = ""
    @State private var passphrase = ""
    @State private var error: String?
    @State private var picking = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledField("Name", "id_ed25519", text: $name)
                    LabeledField("Passphrase", "none", text: $passphrase)
                    Button("Choose a file") { picking = true }.font(.ui(14))
                }
                Section {
                    TextEditor(text: $material)
                        .font(.ui(11))
                        .frame(minHeight: 160)
                        .autocorrectionDisabled()
                        .textInputAutocapitalization(.never)
                } header: {
                    Text("Private key")
                } footer: {
                    if let error { Text(error).foregroundStyle(.red) }
                }
            }
            .font(.ui(14))
            .navigationTitle("Import Key")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) { Button("Import", action: save) }
            }
            .fileImporter(isPresented: $picking, allowedContentTypes: [.data]) { result in
                guard case let .success(url) = result else { return }
                let opened = url.startAccessingSecurityScopedResource()
                defer { if opened { url.stopAccessingSecurityScopedResource() } }
                material = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
                if name.isEmpty { name = url.lastPathComponent }
            }
        }
    }

    private func save() {
        let (line, message) = PrivateKey.publicKey(material, passphrase: passphrase)
        guard let line else {
            error = message
            return
        }
        let key = Key(
            name: name.isEmpty ? String(line.split(separator: " ").first ?? "key") : name,
            publicKey: line)
        store.add(key, material: material, passphrase: passphrase.isEmpty ? nil : passphrase)
        dismiss()
    }
}
