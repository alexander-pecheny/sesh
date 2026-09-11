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
                action_cb: { _, _, _ in false },
                read_clipboard_cb: { _, _, _ in },
                confirm_read_clipboard_cb: { _, _, _, _ in },
                write_clipboard_cb: { _, _, _, _, _ in },
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

        func tick() {
            guard let app else { return }
            ghostty_app_tick(app)
        }

        func setColorScheme(_ scheme: ghostty_color_scheme_e) {
            guard let app else { return }
            ghostty_app_set_color_scheme(app, scheme)
        }

        private static func registerBundledFonts() {
            for url in Bundle.main.urls(forResourcesWithExtension: "ttf", subdirectory: "Fonts") ?? [] {
                CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
            }
        }
    }
}
