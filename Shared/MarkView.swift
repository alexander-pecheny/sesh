import SwiftUI

/// One Agent at a glance, as a dot: hollow when idle and seen, yellow while working, blue
/// while idle with background work running, green for a finished turn not seen yet, and red,
/// blinking, when it waits for the user to choose.
struct MarkView: View {
    let mark: Library.Mark?

    var body: some View {
        if let mark {
            Group {
                if mark == .seen {
                    Circle().strokeBorder(.secondary, lineWidth: 1)
                } else if mark == .waiting {
                    Circle().fill(.red)
                        .phaseAnimator([1, Self.dim]) { $0.opacity($1) } animation: { _ in .easeInOut(duration: Self.blink) }
                } else {
                    Circle().fill(colour(mark))
                }
            }
            .frame(width: Self.size, height: Self.size)
            .help(help(mark))
        }
    }

    static let size: CGFloat = 8
    private static let dim = 0.2
    private static let blink = 0.6

    private func colour(_ mark: Library.Mark) -> Color {
        switch mark {
        case .working: .yellow
        case .background: .blue
        case .finished: .green
        case .waiting: .red
        case .seen: .clear
        }
    }

    private func help(_ mark: Library.Mark) -> String {
        switch mark {
        case .working: "An Agent is working"
        case .background: "Idle, with work still running in the background"
        case .finished: "An Agent finished; not seen yet"
        case .waiting: "An Agent is waiting for you to choose"
        case .seen: "Idle"
        }
    }
}

/// A Task's running Agents, a dot each, the most pressing first.
struct MarksView: View {
    let marks: [Library.Mark]
    private static let most = 4

    var body: some View {
        HStack(spacing: MarkView.size / 2) {
            ForEach(Array(marks.prefix(Self.most).enumerated()), id: \.offset) { MarkView(mark: $0.element) }
        }
    }
}
