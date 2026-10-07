import SwiftUI

/// A Task's Agents at a glance, as one dot: hollow when idle and seen, yellow while working,
/// blue while idle with background work running, green for a finished turn not seen yet,
/// orange when an Agent waits on the user.
struct MarkView: View {
    let mark: Library.Mark?

    var body: some View {
        if let mark {
            Group {
                if mark == .seen {
                    Circle().strokeBorder(.secondary, lineWidth: 1)
                } else {
                    Circle().fill(colour(mark))
                }
            }
            .frame(width: 8, height: 8)
            .help(help(mark))
        }
    }

    private func colour(_ mark: Library.Mark) -> Color {
        switch mark {
        case .working: .yellow
        case .background: .blue
        case .finished: .green
        case .waiting: .orange
        case .seen: .clear
        }
    }

    private func help(_ mark: Library.Mark) -> String {
        switch mark {
        case .working: "An Agent is working"
        case .background: "Idle, with work still running in the background"
        case .finished: "An Agent finished; not seen yet"
        case .waiting: "An Agent is waiting for you"
        case .seen: "Idle"
        }
    }
}
