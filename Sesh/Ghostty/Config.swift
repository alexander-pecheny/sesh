import Foundation
import GhosttyKit

extension Ghostty {
    enum Config {
        static let source = """
            theme = light:Catppuccin Latte,dark:Catppuccin Mocha
            font-family = JetBrainsMono NF
            font-size = 12
            """

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
