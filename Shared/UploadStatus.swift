import SwiftUI

/// An Upload in flight as a ring, which opens on a tap to show what is being sent and to stop it.
struct UploadStatus: View {
    @Environment(\.flavour) private var flavour
    /// How far it has got, or nil while that is unknown and the ring turns instead.
    let fraction: Double?
    let previews: [PlatformImage]
    /// The square that takes the tap.
    var side = Metric.control
    let cancel: () -> Void
    @State private var open = false

    var body: some View {
        Button { open = true } label: {
            UploadRing(fraction: fraction, colour: flavour(.mauve))
                .frame(width: side, height: side)
                .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("upload in progress")
        .accessibilityValue(fraction.map { "\(Int($0 * 100))%" } ?? "")
        .popover(isPresented: $open) {
            UploadPreview(previews: previews) {
                open = false
                cancel()
            }
            .presentationCompactAdaptation(.popover)
        }
    }
}

private struct UploadPreview: View {
    @Environment(\.flavour) private var flavour
    let previews: [PlatformImage]
    let cancel: () -> Void

    private static let shown = 3
    private static let thumbnail: CGFloat = 160

    var body: some View {
        VStack(spacing: Metric.pad) {
            if previews.isEmpty {
                ProgressView()
            } else {
                HStack(spacing: Metric.gap) {
                    ForEach(previews.prefix(Self.shown).indices, id: \.self) { index in
                        Image(platform: previews[index]).resizable().scaledToFit()
                            .clipShape(.rect(cornerRadius: Metric.corner))
                            .frame(maxWidth: Self.thumbnail, maxHeight: Self.thumbnail)
                    }
                }
                if previews.count > Self.shown {
                    Text("and \(previews.count - Self.shown) more").font(.ui(Metric.note)).foregroundStyle(flavour(.subtext0))
                }
            }
            Button("Cancel upload", role: .destructive, action: cancel)
                .buttonStyle(.bordered)
                .tint(flavour(.red))
                .foregroundStyle(flavour(.red))
        }
        .padding(Metric.wide)
    }
}

/// The ring fills as the files go. A transfer that does not report its progress turns a quarter arc instead.
struct UploadRing: View {
    let fraction: Double?
    let colour: Color
    @State private var turned = false

    private static let size: CGFloat = 16
    private static let line: CGFloat = 2.5
    private static let quarter = 0.25

    var body: some View {
        Circle()
            .trim(from: 0, to: fraction.map { max($0, 0.02) } ?? Self.quarter)
            .stroke(colour, style: .init(lineWidth: Self.line, lineCap: .round))
            .rotationEffect(.degrees(fraction == nil && turned ? 270 : -90))
            .animation(fraction == nil ? .linear(duration: 1).repeatForever(autoreverses: false) : .default, value: turned)
            .background(Circle().stroke(colour.opacity(Self.quarter), lineWidth: Self.line))
            .frame(width: Self.size, height: Self.size)
            .onAppear { turned = true }
    }
}
