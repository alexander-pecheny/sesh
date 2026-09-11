import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var store: Store
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Stepper(value: $store.fontSize, in: 6...32, step: 1) {
                    LabeledContent("Font size") { Text("\(Int(store.fontSize))").font(.mono(14)) }
                }
                .accessibilityLabel("font size")
                Text("New Tabs use this size. Pinch inside a Tab to change that Tab only.")
                    .font(.mono(12))
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
    }
}
