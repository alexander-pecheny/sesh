import SwiftUI

struct KeysRow: View {
    enum Action {
        case key(UInt32)
        case character(Character)
        case paste, editor, hide
    }

    static let height: CGFloat = 46

    @ObservedObject var input: InputState
    @Environment(\.colorScheme) private var colorScheme
    let send: (Action) -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        HStack(spacing: 4) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 4) {
                    cap("esc") { send(.key(Keycode.escape)) }
                    ForEach(Mods.all, id: \.rawValue) { modKey($0) }
                    cap("tab") { send(.key(Keycode.tab)) }
                    separator
                    ForEach(Array("~|/-"), id: \.self) { character in
                        cap(String(character), name: Self.names[character]) {
                            send(.character(character))
                        }
                    }
                    separator
                    arrow("chevron.left", "left", Keycode.left)
                    arrow("chevron.down", "down", Keycode.down)
                    arrow("chevron.up", "up", Keycode.up)
                    arrow("chevron.right", "right", Keycode.right)
                }
                .padding(.horizontal, 4)
                .fixedSize(horizontal: true, vertical: false)
            }
            HStack(spacing: 4) {
                cap(icon: "doc.on.clipboard", name: "paste") { send(.paste) }
                Button { input.mode = input.mode.next } label: {
                    HStack(spacing: 3) {
                        Image(systemName: input.mode.icon)
                        Text(input.mode.label).font(.mono(11)).lineLimit(1).fixedSize()
                    }
                    .padding(.horizontal, 8)
                    .frame(height: 32)
                    .background(flavour(.surface0), in: .rect(cornerRadius: 6))
                    .foregroundStyle(flavour(.text))
                }
                .accessibilityLabel("mode")
                .accessibilityValue(input.mode.label)
                cap(icon: "square.and.pencil", name: "editor") { send(.editor) }
                cap(icon: "keyboard.chevron.compact.down", name: "hide") { send(.hide) }
            }
            .padding(.trailing, 4)
        }
        .frame(height: Self.height)
        .frame(maxWidth: .infinity)
        .background(flavour(.mantle))
    }

    private static let names: [Character: String] = ["~": "tilde", "|": "pipe", "/": "slash", "-": "dash"]

    private var separator: some View {
        Rectangle().fill(flavour(.surface1)).frame(width: 1, height: 24)
    }

    private func modKey(_ mod: Mods) -> some View {
        let title = mod == .rightClick ? "rclick" : mod.label
        let held = input.locked.contains(mod) ? flavour(.peach)
            : input.armed.contains(mod) ? flavour(.mauve) : nil
        return Button { input.tap(mod) } label: {
            label(Text(title).font(.mono(13)), background: held ?? flavour(.surface0),
                  foreground: held == nil ? flavour(.text) : flavour(.crust))
        }
        .accessibilityLabel(mod.label)
        .accessibilityValue(held == nil ? "off" : input.locked.contains(mod) ? "locked" : "armed")
    }

    private func cap(_ title: String, name: String? = nil, action: @escaping () -> Void) -> some View {
        Button(action: action) { label(Text(title).font(.mono(13))) }
            .accessibilityLabel(name ?? title)
    }

    private func cap(icon: String, name: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { label(Image(systemName: icon)) }
            .accessibilityLabel(name)
    }

    private func arrow(_ icon: String, _ name: String, _ code: UInt32) -> some View {
        RepeatKey(label: label(Image(systemName: icon))) { send(.key(code)) }
            .accessibilityLabel(name)
            .accessibilityAddTraits(.isButton)
    }

    private func label(
        _ content: some View, background: Color? = nil, foreground: Color? = nil
    ) -> some View {
        content
            .frame(minWidth: 30)
            .frame(height: 32)
            .padding(.horizontal, 4)
            .background(background ?? flavour(.surface0), in: .rect(cornerRadius: 6))
            .foregroundStyle(foreground ?? flavour(.text))
    }
}

/// Hold to repeat: SwiftUI has no repeating button, and a long press fires once.
private struct RepeatKey<Label: View>: View {
    let label: Label
    let action: () -> Void

    @State private var timer: Timer?

    var body: some View {
        label.gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { _ in
                    guard timer == nil else { return }
                    action()
                    timer = Timer.scheduledTimer(withTimeInterval: 0.12, repeats: true) { _ in
                        Task { @MainActor in action() }
                    }
                    timer?.fireDate = Date().addingTimeInterval(0.4)
                }
                .onEnded { _ in
                    timer?.invalidate()
                    timer = nil
                })
    }
}
