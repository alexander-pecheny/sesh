import UIKit
import GhosttyKit

struct Mods: OptionSet, Hashable {
    let rawValue: Int

    static let shift = Mods(rawValue: 1 << 0)
    static let ctrl = Mods(rawValue: 1 << 1)
    static let alt = Mods(rawValue: 1 << 2)
    static let cmd = Mods(rawValue: 1 << 3)
    static let rightClick = Mods(rawValue: 1 << 4)

    static let keyboard: [Mods] = [.ctrl, .alt, .shift, .cmd]

    init(rawValue: Int) { self.rawValue = rawValue }

    init(_ flags: UIKeyModifierFlags) {
        var mods = Mods()
        if flags.contains(.shift) { mods.insert(.shift) }
        if flags.contains(.control) { mods.insert(.ctrl) }
        if flags.contains(.alternate) { mods.insert(.alt) }
        if flags.contains(.command) { mods.insert(.cmd) }
        self = mods
    }

    var label: String {
        switch self {
        case .ctrl: "ctrl"
        case .alt: "alt"
        case .shift: "shift"
        case .cmd: "cmd"
        case .rightClick: "right-click"
        default: ""
        }
    }

    var ghostty: ghostty_input_mods_e {
        var value: UInt32 = 0
        if contains(.shift) { value |= GHOSTTY_MODS_SHIFT.rawValue }
        if contains(.ctrl) { value |= GHOSTTY_MODS_CTRL.rawValue }
        if contains(.alt) { value |= GHOSTTY_MODS_ALT.rawValue }
        if contains(.cmd) { value |= GHOSTTY_MODS_SUPER.rawValue }
        return ghostty_input_mods_e(value)
    }
}

/// macOS virtual keycodes, which is what `ghostty_input_key_s.keycode` wants.
enum Keycode {
    static let escape: UInt32 = 0x35
    static let tab: UInt32 = 0x30
    static let enter: UInt32 = 0x24
    static let backspace: UInt32 = 0x33
    static let left: UInt32 = 0x7b
    static let right: UInt32 = 0x7c
    static let down: UInt32 = 0x7d
    static let up: UInt32 = 0x7e
    static let pageUp: UInt32 = 0x74
    static let pageDown: UInt32 = 0x79
    static let home: UInt32 = 0x73
    static let end: UInt32 = 0x77

    struct Stroke {
        let code: UInt32
        let mods: Mods
        let unshifted: UInt32
    }

    static func stroke(_ character: Character) -> Stroke? {
        if let index = plain.firstIndex(of: character) {
            return Stroke(code: codes[index], mods: [], unshifted: scalar(character))
        }
        if let index = shifted.firstIndex(of: character) {
            return Stroke(code: codes[index], mods: .shift, unshifted: scalar(plain[index]))
        }
        return nil
    }

    static func named(_ usage: UIKeyboardHIDUsage) -> UInt32? {
        switch usage {
        case .keyboardEscape: escape
        case .keyboardTab: tab
        case .keyboardReturnOrEnter: enter
        case .keyboardDeleteOrBackspace: backspace
        case .keyboardLeftArrow: left
        case .keyboardRightArrow: right
        case .keyboardDownArrow: down
        case .keyboardUpArrow: up
        case .keyboardPageUp: pageUp
        case .keyboardPageDown: pageDown
        case .keyboardHome: home
        case .keyboardEnd: end
        default: nil
        }
    }

    private static func scalar(_ character: Character) -> UInt32 {
        character.unicodeScalars.first?.value ?? 0
    }

    private static let plain = Array(#"abcdefghijklmnopqrstuvwxyz1234567890-=[]\;'`,./ "#)
    private static let shifted = Array(#"ABCDEFGHIJKLMNOPQRSTUVWXYZ!@#$%^&*()_+{}|:"~<>? "#)
    private static let codes: [UInt32] = [
        0x00, 0x0b, 0x08, 0x02, 0x0e, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2e,
        0x2d, 0x1f, 0x23, 0x0c, 0x0f, 0x01, 0x11, 0x20, 0x09, 0x0d, 0x07, 0x10, 0x06,
        0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1a, 0x1c, 0x19, 0x1d,
        0x1b, 0x18, 0x21, 0x1e, 0x2a, 0x29, 0x27, 0x32, 0x2b, 0x2f, 0x2c, 0x31,
    ]
}

enum TouchMode: String, CaseIterable, Identifiable {
    case type, click, select

    var id: String { rawValue }
    var label: String { rawValue.capitalized }
    var keyboard: Bool { self == .type }

    var icon: String {
        switch self {
        case .type: "keyboard"
        case .click: "pointer"
        case .select: "square-dashed-mouse-pointer"
        }
    }
}

/// Armed and locked modifiers plus the Touch mode: the keys row writes it, the terminal
/// view reads it.
@MainActor
final class InputState: ObservableObject {
    @Published private(set) var armed: Mods = []
    @Published private(set) var locked: Mods = []
    @Published var mode: TouchMode {
        didSet {
            UserDefaults.standard.set(mode.rawValue, forKey: "touchMode")
            onMode?()
        }
    }

    /// The terminal view swaps its input view when the mode changes.
    var onMode: (() -> Void)?

    init() {
        let defaults = UserDefaults.standard
        let stored = defaults.string(forKey: "touchMode")
        // Two-mode installs called today's Type mode "click".
        if !defaults.bool(forKey: "touchModes3") {
            defaults.set(true, forKey: "touchModes3")
            mode = stored == "select" ? .select : .type
            defaults.set(mode.rawValue, forKey: "touchMode")
        } else {
            mode = stored.flatMap(TouchMode.init) ?? .type
        }
    }

    var active: Mods { armed.union(locked) }

    func tap(_ mod: Mods) {
        if locked.contains(mod) {
            locked.remove(mod)
        } else if armed.contains(mod) {
            armed.remove(mod)
            locked.insert(mod)
        } else {
            armed.insert(mod)
        }
    }

    func consume() -> Mods {
        let mods = active
        armed = []
        return mods
    }
}
