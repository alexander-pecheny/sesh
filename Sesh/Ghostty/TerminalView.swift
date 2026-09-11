import SwiftUI
import UIKit
import GhosttyKit

extension Ghostty {
    /// A libghostty surface in manual I/O mode: `onWrite` receives what the terminal wants
    /// sent to the remote, `process(_:)` pushes remote output back in.
    final class TerminalView: UIView, UIKeyInput, UIGestureRecognizerDelegate {
        private var surface: ghostty_surface_t?
        private var grid: (UInt16, UInt16) = (0, 0)
        private var writes = 0
        private var writesAtPress = 0
        private var shiftBypass = false
        private var panAnchor = CGPoint.zero
        private var touchStart = CGPoint.zero
        private var pinchBase = 0.0
        private var fontSize: Double

        let input: InputState
        var onWrite: ((Data) -> Void)?
        var onResize: ((UInt16, UInt16) -> Void)?
        var onSelection: ((CGPoint?) -> Void)?
        var onEditor: (() -> Void)?
        var onTitle: ((String) -> Void)?

        var gridSize: (UInt16, UInt16) { grid }

        init(app: ghostty_app_t, input: InputState, fontSize: Double) {
            self.input = input
            self.fontSize = fontSize
            super.init(frame: CGRect(x: 0, y: 0, width: 800, height: 600))

            var config = ghostty_surface_config_new()
            config.platform_tag = GHOSTTY_PLATFORM_IOS
            config.platform = ghostty_platform_u(ios: ghostty_platform_ios_s(
                uiview: Unmanaged.passUnretained(self).toOpaque()))
            config.userdata = Unmanaged.passUnretained(self).toOpaque()
            config.scale_factor = UITraitCollection.current.displayScale
            config.font_size = Float(fontSize)
            config.io_mode = GHOSTTY_SURFACE_IO_MANUAL
            config.io_write_userdata = Unmanaged.passUnretained(self).toOpaque()
            config.io_write_cb = { userdata, bytes, len in
                guard let userdata, let bytes, len > 0 else { return }
                let view = Unmanaged<TerminalView>.fromOpaque(userdata).takeUnretainedValue()
                let data = Data(bytes: bytes, count: Int(len))
                DispatchQueue.main.async { view.wrote(data) }
            }

            surface = ghostty_surface_new(app, &config)
            if surface == nil { logger.critical("ghostty_surface_new failed") }
            isAccessibilityElement = true
            accessibilityLabel = "Terminal"
            accessibilityIdentifier = "terminal"

            addGestures()
        }

        required init?(coder: NSCoder) { fatalError("unsupported") }

        deinit {
            guard let surface else { return }
            ghostty_surface_free(surface)
        }

        func process(_ data: Data) {
            guard let surface, !data.isEmpty else { return }
            data.withUnsafeBytes { buffer in
                ghostty_surface_process_output(
                    surface,
                    buffer.baseAddress!.assumingMemoryBound(to: CChar.self),
                    UInt(buffer.count))
            }
        }

        private func wrote(_ data: Data) {
            writes += 1
            onWrite?(data)
        }

        // MARK: UIView

        override func layoutSubviews() {
            super.layoutSubviews()
            guard let surface else { return }
            let scale = window?.screen.scale ?? traitCollection.displayScale

            // libghostty renders into a sublayer it adds to us but never sizes; it reads
            // that layer's bounds and contentsScale back as its own drawable size.
            layer.sublayers?.forEach {
                $0.frame = layer.bounds
                $0.contentsScale = scale
            }

            updateColorScheme()
            ghostty_surface_set_content_scale(surface, scale, scale)
            ghostty_surface_set_size(
                surface,
                UInt32(bounds.width * scale),
                UInt32(bounds.height * scale))

            let size = ghostty_surface_size(surface)
            guard size.columns > 0, size.rows > 0, (size.columns, size.rows) != grid else { return }
            grid = (size.columns, size.rows)
            onResize?(size.columns, size.rows)
        }

        // A Tab that is not the visible one keeps its Session and its VT state; only the
        // renderer is told to stand down.
        override func didMoveToWindow() {
            super.didMoveToWindow()
            updateColorScheme()
            if let surface { ghostty_surface_set_occlusion(surface, window != nil) }
            if window != nil { focus() }
        }

        override func traitCollectionDidChange(_ previous: UITraitCollection?) {
            super.traitCollectionDidChange(previous)
            updateColorScheme()
        }

