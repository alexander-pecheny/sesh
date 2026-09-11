import SwiftUI

enum Catppuccin {
    enum Swatch: Int {
        case rosewater, flamingo, pink, mauve, red, maroon, peach, yellow, green, teal,
             sky, sapphire, blue, lavender, text, subtext1, subtext0, overlay2, overlay1,
             overlay0, surface2, surface1, surface0, base, mantle, crust
    }

    enum Flavour {
        case latte, mocha

        static var current: Flavour {
            UITraitCollection.current.userInterfaceStyle == .dark ? .mocha : .latte
        }

        func callAsFunction(_ swatch: Swatch) -> Color {
            let hex = (self == .latte ? Catppuccin.latte : Catppuccin.mocha)[swatch.rawValue]
            return Color(
                red: Double((hex >> 16) & 0xff) / 255,
                green: Double((hex >> 8) & 0xff) / 255,
                blue: Double(hex & 0xff) / 255)
        }
    }

    private static let latte: [UInt32] = [
        0xdc8a78, 0xdd7878, 0xea76cb, 0x8839ef, 0xd20f39, 0xe64553, 0xfe640b, 0xdf8e1d,
        0x40a02b, 0x179299, 0x04a5e5, 0x209fb5, 0x1e66f5, 0x7287fd, 0x4c4f69, 0x5c5f77,
        0x6c6f85, 0x7c7f93, 0x8c8fa1, 0x9ca0b0, 0xacb0be, 0xbcc0cc, 0xccd0da, 0xeff1f5,
        0xe6e9ef, 0xdce0e8,
    ]

    private static let mocha: [UInt32] = [
        0xf5e0dc, 0xf2cdcd, 0xf5c2e7, 0xcba6f7, 0xf38ba8, 0xeba0ac, 0xfab387, 0xf9e2af,
        0xa6e3a1, 0x94e2d5, 0x89dceb, 0x74c7ec, 0x89b4fa, 0xb4befe, 0xcdd6f4, 0xbac2de,
        0xa6adc8, 0x9399b2, 0x7f849c, 0x6c7086, 0x585b70, 0x45475a, 0x313244, 0x1e1e2e,
        0x181825, 0x11111b,
    ]
}
