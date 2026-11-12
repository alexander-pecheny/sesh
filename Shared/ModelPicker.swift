import SwiftUI

/// A model Claude's `/model` menu offers, as the helper read it.
struct AgentModel: Decodable, Identifiable, Hashable {
    let label: String
    let note: String
    let current: Bool
    var id: String { label }
}

/// The toolbar's switch of Claude's model for this session, which leaves its default alone.
struct ModelButton: View {
    @ObservedObject var conversation: Conversation
    @State private var open = false

    var body: some View {
        Button { open = true } label: { Label("Model", image: "sparkles") }
            .help(conversation.state == "working" ? "Switch the model once Claude is done" : "Switch the model for this session")
            .disabled(conversation.state == "working")
            .popover(isPresented: $open) {
                ModelPicker(conversation: conversation) { open = false }
                    .presentationCompactAdaptation(.popover)
            }
    }
}

private struct ModelPicker: View {
    @Environment(\.flavour) private var flavour
    let conversation: Conversation
    let done: () -> Void
    @State private var models: [AgentModel]?
    @State private var problem: String?
    @State private var picking: String?
    private static let width: CGFloat = 300

    var body: some View {
        VStack(alignment: .leading, spacing: Metric.gap) {
            Text("Model for this session").font(.ui(Metric.body).weight(.medium)).foregroundStyle(flavour(.text))
            Text("New sessions keep the default.").font(.ui(Metric.note)).foregroundStyle(flavour(.subtext0))
            if let models {
                ForEach(models) { model in row(model) }
            } else if problem == nil {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity)
            }
            if let problem {
                Text(problem).font(.ui(Metric.small)).foregroundStyle(flavour(.red)).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(Metric.pad)
        .frame(width: Self.width)
        .background(flavour(.mantle))
        .task {
            switch await conversation.models() {
            case .success(let read): models = read
            case .failure(let failed): problem = failed.text
            }
        }
    }

    private func row(_ model: AgentModel) -> some View {
        Button {
            guard !model.current else { return done() }
            picking = model.label
            Task {
                problem = await conversation.pick(model: model)
                picking = nil
                if problem == nil { done() }
            }
        } label: {
            HStack(spacing: Metric.gap) {
                VStack(alignment: .leading, spacing: 0) {
                    Text(model.label).font(.ui(Metric.label)).foregroundStyle(flavour(.text))
                    if !model.note.isEmpty {
                        Text(model.note).font(.ui(Metric.small)).foregroundStyle(flavour(.subtext0))
                    }
                }
                Spacer(minLength: 0)
                if picking == model.label {
                    ProgressView().controlSize(.small)
                } else if model.current {
                    Image.lucide("circle-check", size: Metric.label).foregroundStyle(flavour(.mauve))
                }
            }
            .padding(.vertical, Metric.tiny)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .disabled(picking != nil)
    }
}
