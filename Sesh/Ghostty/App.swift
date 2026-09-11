import CoreText
import OSLog
import SwiftUI
import GhosttyKit

enum Ghostty {
    static let logger = Logger(subsystem: "me.pecheny.sesh", category: "ghostty")
}

extension Ghostty {
    final class App: ObservableObject {
        private(set) var app: ghostty_app_t?
        private var config: ghostty_config_t?

        init() {
            Self.registerBundledFonts()
            if let resources = Bundle.main.resourcePath {
                setenv("GHOSTTY_RESOURCES_DIR", resources + "/ghostty", 1)
            }

            guard ghostty_init(UInt(CommandLine.argc), CommandLine.unsafeArgv) == GHOSTTY_SUCCESS else {
                logger.critical("ghostty_init failed")
                return
            }
            guard let config = Config.load() else { return }
            self.config = config

            var runtime = ghostty_runtime_config_s(
                userdata: Unmanaged.passUnretained(self).toOpaque(),
                supports_selection_clipboard: false,
                wakeup_cb: { userdata in
                    guard let userdata else { return }
                    let app = Unmanaged<App>.fromOpaque(userdata).takeUnretainedValue()
                    DispatchQueue.main.async { app.tick() }
                },
                action_cb: { app, target, action in App.perform(app, target, action) },
                read_clipboard_cb: { _, _, _ in },
                confirm_read_clipboard_cb: { _, _, _, _ in },
                write_clipboard_cb: { _, _, contents, count, _ in App.writeClipboard(contents, count) },
                close_surface_cb: { _, _ in })

            guard let app = ghostty_app_new(&runtime, config) else {
                logger.critical("ghostty_app_new failed")
                return
            }
            self.app = app
            ghostty_app_set_focus(app, true)
        }

        deinit {
            if let app { ghostty_app_free(app) }
            if let config { ghostty_config_free(config) }
        }

        // libghostty asks the embedder to reload when the conditional state (dark/light)
        // changes; without this the theme never leaves its default flavour.
        private static func perform(
            _ app: ghostty_app_t?,
            _ target: ghostty_target_s,
            _ action: ghostty_action_s
        ) -> Bool {
            if action.tag == GHOSTTY_ACTION_SET_TITLE {
                return retitle(target, action.action.set_title.title)
            }
            guard action.tag == GHOSTTY_ACTION_RELOAD_CONFIG, let config = Config.load() else {
                return false
            }
            defer { ghostty_config_free(config) }
            switch target.tag {
            case GHOSTTY_TARGET_APP:
                guard let app else { return false }
                ghostty_app_update_config(app, config)
            case GHOSTTY_TARGET_SURFACE:
                ghostty_surface_update_config(target.target.surface, config)
            default:
                return false
            }
            return true
        }

        private static func retitle(_ target: ghostty_target_s, _ title: UnsafePointer<CChar>?) -> Bool {
            guard target.tag == GHOSTTY_TARGET_SURFACE, let title,
                  let userdata = ghostty_surface_userdata(target.target.surface) else { return false }
            let view = Unmanaged<TerminalView>.fromOpaque(userdata).takeUnretainedValue()
            let text = String(cString: title)
            DispatchQueue.main.async { view.onTitle?(text) }
            return true
        }

        func tick() {
            guard let app else { return }
            ghostty_app_tick(app)
        }

        private static func registerBundledFonts() {
            for url in Bundle.main.urls(forResourcesWithExtension: "ttf", subdirectory: "Fonts") ?? [] {
                CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
            }
        }

        // OSC 52 writes land here; the remote program can set the phone's clipboard.
        private static func writeClipboard(_ contents: UnsafePointer<ghostty_clipboard_content_s>?, _ count: Int) {
            guard let contents else { return }
            for i in 0..<count where String(cString: contents[i].mime) == "text/plain" {
                let text = String(cString: contents[i].data)
                DispatchQueue.main.async { UIPasteboard.general.string = text }
            }
        }
    }
}
