import AppKit
import CoreText
import GhosttyKit
import SwiftUI

extension Ghostty {
    /// libghostty for the Mac. Unlike the phone's, its surfaces run their own process: a
    /// shell here, or ssh to a Host.
    @MainActor
    final class MacApp {
        static let shared = MacApp()
        private(set) var app: ghostty_app_t?

        private init() {
            for url in Bundle.main.urls(forResourcesWithExtension: "ttf", subdirectory: "Fonts") ?? [] {
                CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
            }
            if let resources = Bundle.main.resourcePath { setenv("GHOSTTY_RESOURCES_DIR", resources + "/ghostty", 1) }
            guard ghostty_init(UInt(CommandLine.argc), CommandLine.unsafeArgv) == GHOSTTY_SUCCESS,
                  let config = Config.load() else { return }
            var runtime = ghostty_runtime_config_s(
                userdata: nil,
                supports_selection_clipboard: false,
                wakeup_cb: { _ in DispatchQueue.main.async { MainActor.assumeIsolated { MacApp.shared.tick() } } },
                action_cb: { app, target, action in MacApp.perform(app, target, action) },
                read_clipboard_cb: { userdata, _, state in
                    guard let userdata else { return }
                    let view = Unmanaged<TerminalSurface>.fromOpaque(userdata).takeUnretainedValue()
                    let text = NSPasteboard.general.string(forType: .string) ?? ""
                    DispatchQueue.main.async { MainActor.assumeIsolated { view.paste(text, state) } }
                },
                confirm_read_clipboard_cb: { _, _, _, _ in },
                write_clipboard_cb: { _, _, contents, count, _ in
                    guard let contents else { return }
                    for index in 0..<count where String(cString: contents[index].mime) == "text/plain" {
                        let text = String(cString: contents[index].data)
                        DispatchQueue.main.async {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(text, forType: .string)
                        }
                    }
                },
                close_surface_cb: { userdata, _ in
                    guard let userdata else { return }
                    let view = Unmanaged<TerminalSurface>.fromOpaque(userdata).takeUnretainedValue()
                    DispatchQueue.main.async { MainActor.assumeIsolated { view.onClose?() } }
                })
            app = ghostty_app_new(&runtime, config)
        }

        func tick() { if let app { ghostty_app_tick(app) } }

        /// The theme follows dark mode only if the config is reloaded when ghostty asks.
        nonisolated private static func perform(_ app: ghostty_app_t?, _ target: ghostty_target_s, _ action: ghostty_action_s) -> Bool {
            guard action.tag == GHOSTTY_ACTION_RELOAD_CONFIG, let config = Config.load() else { return false }
            defer { ghostty_config_free(config) }
            switch target.tag {
            case GHOSTTY_TARGET_APP:
                guard let app else { return false }
                ghostty_app_update_config(app, config)
            case GHOSTTY_TARGET_SURFACE: ghostty_surface_update_config(target.target.surface, config)
            default: return false
            }
            return true
        }
    }

    /// One terminal running `command` in `folder`, or the login shell when there is none.
    @MainActor
    final class TerminalSurface: NSView {
        private var surface: ghostty_surface_t?
        var onClose: (() -> Void)?

        init(command: String?, folder: String?) {
            super.init(frame: NSRect(x: 0, y: 0, width: 800, height: 600))
            wantsLayer = true
            guard let app = MacApp.shared.app else { return }
            var config = ghostty_surface_config_new()
            config.platform_tag = GHOSTTY_PLATFORM_MACOS
            config.platform = ghostty_platform_u(macos: ghostty_platform_macos_s(nsview: Unmanaged.passUnretained(self).toOpaque()))
            config.userdata = Unmanaged.passUnretained(self).toOpaque()
            config.scale_factor = Double(NSScreen.main?.backingScaleFactor ?? 2)
            config.font_size = Self.fontSize
            let command = command.flatMap { strdup($0) }, folder = folder.flatMap { strdup($0) }
            defer { free(command); free(folder) }
            config.command = UnsafePointer(command)
            config.working_directory = UnsafePointer(folder)
            surface = ghostty_surface_new(app, &config)
        }

        required init?(coder: NSCoder) { fatalError("unsupported") }

        /// The size the owner set for Ghostty itself, so Sesh's terminals read the same.
        private static let fontSize: Float = {
            let home = FileManager.default.homeDirectoryForCurrentUser
            let files = [".config/ghostty/config", "Library/Application Support/com.mitchellh.ghostty/config"]
            for file in files {
                guard let text = try? String(contentsOf: home.appending(path: file), encoding: .utf8) else { continue }
                for line in text.split(separator: "\n") {
                    let parts = line.split(separator: "=", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
                    if parts.count == 2, parts[0] == "font-size", let size = Float(parts[1]) { return size }
                }
            }
            return 13
        }()

        deinit {
            if let surface { ghostty_surface_free(surface) }
        }

        fileprivate func paste(_ text: String, _ state: UnsafeMutableRawPointer?) {
            guard let surface else { return }
            ghostty_surface_complete_clipboard_request(surface, text, state, false)
        }

        // MARK: Size and focus

        override var acceptsFirstResponder: Bool { true }

        override func becomeFirstResponder() -> Bool {
            if let surface { ghostty_surface_set_focus(surface, true) }
            return true
        }

        override func resignFirstResponder() -> Bool {
            if let surface { ghostty_surface_set_focus(surface, false) }
            return true
        }

        override func layout() {
            super.layout()
            resize()
        }

        override func viewDidChangeEffectiveAppearance() {
            super.viewDidChangeEffectiveAppearance()
            resize()
        }

        override func viewDidChangeBackingProperties() {
            super.viewDidChangeBackingProperties()
            resize()
        }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let surface { ghostty_surface_set_occlusion(surface, window != nil) }
            resize()
            window?.makeFirstResponder(self)
        }

