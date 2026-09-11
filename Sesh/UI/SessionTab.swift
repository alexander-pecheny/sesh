import SwiftUI
import GhosttyKit

struct SessionTab: View {
    @EnvironmentObject private var store: Store
    @Environment(\.colorScheme) private var colorScheme
    @StateObject private var session: SeshSession
    let onClose: () -> Void

    init(host: Host, store: Store, app: ghostty_app_t, onClose: @escaping () -> Void) {
        _session = StateObject(wrappedValue: SeshSession(host: host, store: store, app: app))
        self.onClose = onClose
    }

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(session.host.title).font(.mono(14)).foregroundStyle(flavour(.text))
                Spacer()
                Text(status).font(.mono(11)).foregroundStyle(flavour(.subtext0))
                Button {
                    session.close()
                    onClose()
                } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(flavour(.overlay1))
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            .background(flavour(.mantle))

            Ghostty.Terminal(view: session.terminal)
                .overlay(alignment: .topLeading) { copyButton }
        }
        .background(flavour(.base))
        .sheet(isPresented: $session.editing) {
            EditorView { text, enter in session.sendDraft(text, enter: enter) }
                .environmentObject(store)
        }
        .onChange(of: session.editing) { _, editing in
            if !editing { _ = session.terminal.becomeFirstResponder() }
        }
        .sheet(item: $session.hostKeyQuestion) { question in
            HostKeySheet(question: question, host: session.host) { session.answerHostKey($0) }
        }
        .sheet(item: $session.authQuestion) { question in
            AuthSheet(question: question, savePassword: $session.savePassword) {
                session.answerPrompt(question, $0)
            }
        }
    }

    @ViewBuilder private var copyButton: some View {
        if let anchor = session.selection {
            Button("Copy") { session.copySelection() }
                .font(.mono(13))
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(flavour(.surface1), in: .capsule)
                .foregroundStyle(flavour(.text))
                .offset(x: max(anchor.x - 20, 4), y: max(anchor.y - 44, 4))
        }
    }

    private var status: String {
        switch session.stage {
        case .connecting: "connecting"
        case .authenticating: "authenticating"
        case .bootstrapping: "starting mosh-server"
        case .connected: "connected"
        case .closed: "closed"
        case .failed: session.message
        }
    }
}

struct HostKeySheet: View {
    let question: SeshSession.HostKeyQuestion
    let host: Host
    let answer: (Bool) -> Void

    var body: some View {
        NavigationStack {
            Form {
                Section(question.previous == nil ? "First connection" : "Host key changed") {
                    Text(question.previous == nil
                         ? "\(host.address) has not been seen before. Accept its key?"
                         : "The key for \(host.address) is not the one recorded. Someone may be listening.")
                        .font(.mono(13))
                }
                if let previous = question.previous {
                    LabeledContent("Recorded") { Text(previous).font(.mono(11)) }
                }
                LabeledContent(question.previous == nil ? "Fingerprint" : "Offered") {
                    Text(question.fingerprint).font(.mono(11))
                }
            }
            .navigationTitle("Host key")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { answer(false) } }
                ToolbarItem(placement: .confirmationAction) {
                    Button(question.previous == nil ? "Accept" : "Replace") { answer(true) }
                }
            }
        }
        .interactiveDismissDisabled()
    }
}

struct AuthSheet: View {
    let question: SeshSession.AuthQuestion
    @Binding var savePassword: Bool
    let answer: ([String]?) -> Void
    @State private var values: [String]

    init(question: SeshSession.AuthQuestion, savePassword: Binding<Bool>, answer: @escaping ([String]?) -> Void) {
        self.question = question
        _savePassword = savePassword
        self.answer = answer
        _values = State(initialValue: Array(repeating: "", count: question.prompts.count))
    }

    var body: some View {
        NavigationStack {
            Form {
                if !question.instruction.isEmpty {
                    Text(question.instruction).font(.mono(13))
                }
                ForEach(Array(question.prompts.enumerated()), id: \.offset) { index, prompt in
                    if prompt.echo {
                        TextField(prompt.text, text: $values[index]).autocorrectionDisabled()
                    } else {
                        SecureField(prompt.text, text: $values[index])
                    }
                }
                Toggle("Save password", isOn: $savePassword)
            }
            .font(.mono(14))
            .textInputAutocapitalization(.never)
            .navigationTitle(question.title.isEmpty ? "Authentication" : question.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { answer(nil) } }
                ToolbarItem(placement: .confirmationAction) { Button("Send") { answer(values) } }
            }
        }
        .interactiveDismissDisabled()
    }
}
