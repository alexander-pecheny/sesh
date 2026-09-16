import SwiftUI
import GhosttyKit

struct SessionTab: View {
    @EnvironmentObject private var store: Store
    @Environment(\.colorScheme) private var colorScheme
    @ObservedObject var session: SeshSession

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(spacing: 0) {
            Ghostty.Terminal(view: session.terminal)
                .overlay(alignment: .topLeading) { copyButton }
                .overlay { disconnected }
            KeysRow(
                input: session.terminal.input,
                uploading: session.uploading?.fraction,
                canUpload: session.canUpload
            ) { action in
                switch action {
                case .upload where session.uploading != nil: session.cancelUpload()
                default: session.terminal.perform(action)
                }
            }
        }
            .sheet(isPresented: $session.editing) {
                EditorView(session: session) { text, enter in session.sendDraft(text, enter: enter) }
                    .environmentObject(store)
            }
            .sheet(isPresented: $session.picking) {
                PhotoPicker { results in
                    session.picking = false
                    session.upload(results) { session.terminal.paste($0) }
                }
                .ignoresSafeArea()
            }
            .uploadFailure($session.uploadError)
            .onChange(of: session.editing) { _, editing in
                if !editing { _ = session.terminal.becomeFirstResponder() }
            }
            .onChange(of: session.picking) { _, picking in
                if !picking { _ = session.terminal.becomeFirstResponder() }
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

    @ViewBuilder private var disconnected: some View {
        if session.ended {
            VStack(spacing: 12) {
                Text(session.reason)
                    .font(.ui(13))
                    .multilineTextAlignment(.center)
                    .foregroundStyle(flavour(.text))
                Button("Reconnect") { session.reconnect() }
                    .font(.ui(14))
                    .buttonStyle(.borderedProminent)
                    .tint(flavour(.mauve))
            }
            .padding(20)
            .background(flavour(.mantle), in: .rect(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(flavour(.surface1)))
            .padding(24)
        }
    }

    @ViewBuilder private var copyButton: some View {
        if let anchor = session.selection {
            Button("Copy") { session.copySelection() }
                .font(.ui(13))
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(flavour(.surface1), in: .capsule)
                .foregroundStyle(flavour(.text))
                .offset(x: max(anchor.x - 20, 4), y: max(anchor.y - 44, 4))
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
                        .font(.ui(13))
                }
                if let previous = question.previous {
                    LabeledContent("Recorded") { Text(previous).font(.ui(11)) }
                }
                LabeledContent(question.previous == nil ? "Fingerprint" : "Offered") {
                    Text(question.fingerprint).font(.ui(11))
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
                    Text(question.instruction).font(.ui(13))
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
            .font(.ui(14))
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
