import SwiftUI

/// What a Host's Link asks before it can connect: a new or changed host key, or a passphrase
/// or password, as ssh would ask at the terminal.
struct Questions: View {
    @ObservedObject var link: CoreLink

    var body: some View {
        Color.clear
            .sheet(item: $link.hostKeyQuestion) { question in
                HostKeySheet(question: question, host: link.title) { link.answerHostKey($0) }
            }
            .sheet(item: $link.authQuestion) { question in
                AuthSheet(question: question, host: link.title) { link.answerPrompt(question, $0) }
            }
    }
}

private struct HostKeySheet: View {
    let question: HostKeyQuestion
    let host: String
    let answer: (Bool) -> Void

    var body: some View {
        Form {
            Text(question.previous == nil
                 ? "\(host) has not been seen before. Its key goes in ~/.ssh/known_hosts if you accept it."
                 : "The key \(host) offers is not the one ~/.ssh/known_hosts records. Someone may be listening.")
            if let previous = question.previous {
                LabeledContent("Recorded") { Text(previous).font(.caption.monospaced()).textSelection(.enabled) }
            }
            LabeledContent(question.previous == nil ? "Fingerprint" : "Offered") {
                Text(question.fingerprint).font(.caption.monospaced()).textSelection(.enabled)
            }
        }
        .padding()
        .frame(width: 460)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { answer(false) } }
            ToolbarItem(placement: .confirmationAction) { Button(question.previous == nil ? "Accept" : "Replace") { answer(true) } }
        }
        .interactiveDismissDisabled()
    }
}

private struct AuthSheet: View {
    let question: AuthQuestion
    let host: String
    let answer: ([String]?) -> Void
    @State private var values: [String]
    @FocusState private var focused: Int?

    init(question: AuthQuestion, host: String, answer: @escaping ([String]?) -> Void) {
        self.question = question
        self.host = host
        self.answer = answer
        _values = State(initialValue: Array(repeating: "", count: question.prompts.count))
    }

    private var passphrase: Bool { question.title == "Passphrase" }

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.pad) {
            HStack(spacing: Metric.gap) {
                Image.lucide("key-round", size: Metric.title)
                Text(passphrase ? "Unlock your SSH key" : "Sign in to \(host)").font(.headline)
            }
            Text(question.instruction.isEmpty ? "\(host) asks:" : question.instruction)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(Array(question.prompts.enumerated()), id: \.offset) { index, prompt in
                Group {
                    if prompt.echo {
                        TextField(prompt.text, text: $values[index])
                    } else {
                        SecureField(prompt.text, text: $values[index])
                    }
                }
                .textFieldStyle(.roundedBorder)
                .controlSize(.large)
                .focused($focused, equals: index)
                .onSubmit { if index == question.prompts.count - 1 { answer(values) } else { focused = index + 1 } }
            }
            HStack {
                Text(host).font(.caption).foregroundStyle(.tertiary)
                Spacer()
                Button("Cancel", role: .cancel) { answer(nil) }.keyboardShortcut(.cancelAction)
                Button(passphrase ? "Unlock" : "Send") { answer(values) }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
            }
            .controlSize(.large)
        }
        .padding(Metric.wide + Metric.tiny)
        .frame(width: 420)
        .interactiveDismissDisabled()
        .onAppear { focused = 0 }
    }
}
