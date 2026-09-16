import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.dismiss) private var dismiss
    @State private var showingLicenses = false

    var body: some View {
        NavigationStack {
            Form {
                Stepper(value: $store.fontSize, in: 6...32, step: 1) {
                    LabeledContent("Font size") { Text("\(Int(store.fontSize))").font(.ui(14)) }
                }
                .accessibilityLabel("font size")
                Text("New Tabs use this size. Pinch inside a Tab to change that Tab only.")
                    .font(.ui(12))
                Toggle("Compress images", isOn: $store.compressUploads).font(.ui(14))
                Text("Uploaded images are cut to 1568px on the longest edge. Videos are never touched.")
                    .font(.ui(12))
                Button("Licences") { showingLicenses = true }.font(.ui(14))
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .sheet(isPresented: $showingLicenses) { LicensesView() }
        }
    }
}