        private func updateColorScheme() {
            guard let surface else { return }
            let scheme = traitCollection.userInterfaceStyle == .dark
                ? GHOSTTY_COLOR_SCHEME_DARK : GHOSTTY_COLOR_SCHEME_LIGHT
            ghostty_app_set_color_scheme(ghostty_surface_app(surface), scheme)
            ghostty_surface_set_color_scheme(surface, scheme)
        }

        // MARK: Focus and the keys row

        override var canBecomeFirstResponder: Bool { true }

        override var inputAccessoryView: UIView? { accessory }

        private lazy var accessory: UIView = {
            let row = KeysRow(input: input) { [weak self] in self?.perform($0) }
            let host = UIHostingController(rootView: row)
            keysRow = host
            host.view.backgroundColor = .clear
            host.view.frame = CGRect(x: 0, y: 0, width: 0, height: KeysRow.height)
            host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            let container = UIView(frame: host.view.frame)
            container.autoresizingMask = .flexibleWidth
            container.addSubview(host.view)
            return container
        }()

        private var keysRow: UIViewController?

        @objc @discardableResult private func focus() -> Bool { becomeFirstResponder() }

        override func becomeFirstResponder() -> Bool {
            let became = super.becomeFirstResponder()
            if let surface, became { ghostty_surface_set_focus(surface, true) }
            return became
        }

        override func resignFirstResponder() -> Bool {
            let resigned = super.resignFirstResponder()
            if let surface, resigned { ghostty_surface_set_focus(surface, false) }
            return resigned
        }

        private func perform(_ action: KeysRow.Action) {
            switch action {
            case .key(let code): send(keycode: code, mods: input.consume())
            case .character(let character): send(character: character, extra: input.consume())
            case .paste: paste(UIPasteboard.general.string ?? "")
            case .editor: onEditor?()
            case .hide: _ = resignFirstResponder()
            }
        }

        // MARK: Keys

        func paste(_ text: String) {
            guard let surface, !text.isEmpty else { return }
            text.withCString { ghostty_surface_text(surface, $0, UInt(text.utf8.count)) }
        }

        func send(keycode: UInt32, mods: Mods, text: String? = nil, unshifted: UInt32 = 0) {
            guard let surface else { return }
            var event = ghostty_input_key_s()
            event.keycode = keycode
            event.mods = mods.ghostty
            event.consumed_mods = mods.subtracting([.ctrl, .cmd]).ghostty
            event.unshifted_codepoint = unshifted
            event.action = GHOSTTY_ACTION_PRESS
            if let text, mods.isDisjoint(with: [.ctrl, .cmd]) {
                text.withCString {
                    event.text = $0
                    _ = ghostty_surface_key(surface, event)
                }
                event.text = nil
            } else {
                _ = ghostty_surface_key(surface, event)
            }
            event.action = GHOSTTY_ACTION_RELEASE
            _ = ghostty_surface_key(surface, event)
        }

        private func send(character: Character, extra: Mods) {
            guard let stroke = Keycode.stroke(character) else {
                paste(String(character))
                return
            }
            send(
                keycode: stroke.code,
                mods: stroke.mods.union(extra),
                text: String(character),
                unshifted: stroke.unshifted)
        }

        // MARK: UIKeyInput

        var hasText: Bool { true }

        // Without these iOS turns a shell's quotes and dashes into typographic ones.
        var autocorrectionType = UITextAutocorrectionType.no
        var autocapitalizationType = UITextAutocapitalizationType.none
        var spellCheckingType = UITextSpellCheckingType.no
        var smartQuotesType = UITextSmartQuotesType.no
        var smartDashesType = UITextSmartDashesType.no
        var smartInsertDeleteType = UITextSmartInsertDeleteType.no

        func insertText(_ text: String) {
            let mods = input.consume()
            if text == "\n" {
                send(keycode: Keycode.enter, mods: mods)
            } else if mods.isDisjoint(with: [.ctrl, .alt, .cmd]) {
                paste(text)
            } else {
                text.forEach { send(character: $0, extra: mods) }
            }
        }

        func deleteBackward() { send(keycode: Keycode.backspace, mods: input.consume()) }

        override func pressesBegan(_ presses: Set<UIPress>, with event: UIPressesEvent?) {
            let unhandled = presses.filter { !handle($0) }
            if !unhandled.isEmpty { super.pressesBegan(unhandled, with: event) }
        }

        /// Plain characters are left to `insertText`; only named keys and real modifier
        /// combos are ours, or every hardware keystroke would arrive twice.
        private func handle(_ press: UIPress) -> Bool {
            guard let key = press.key else { return false }
            let mods = Mods(key.modifierFlags).union(input.consume())
            if let code = Keycode.named(key.keyCode) {
                send(keycode: code, mods: mods)
                return true
            }
            guard !mods.isDisjoint(with: [.ctrl, .alt, .cmd]),
                  let character = key.charactersIgnoringModifiers.first,
                  let stroke = Keycode.stroke(character) else { return false }
            send(keycode: stroke.code, mods: stroke.mods.union(mods), unshifted: stroke.unshifted)
            return true
        }

