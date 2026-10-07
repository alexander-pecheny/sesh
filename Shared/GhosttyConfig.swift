import Foundation
import GhosttyKit
import OSLog

enum Ghostty {
    static let logger = Logger(subsystem: "me.pecheny.sesh", category: "ghostty")
}

extension Ghostty {
    enum Config {
        /// The Mac sets the owner's own Ghostty size here, so a config reload keeps it.
        nonisolated(unsafe) static var fontSize: Float = 12

        static var source: String { """
            theme = light:Catppuccin Latte,dark:Catppuccin Mocha
            font-family = JetBrainsMono NF
            font-size = \(fontSize)
            clipboard-read = deny
            clipboard-write = allow
            """
        }

        // iOS has no XDG config dir, so we hand libghostty a file we write ourselves.
        static func load() -> ghostty_config_t? {
            guard let config = ghostty_config_new() else {
                logger.critical("ghostty_config_new failed")
                return nil
            }

            let path = FileManager.default.temporaryDirectory.appendingPathComponent("sesh.conf")
            do {
                try source.write(to: path, atomically: true, encoding: .utf8)
                ghostty_config_load_file(config, path.path)
            } catch {
                logger.error("writing \(path.path) failed: \(error.localizedDescription)")
            }
            ghostty_config_finalize(config)

            for i in 0..<ghostty_config_diagnostics_count(config) {
                let message = String(cString: ghostty_config_get_diagnostic(config, i).message)
                logger.warning("config: \(message)")
            }
            return config
        }
    }
}
