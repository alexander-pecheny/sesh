import SwiftUI

/// What the terminal shows ahead of the Transcript, muted under a label that says where it
/// comes from. It goes once the Transcript holds the same text and the reply shows above.
struct LiveTail: View {
    @Environment(\.colorScheme) private var colorScheme
    let text: String

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.tiny) {
            HStack(spacing: Metric.tiny) {
                Image.lucide("terminal", size: Metric.caption)
                Text("Live from the terminal")
            }
            .font(.ui(Metric.caption))
            .foregroundStyle(flavour(.overlay0))
            Text(text)
                .font(.ui(Metric.body))
                .foregroundStyle(flavour(.overlay1))
                .textSelection(.enabled)
        }
        .padding(.leading, Metric.gap)
        .overlay(alignment: .leading) { Rectangle().fill(flavour(.surface1)).frame(width: 2) }
        .readable()
        .accessibilityElement(children: .combine)
    }
}
