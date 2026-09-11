import UIKit
import GhosttyKit

extension Ghostty {
    /// A libghostty surface in manual I/O mode: `onWrite` receives what the terminal wants
    /// sent to the remote, `process(_:)` pushes remote output back in.
    final class TerminalView: UIView, UIKeyInput {
        private static let enterKeycode: UInt32 = 0x24
        private static let backspaceKeycode: UInt32 = 0x33

        private var surface: ghostty_surface_t?
        var onWrite: ((Data) -> Void)?

        override class var layerClass: AnyClass { CAMetalLayer.self }

        init(app: ghostty_app_t) {
            super.init(frame: CGRect(x: 0, y: 0, width: 800, height: 600))

            var config = ghostty_surface_config_new()
            config.platform_tag = GHOSTTY_PLATFORM_IOS
            config.platform = ghostty_platform_u(ios: ghostty_platform_ios_s(
                uiview: Unmanaged.passUnretained(self).toOpaque()))
            config.userdata = Unmanaged.passUnretained(self).toOpaque()
            config.scale_factor = contentScaleFactor
            config.io_mode = GHOSTTY_SURFACE_IO_MANUAL
            config.io_write_userdata = Unmanaged.passUnretained(self).toOpaque()
            config.io_write_cb = { userdata, bytes, len in
                guard let userdata, let bytes, len > 0 else { return }
                let view = Unmanaged<TerminalView>.fromOpaque(userdata).takeUnretainedValue()
                let data = Data(bytes: bytes, count: Int(len))
                DispatchQueue.main.async { view.onWrite?(data) }
            }

            surface = ghostty_surface_new(app, &config)
            if surface == nil { logger.critical("ghostty_surface_new failed") }

            addGestureRecognizer(UITapGestureRecognizer(target: self, action: #selector(focus)))
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

        // MARK: UIView

        override func layoutSubviews() {
            super.layoutSubviews()
            guard let surface else { return }
            let scale = window?.screen.scale ?? contentScaleFactor
            ghostty_surface_set_content_scale(surface, scale, scale)
            ghostty_surface_set_size(
                surface,
                UInt32(bounds.width * scale),
                UInt32(bounds.height * scale))
        }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            updateColorScheme()
            if window != nil { focus() }
        }

        override func traitCollectionDidChange(_ previous: UITraitCollection?) {
            super.traitCollectionDidChange(previous)
            updateColorScheme()
        }

        private func updateColorScheme() {
            guard let surface else { return }
            ghostty_surface_set_color_scheme(
                surface,
                traitCollection.userInterfaceStyle == .dark
                    ? GHOSTTY_COLOR_SCHEME_DARK : GHOSTTY_COLOR_SCHEME_LIGHT)
        }

        // MARK: Focus

        override var canBecomeFirstResponder: Bool { true }

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

        // MARK: UIKeyInput

        var hasText: Bool { true }

        func insertText(_ text: String) {
            guard let surface else { return }
            if text == "\n" {
                sendKey(Self.enterKeycode)
                return
            }
            let length = text.utf8.count
            text.withCString { ghostty_surface_text(surface, $0, UInt(length)) }
        }

        func deleteBackward() { sendKey(Self.backspaceKeycode) }

        private func sendKey(_ keycode: UInt32) {
            guard let surface else { return }
            var event = ghostty_input_key_s()
            event.mods = GHOSTTY_MODS_NONE
            event.consumed_mods = GHOSTTY_MODS_NONE
            event.keycode = keycode
            event.action = GHOSTTY_ACTION_PRESS
            _ = ghostty_surface_key(surface, event)
            event.action = GHOSTTY_ACTION_RELEASE
            _ = ghostty_surface_key(surface, event)
        }
    }
}
