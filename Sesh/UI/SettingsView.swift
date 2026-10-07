import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var showingKeys = false
    @State private var showingLicenses = false
    private static let sizes = 6.0...32.0

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    NavigationLink { HostsView() } label: {
                        LabeledContent("Hosts") { Text("\(store.hosts.count)") }
                    }
                    Button { showingKeys = true } label: {
                        LabeledContent("Keys") { Text("\(store.keys.count)") }
                    }
                } footer: {
                    Text("Vaults and Agents run on these Hosts.")
                }
                Section {
                    Stepper(value: $store.fontSize, in: Self.sizes, step: 1) {
                        LabeledContent("Font size") { Text("\(Int(store.fontSize))").font(.ui(Metric.label)) }
                    }
                    .accessibilityLabel("font size")
                    Text("New Terminals use this size. Pinch inside a Terminal to change that one only.")
                        .font(.ui(Metric.caption))
                    Toggle("Compress images", isOn: $store.compressUploads).font(.ui(Metric.label))
                    Text("Uploaded images are cut to 1568px on the longest edge. Videos are never touched.")
                        .font(.ui(Metric.caption))
                }
                Button("Licences") { showingLicenses = true }.font(.ui(Metric.label))
            }
            .font(.ui(Metric.label))
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .sheet(isPresented: $showingKeys) { KeysView() }
            .sheet(isPresented: $showingLicenses) { LicensesView() }
        }
    }
}
