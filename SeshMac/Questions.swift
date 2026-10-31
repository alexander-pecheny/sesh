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

    var body: some View {
        Form {
            Text(question.instruction.isEmpty ? "\(host) asks:" : question.instruction)
            ForEach(Array(question.prompts.enumerated()), id: \.offset) { index, prompt in
                Group {
                    if prompt.echo {
                        TextField(prompt.text, text: $values[index])
                    } else {
                        SecureField(prompt.text, text: $values[index])
                    }
                }
                .focused($focused, equals: index)
                .onSubmit { if index == question.prompts.count - 1 { answer(values) } else { focused = index + 1 } }
            }
        }
        .padding()
        .frame(width: 420)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Cancel") { answer(nil) } }
            ToolbarItem(placement: .confirmationAction) { Button("Send") { answer(values) } }
        }
        .interactiveDismissDisabled()
        .onAppear { focused = 0 }
    }
}
