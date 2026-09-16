import SwiftUI

struct KeysRow: View {
    enum Action {
        case key(UInt32)
        case shifted(UInt32)
        case character(Character)
        case paste, editor, upload
    }

    static let row: CGFloat = 40
    static let height: CGFloat = row * 2

    @ObservedObject var input: InputState
    @Environment(\.colorScheme) private var colorScheme
    /// How far the Upload in flight has got, or `nil` when none is.
    let uploading: Double?
    let canUpload: Bool
    let send: (Action) -> Void

    private var flavour: Catppuccin.Flavour { colorScheme == .dark ? .mocha : .latte }

    var body: some View {
        VStack(spacing: 0) {
            row {
                modes
                cap(icon: "clipboard-paste", name: "paste") { send(.paste) }
                upload
                cap(icon: "square-pen", name: "editor") { send(.editor) }
                cap(icon: "corner-down-left", name: "enter") { send(.key(Keycode.enter)) }
                cap(icon: "text-wrap", name: "shift enter") { send(.shifted(Keycode.enter)) }
                modKey(.rightClick) { Image.lucide("mouse", size: 16) }
                separator
                arrow("chevron-left", "left", Keycode.left)
                arrow("chevron-up", "up", Keycode.up)
                arrow("chevron-down", "down", Keycode.down)
                arrow("chevron-right", "right", Keycode.right)
            }
            row {
                arrow("arrow-left-to-line", "home", Keycode.home)
                arrow("arrow-up-to-line", "pgup", Keycode.pageUp)
                arrow("arrow-down-to-line", "pgdn", Keycode.pageDown)
                arrow("arrow-right-to-line", "end", Keycode.end)
                separator
                ForEach(Mods.keyboard, id: \.rawValue) { mod in
                    modKey(mod) { Text(mod.label).font(.ui(12)) }
                }
                cap("esc") { send(.key(Keycode.escape)) }
                cap("tab") { send(.key(Keycode.tab)) }
                separator
                ForEach(Array("~|/-"), id: \.self) { character in
                    cap(String(character), name: Self.names[character]) {
                        send(.character(character))
                    }
                }
            }
        }
        .frame(height: Self.height)
        .frame(maxWidth: .infinity)
        .background(flavour(.mantle))
    }

    private func row(@ViewBuilder content: () -> some View) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 3) { content() }
                .padding(.horizontal, 5)
                .fixedSize(horizontal: true, vertical: false)
        }
        .frame(height: Self.row)
    }

    /// The button is the progress ring, so while it fills the only thing it can do is stop.
    private var upload: some View {
        Button { send(.upload) } label: {
            label(Group {
                if let uploading {
                    UploadRing(fraction: uploading, colour: flavour(.mauve))
                } else {
                    Image.lucide("image-up", size: 16)
                }
            }, width: 22)
        }
        .disabled(uploading == nil && !canUpload)
        .accessibilityLabel(uploading == nil ? "upload" : "cancel upload")
        .accessibilityValue(uploading.map { "\(Int($0 * 100))%" } ?? "")
    }

    private var modes: some View {
        HStack(spacing: 1) {
            ForEach(TouchMode.allCases) { mode in
                Button { input.mode = mode } label: {
                    Image.lucide(mode.icon, size: 15)
                        .frame(width: 30, height: 28)
                        .background(mode == input.mode ? flavour(.mauve) : flavour(.surface0))
                        .foregroundStyle(mode == input.mode ? flavour(.crust) : flavour(.text))
                }
                .accessibilityLabel(mode.label)
            }
        }
        .clipShape(.rect(cornerRadius: 6))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("mode")
        .accessibilityValue(input.mode.label)
    }

    private static let names: [Character: String] = ["~": "tilde", "|": "pipe", "/": "slash", "-": "dash"]

    private var separator: some View {
        Rectangle().fill(flavour(.surface1)).frame(width: 1, height: 24)
    }

    private func modKey(_ mod: Mods, @ViewBuilder title: () -> some View) -> some View {
        let held = input.locked.contains(mod) ? flavour(.peach)
            : input.armed.contains(mod) ? flavour(.mauve) : nil
        return Button { input.tap(mod) } label: {
            label(title(), background: held ?? flavour(.surface0),
                  foreground: held == nil ? flavour(.text) : flavour(.crust))
        }
        .accessibilityLabel(mod.label)
        .accessibilityValue(held == nil ? "off" : input.locked.contains(mod) ? "locked" : "armed")
    }

    private func cap(_ title: String, name: String? = nil, action: @escaping () -> Void) -> some View {
        Button(action: action) { label(Text(title).font(.ui(12))) }
            .accessibilityLabel(name ?? title)
    }

    private func cap(icon: String, name: String, action: @escaping () -> Void) -> some View {
        Button(action: action) { label(Image.lucide(icon, size: 16), width: 22) }
            .accessibilityLabel(name)
    }

    private func arrow(_ icon: String, _ name: String, _ code: UInt32) -> some View {
        Button { send(.key(code)) } label: { label(Image.lucide(icon, size: 16)) }
            .accessibilityLabel(name)
    }

    private func label(
        _ content: some View, background: Color? = nil, foreground: Color? = nil,
        width: CGFloat = 26
    ) -> some View {
        content
            .frame(minWidth: width)
            .frame(height: 32)
            .padding(.horizontal, 3)
            .background(background ?? flavour(.surface0), in: .rect(cornerRadius: 6))
            .foregroundStyle(foreground ?? flavour(.text))
    }
}

/// Hold to repeat: SwiftUI has no repeating button, and a long press fires once.

extension View {
    /// Attached on both the Tab and the Draft: an alert on a view under a sheet never
    /// appears over it.
    func uploadFailure(_ message: Binding<String?>) -> some View {
        alert("Upload failed", isPresented: .init(
            get: { message.wrappedValue != nil },
            set: { if !$0 { message.wrappedValue = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(message.wrappedValue ?? "")
        }
    }
}
