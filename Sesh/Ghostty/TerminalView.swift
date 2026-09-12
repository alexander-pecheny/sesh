import SwiftUI
import UIKit
import GhosttyKit

extension Ghostty {
    /// A libghostty surface in manual I/O mode: `onWrite` receives what the terminal wants
    /// sent to the remote, `process(_:)` pushes remote output back in.
    final class TerminalView: UIView, UIKeyInput, UIGestureRecognizerDelegate, UIScrollViewDelegate {
        private var surface: ghostty_surface_t?
        private var grid: (UInt16, UInt16) = (0, 0)
        private let scroller = Scroller()
        private var scrollerOffset = 0.0
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
            input.onMode = { [weak self] in self?.modeChanged() }
            isAccessibilityElement = true
            accessibilityLabel = "Terminal"
            accessibilityIdentifier = "terminal"

            addGestures()
            addScroller()
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

        private func wrote(_ data: Data) { onWrite?(data) }

        // MARK: UIView

        override func layoutSubviews() {
            super.layoutSubviews()
            guard let surface else { return }
            let scale = window?.screen.scale ?? traitCollection.displayScale

            // libghostty renders into a sublayer it adds to us but never sizes; it reads
            // that layer's bounds and contentsScale back as its own drawable size.
            layer.sublayers?.forEach { sublayer in
                guard !(sublayer.delegate is UIView) else { return }
                sublayer.frame = layer.bounds
                sublayer.contentsScale = scale
            }
            scroller.frame = bounds
            recentreScroller()

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

        // Click and Select put the keyboard away without resigning, so hardware keys
        // still arrive.
        override var inputView: UIView? { input.mode.keyboard ? nil : blankKeyboard }

        private let blankKeyboard = UIView(frame: .zero)

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

        func perform(_ action: KeysRow.Action) {
            switch action {
            case .key(let code): send(keycode: code, mods: input.consume())
            case .character(let character): send(character: character, extra: input.consume())
            case .paste: paste(UIPasteboard.general.string ?? "")
            case .editor: onEditor?()
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
            let pinch = UIPinchGestureRecognizer(target: self, action: #selector(onPinch))
            for recogniser in [tap, pan, pinch] as [UIGestureRecognizer] {
                recogniser.delegate = self
                addGestureRecognizer(recogniser)
            }
        }

        // An invisible UIScrollView on top supplies native scrolling with inertia; its
        // offset changes become precision wheel events in pixels, which is what ghostty
        // measures against its cell height. One finger scrolls, except in Select mode
        // where one finger selects and two scroll.
        private final class Scroller: UIScrollView {
            var touchDown: ((CGPoint) -> Void)?

            override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
                if let touch = touches.first, let view = superview {
                    touchDown?(touch.location(in: view))
                }
                super.touchesBegan(touches, with: event)
            }
        }

        private static let scrollerSpan: CGFloat = 200_000

        private func addScroller() {
            scroller.delegate = self
            scroller.backgroundColor = .clear
            scroller.showsVerticalScrollIndicator = false
            scroller.showsHorizontalScrollIndicator = false
            scroller.contentInsetAdjustmentBehavior = .never
            scroller.contentSize = CGSize(width: 0, height: Self.scrollerSpan)
            scroller.touchDown = { [weak self] point in self?.touchStart = point }
            addSubview(scroller)
            modeChanged()
        }

        private func modeChanged() {
            scroller.panGestureRecognizer.minimumNumberOfTouches = input.mode == .select ? 2 : 1
            reloadInputViews()
        }

        private func recentreScroller() {
            scrollerOffset = (Self.scrollerSpan - bounds.height) / 2
            scroller.contentOffset = CGPoint(x: 0, y: scrollerOffset)
        }

        func scrollViewDidScroll(_ scrollView: UIScrollView) {
            guard let surface, scrollView.isDragging || scrollView.isDecelerating else { return }
            let dy = scrollView.contentOffset.y - scrollerOffset
            scrollerOffset = scrollView.contentOffset.y
            guard dy != 0 else { return }
            let scale = window?.screen.scale ?? traitCollection.displayScale
            let point = scroller.panGestureRecognizer.location(in: self)
            ghostty_surface_mouse_pos(surface, point.x, point.y, GHOSTTY_MODS_NONE)
            ghostty_surface_mouse_scroll(surface, 0, -dy * scale, 1)
        }

        func scrollViewDidEndDragging(_ scrollView: UIScrollView, willDecelerate decelerate: Bool) {
            if !decelerate { recentreScroller() }
        }

        func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) { recentreScroller() }

        func gestureRecognizer(
            _ recogniser: UIGestureRecognizer,
            shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
        ) -> Bool { true }

        @objc private func onTap(_ recogniser: UITapGestureRecognizer) {
            focus()
            let point = recogniser.location(in: self)
            guard input.mode == .select else {
                let mods = input.consume()
                click(mods.contains(.rightClick) ? GHOSTTY_MOUSE_RIGHT : GHOSTTY_MOUSE_LEFT,
                      at: point, mods: mods.subtracting(.rightClick))
                return
            }
            if let surface { _ = ghostty_surface_clear_selection(surface) }
            onSelection?(nil)
        }

        @objc private func onPan(_ recogniser: UIPanGestureRecognizer) {
            guard input.mode == .select, let surface else { return }
            // Shift keeps a select-mode drag local: ghostty never reports it to the program.
            let mods = Mods.shift.ghostty
            var point = recogniser.location(in: self)
            switch recogniser.state {
            case .began:
                point = touchStart
                ghostty_surface_mouse_pos(surface, point.x, point.y, mods)
                _ = ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_LEFT, mods)
            case .changed:
                ghostty_surface_mouse_pos(surface, point.x, point.y, mods)
            default:
                _ = ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_LEFT, mods)
                onSelection?(selection()?.anchor)
            }
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