        // MARK: Touch

        private func addGestures() {
            let tap = UITapGestureRecognizer(target: self, action: #selector(onTap))
            let pan = UIPanGestureRecognizer(target: self, action: #selector(onPan))
            pan.maximumNumberOfTouches = 1
            let twoFinger = UIPanGestureRecognizer(target: self, action: #selector(onScroll))
            twoFinger.minimumNumberOfTouches = 2
            let pinch = UIPinchGestureRecognizer(target: self, action: #selector(onPinch))
            for recogniser in [tap, pan, twoFinger, pinch] as [UIGestureRecognizer] {
                recogniser.delegate = self
                addGestureRecognizer(recogniser)
            }
        }

        override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
            super.touchesBegan(touches, with: event)
            if let touch = touches.first { touchStart = touch.location(in: self) }
        }

        func gestureRecognizer(
            _ recogniser: UIGestureRecognizer,
            shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
        ) -> Bool { true }

        private var selectMods: Mods { shiftBypass ? .shift : [] }

        @objc private func onTap(_ recogniser: UITapGestureRecognizer) {
            focus()
            let point = recogniser.location(in: self)
            switch input.mode {
            case .click:
                let mods = input.consume()
                click(mods.contains(.rightClick) ? GHOSTTY_MOUSE_RIGHT : GHOSTTY_MOUSE_LEFT,
                      at: point, mods: mods.subtracting(.rightClick))
            case .select:
                click(GHOSTTY_MOUSE_LEFT, at: point, mods: selectMods)
                onSelection?(nil)
            }
        }

        @objc private func onPan(_ recogniser: UIPanGestureRecognizer) {
            guard input.mode == .select else {
                onScroll(recogniser)
                return
            }
            guard let surface else { return }
            let mods = selectMods.ghostty
            var point = recogniser.location(in: self)
            switch recogniser.state {
            case .began:
                point = touchStart
                writesAtPress = writes
                ghostty_surface_mouse_pos(surface, point.x, point.y, mods)
                _ = ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_LEFT, mods)
            case .changed:
                ghostty_surface_mouse_pos(surface, point.x, point.y, mods)
            default:
                _ = ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_LEFT, mods)
                if writes != writesAtPress { shiftBypass = true }
                onSelection?(selection()?.anchor)
            }
        }

        @objc private func onScroll(_ recogniser: UIPanGestureRecognizer) {
            guard let surface else { return }
            let translation = recogniser.translation(in: self)
            if recogniser.state == .began { panAnchor = translation }
            let delta = CGPoint(x: translation.x - panAnchor.x, y: translation.y - panAnchor.y)
            panAnchor = translation
            guard recogniser.state == .changed else { return }
            let point = recogniser.location(in: self)
            ghostty_surface_mouse_pos(surface, point.x, point.y, GHOSTTY_MODS_NONE)
            ghostty_surface_mouse_scroll(surface, delta.x, delta.y, 1)
        }

        @objc private func onPinch(_ recogniser: UIPinchGestureRecognizer) {
            guard let surface else { return }
            if recogniser.state == .began { pinchBase = fontSize }
            let size = min(max(pinchBase * Double(recogniser.scale), 6), 48)
            guard abs(size - fontSize) >= 0.5 else { return }
            fontSize = size
            let action = "set_font_size:\(String(format: "%.1f", size))"
            _ = ghostty_surface_binding_action(surface, action, UInt(action.utf8.count))
        }

        private func click(
            _ button: ghostty_input_mouse_button_e, at point: CGPoint, mods: Mods
        ) {
            guard let surface else { return }
            ghostty_surface_mouse_pos(surface, point.x, point.y, mods.ghostty)
            _ = ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_PRESS, button, mods.ghostty)
            _ = ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_RELEASE, button, mods.ghostty)
        }

        func selection() -> (text: String, anchor: CGPoint)? {
            guard let surface, ghostty_surface_has_selection(surface) else { return nil }
            var text = ghostty_text_s()
            guard ghostty_surface_read_selection(surface, &text) else { return nil }
            defer { withUnsafeMutablePointer(to: &text) { sesh_ghostty_free_text($0) } }
            guard let bytes = text.text else { return nil }
            return (String(cString: bytes), CGPoint(x: text.tl_px_x, y: text.tl_px_y))
        }
    }
}