        private func resize() {
            guard let surface else { return }
            let scale = window?.backingScaleFactor ?? 2
            let pixels = convertToBacking(bounds.size)
            ghostty_surface_set_content_scale(surface, scale, scale)
            ghostty_surface_set_size(surface, UInt32(pixels.width), UInt32(pixels.height))
            let dark = effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            let scheme = dark ? GHOSTTY_COLOR_SCHEME_DARK : GHOSTTY_COLOR_SCHEME_LIGHT
            ghostty_app_set_color_scheme(ghostty_surface_app(surface), scheme)
            ghostty_surface_set_color_scheme(surface, scheme)
        }

        // MARK: Keys

        private static func mods(_ flags: NSEvent.ModifierFlags) -> ghostty_input_mods_e {
            var mods = GHOSTTY_MODS_NONE.rawValue
            if flags.contains(.shift) { mods |= GHOSTTY_MODS_SHIFT.rawValue }
            if flags.contains(.control) { mods |= GHOSTTY_MODS_CTRL.rawValue }
            if flags.contains(.option) { mods |= GHOSTTY_MODS_ALT.rawValue }
            if flags.contains(.command) { mods |= GHOSTTY_MODS_SUPER.rawValue }
            if flags.contains(.capsLock) { mods |= GHOSTTY_MODS_CAPS.rawValue }
            return ghostty_input_mods_e(rawValue: mods)
        }

        private func send(_ event: NSEvent, _ action: ghostty_input_action_e) -> Bool {
            guard let surface else { return false }
            var key = ghostty_input_key_s()
            key.action = action
            key.mods = Self.mods(event.modifierFlags)
            key.keycode = UInt32(event.keyCode)
            key.unshifted_codepoint = event.charactersIgnoringModifiers?.unicodeScalars.first?.value ?? 0
            // Function keys arrive as private-use characters and control keys as control
            // characters; ghostty encodes both from the keycode instead.
            let text = action == GHOSTTY_ACTION_RELEASE ? nil : event.characters.flatMap { characters -> String? in
                guard let first = characters.unicodeScalars.first, first.value >= 0x20,
                      !(0xF700...0xF8FF).contains(first.value) else { return nil }
                return characters
            }
            return (text ?? "").withCString { pointer in
                key.text = text == nil ? nil : pointer
                return ghostty_surface_key(surface, key)
            }
        }

        override func keyDown(with event: NSEvent) {
            _ = send(event, event.isARepeat ? GHOSTTY_ACTION_REPEAT : GHOSTTY_ACTION_PRESS)
        }

        override func keyUp(with event: NSEvent) { _ = send(event, GHOSTTY_ACTION_RELEASE) }

        /// Uploads a pasted image and returns its path, which is then typed in.
        var pasteImage: ((Data, String) async -> String?)?

        func type(_ text: String) {
            guard let surface else { return }
            ghostty_surface_text(surface, text, UInt(text.utf8.count))
        }

        /// Command keys go to ghostty's own bindings first: copy, paste, font size. An image
        /// cannot be pasted into a terminal, so it is uploaded and its path pasted instead.
        override func performKeyEquivalent(with event: NSEvent) -> Bool {
            guard window?.firstResponder === self, event.modifierFlags.contains(.command) else { return false }
            if event.charactersIgnoringModifiers == "v", let pasteImage, let (data, ext) = PastedImage.read(.general) {
                Task { @MainActor in
                    if let path = await pasteImage(data, ext) { self.type(quote(path)) }
                }
                return true
            }
            return send(event, GHOSTTY_ACTION_PRESS)
        }

        // MARK: Mouse

        private func position(_ event: NSEvent) {
            guard let surface else { return }
            let point = convert(event.locationInWindow, from: nil)
            ghostty_surface_mouse_pos(surface, point.x, bounds.height - point.y, Self.mods(event.modifierFlags))
        }

        private func button(_ event: NSEvent, _ state: ghostty_input_mouse_state_e, _ button: ghostty_input_mouse_button_e) {
            guard let surface else { return }
            position(event)
            _ = ghostty_surface_mouse_button(surface, state, button, Self.mods(event.modifierFlags))
        }

        override func mouseDown(with event: NSEvent) {
            window?.makeFirstResponder(self)
            button(event, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_LEFT)
        }

        override func mouseUp(with event: NSEvent) { button(event, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_LEFT) }
        override func rightMouseDown(with event: NSEvent) { button(event, GHOSTTY_MOUSE_PRESS, GHOSTTY_MOUSE_RIGHT) }
        override func rightMouseUp(with event: NSEvent) { button(event, GHOSTTY_MOUSE_RELEASE, GHOSTTY_MOUSE_RIGHT) }
        override func mouseDragged(with event: NSEvent) { position(event) }
        override func mouseMoved(with event: NSEvent) { position(event) }

        override func updateTrackingAreas() {
            trackingAreas.forEach(removeTrackingArea)
            addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .activeInKeyWindow, .inVisibleRect], owner: self))
            super.updateTrackingAreas()
        }

        override func scrollWheel(with event: NSEvent) {
            guard let surface else { return }
            // Bit 0 says the deltas are precise, as from a trackpad.
            let precise: ghostty_input_scroll_mods_t = event.hasPreciseScrollingDeltas ? 1 : 0
            ghostty_surface_mouse_scroll(surface, event.scrollingDeltaX, event.scrollingDeltaY, precise)
        }
    }

    struct Terminal: NSViewRepresentable {
        let view: TerminalSurface

        func makeNSView(context: Context) -> TerminalSurface { view }
        func updateNSView(_ view: TerminalSurface, context: Context) {}
    }
}
