import SwiftUI

/// A herdr pane's own screen, attached over an ordinary Session to its Host, with the keys
/// row, the Editor and Uploads as in any terminal. When the attach ends because the pane is
/// gone, the Tab goes too; when only the connection went, Reconnect attaches again.
struct PaneTab: View {
    @Environment(\.colorScheme) private var colorScheme
    @EnvironmentObject private var library: Library
    @EnvironmentObject private var ghostty: Ghostty.App
    let key: String
    let pane: String
    let machine: Machine
    let gone: () -> Void
    @State private var session: SeshSession?
    @State private var problem: String?
    @State private var attempt = 0

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        Group {
            if let session {
                Attached(session: session, pane: pane, machine: machine, gone: gone)
            } else if let problem {
                VStack(spacing: Metric.pad) {
                    Text(problem).font(.ui(Metric.label)).foregroundStyle(flavour(.text)).multilineTextAlignment(.center)
                    Button("Try Again") {
                        self.problem = nil
                        attempt += 1
                    }
                    .buttonStyle(.borderedProminent)
                    Button("Close Tab", action: gone)
                }
                .padding(Metric.wide)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .task(id: attempt) {
            guard let app = ghostty.app else { return problem = "The terminal could not start." }
            switch await library.session(for: key, pane: pane, on: machine, app: app) {
            case .success(let found): session = found
            case .failure(let failure): problem = failure.message
            }
        }
    }
}

private struct Attached: View {
    @ObservedObject var session: SeshSession
    let pane: String
    let machine: Machine
    let gone: () -> Void

    var body: some View {
        SessionTab(session: session)
            .onChange(of: session.ended) { _, ended in
                guard ended else { return }
                // Only a pane herdr says is gone closes the Tab; a dropped connection reconnects.
                Task { if case .gone = await Herdr.look(for: pane, on: machine) { gone() } }
            }
    }
}
