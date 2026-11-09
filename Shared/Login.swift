import SwiftUI

/// Claude asking to be signed in: its sign-in page opens from here, and the code that page shows
/// goes back to Claude.
struct LoginCard: View {
    @Environment(\.flavour) private var flavour
    @Environment(\.openURL) private var openURL
    let url: URL
    /// Signs Claude in with the code, or says why it could not.
    let send: (String) async -> String?
    @State private var code = ""
    @State private var problem: String?
    @State private var sending = false

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.pad) {
            HStack(spacing: Metric.gap) {
                Image.lucide("key-round", size: Metric.title).foregroundStyle(flavour(.mauve))
                Text("Claude needs you to sign in").font(.ui(Metric.body).weight(.medium)).foregroundStyle(flavour(.text))
            }
            Text("Open the sign-in page and approve, then paste the code it shows.")
                .font(.ui(Metric.note))
                .foregroundStyle(flavour(.subtext0))
            Button { openURL(url) } label: { Text("Open sign-in page").frame(maxWidth: .infinity) }
                .buttonStyle(FilledButton(fill: flavour(.mauve), text: flavour(.base)))
            HStack(spacing: Metric.gap) {
                TextField("Code", text: $code)
                    .textFieldStyle(.plain)
                    .font(.system(size: Metric.label, design: .monospaced))
                    .autocorrectionDisabled()
                    #if os(iOS)
                    .textInputAutocapitalization(.never)
                    #endif
                    .onSubmit { submit(code) }
                    .padding(Metric.gap)
                    .background(flavour(.base), in: .rect(cornerRadius: Metric.corner))
                #if os(iOS)
                // Pastes without iOS asking whether Sesh may read the clipboard.
                PasteButton(payloadType: String.self) { pasted in
                    if let pasted = pasted.first { Task { @MainActor in submit(pasted) } }
                }
                .labelStyle(.iconOnly)
                .disabled(sending)
                #endif
                Button(sending ? "Signing in…" : "Sign in") { submit(code) }
                    .buttonStyle(.bordered)
                    .tint(flavour(.mauve))
                    .disabled(sending || code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            if let problem {
                Text(problem).font(.ui(Metric.small)).foregroundStyle(flavour(.red))
            }
        }
        .padding(Metric.pad)
        .background(flavour(.mantle), in: .rect(cornerRadius: Metric.corner))
        .overlay(RoundedRectangle(cornerRadius: Metric.corner).stroke(flavour(.mauve).opacity(0.5)))
    }

    private func submit(_ text: String) {
        guard !sending, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        code = text
        sending = true
        Task {
            problem = await send(text)
            sending = false
        }
    }
}

/// The sign-in a Conversation shows, over the Agent's own terminal.
struct LoginBar: View {
    @ObservedObject var conversation: Conversation

    var body: some View {
        if let permission = conversation.permissions.first(where: { $0.tool == "login" }), let url = permission.url.flatMap(URL.init(string:)) {
            LoginCard(url: url) { await conversation.login($0, permission) }
                .id(permission.id)
                .padding(Metric.pad)
        }
    }
}
