//! Icon alias mapping and font resolution according to Spec B Tier 1.

/// Resolve an icon name, alias, or raw codepoint into a unicode string.
/// If an alias is unknown, returns "⚠" as a graceful fallback without crashing.
pub fn resolve_icon(alias_or_codepoint: &str) -> String {
    resolve_icon_or(alias_or_codepoint, "⚠")
}

/// Human-agnostic neutral glyph used when a name is neither a known alias nor a
/// codepoint. Tray items carry freedesktop theme names, which must never render
/// as the broken-widget "⚠" marker.
pub const FALLBACK_UNKNOWN_ICON: &str = "fa-circle-dot";

/// The resolved glyph form of [`FALLBACK_UNKNOWN_ICON`], for callers that need
/// the final render-ready string (e.g. menu item icons).
pub const FALLBACK_UNKNOWN_GLYPH: &str = "\u{f192}";

/// Resolve an icon alias with a caller-provided fallback for unknown names.
pub fn resolve_icon_or(alias_or_codepoint: &str, fallback: &str) -> String {
    let trimmed = alias_or_codepoint.trim();

    // Check if it's already a single unicode character (such as a direct nerd font or FA codepoint)
    if trimmed.chars().count() == 1 {
        return trimmed.to_string();
    }

    // Check alias table
    if let Some(glyph) = lookup_alias(trimmed) {
        return glyph.to_string();
    }

    // Check if it's a hexadecimal unicode representation like "\uf028" or "u+f028"
    if let Some(code_hex) = trimmed
        .strip_prefix("\\u{")
        .and_then(|s| s.strip_suffix('}'))
        .or_else(|| trimmed.strip_prefix("\\u"))
        .or_else(|| trimmed.strip_prefix("U+"))
        .or_else(|| trimmed.strip_prefix("u+"))
    {
        if let Ok(val) = u32::from_str_radix(code_hex, 16) {
            if let Some(ch) = char::from_u32(val) {
                return ch.to_string();
            }
        }
    }

    // Fallback indicator
    fallback.to_string()
}

fn lookup_alias(alias: &str) -> Option<&'static str> {
    match alias {
        // Audio / Volume
        "fa-volume-high" | "volume-high" | "fa-volume-up" => Some("\u{f028}"),
        "fa-volume-low" | "volume-low" | "fa-volume-down" => Some("\u{f027}"),
        "fa-volume-off" | "volume-off" => Some("\u{f026}"),
        "fa-volume-xmark" | "fa-volume-mute" | "volume-mute" => Some("\u{f6a9}"),

        // Network / Wi-Fi
        "fa-wifi" | "wifi" => Some("\u{f1eb}"),
        "fa-wifi-weak" | "wifi-weak" => Some("\u{f1eb}"),
        "fa-network-wired" | "network-wired" | "fa-ethernet" => Some("\u{f6ff}"),

        // Bluetooth
        "fa-bluetooth" | "bluetooth" => Some("\u{f293}"),
        "fa-bluetooth-b" | "bluetooth-b" => Some("\u{f294}"),

        // Battery / Power
        "fa-battery-full" | "fa-battery-4" | "fa-battery" | "battery" => Some("\u{f240}"),
        "fa-battery-three-quarters" | "fa-battery-3" => Some("\u{f241}"),
        "fa-battery-half" | "fa-battery-2" => Some("\u{f242}"),
        "fa-battery-quarter" | "fa-battery-1" => Some("\u{f243}"),
        "fa-battery-empty" | "fa-battery-0" => Some("\u{f244}"),
        "fa-bolt" | "bolt" | "fa-charging" => Some("\u{f0e7}"),
        "fa-plug" | "plug" => Some("\u{f1e6}"),
        "fa-leaf" | "leaf" => Some("\u{f06c}"),
        "fa-gauge" | "fa-gauge-high" | "gauge" => Some("\u{f625}"),
        "fa-rocket" | "rocket" => Some("\u{f135}"),
        "fa-scale-balanced" | "fa-balance-scale" | "balance-scale" => Some("\u{f515}"),

        // Notifications
        "fa-bell" | "bell" => Some("\u{f0f3}"),
        "fa-bell-slash" | "bell-slash" => Some("\u{f1f6}"),

        // System stats / hardware
        "fa-cpu" | "fa-microchip" | "microchip" => Some("\u{f2db}"),
        "fa-memory" | "memory" => Some("\u{f538}"),
        "fa-hdd" | "fa-hard-drive" | "hard-drive" => Some("\u{f0a0}"),
        "fa-temperature-half" | "fa-thermometer" | "thermometer" => Some("\u{f2c9}"),

        // Workspaces / Indicators
        "fa-circle" | "circle" => Some("\u{f111}"),
        "fa-circle-dot" | "circle-dot" => Some("\u{f192}"),
        "fa-circle-exclamation" | "fa-exclamation-circle" => Some("\u{f06a}"),
        "fa-chevron-up" | "chevron-up" => Some("\u{f077}"),
        "fa-chevron-down" | "chevron-down" => Some("\u{f078}"),
        "fa-chevron-right" | "chevron-right" => Some("\u{f054}"),
        "fa-chevron-left" | "chevron-left" => Some("\u{f053}"),
        "fa-xmark" | "fa-times" | "fa-x" | "xmark" => Some("\u{f00d}"),
        "fa-circle-o" | "fa-circle-regular" | "circle-o" => Some("\u{f10c}"),

        // Clock / Calendar
        "fa-clock" | "clock" => Some("\u{f017}"),
        "fa-calendar" | "calendar" => Some("\u{f133}"),

        // Clipboard
        "fa-clipboard" | "clipboard" => Some("\u{f328}"),
        "fa-clipboard-list" | "clipboard-list" => Some("\u{f46d}"),

        // Misc / Settings
        "fa-gear" | "fa-cog" | "gear" => Some("\u{f013}"),
        "fa-triangle-exclamation" | "fa-warning" | "warning" => Some("\u{f071}"),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_known_aliases() {
        assert_eq!(resolve_icon("fa-volume-high"), "\u{f028}");
        assert_eq!(resolve_icon("fa-wifi"), "\u{f1eb}");
        assert_eq!(resolve_icon("fa-battery-full"), "\u{f240}");
        assert_eq!(resolve_icon("fa-bell-slash"), "\u{f1f6}");
    }

    #[test]
    fn test_resolve_hex_codepoint() {
        assert_eq!(resolve_icon("\\u{f028}"), "\u{f028}");
        assert_eq!(resolve_icon("u+f028"), "\u{f028}");
        assert_eq!(resolve_icon("U+F028"), "\u{f028}");
    }

    #[test]
    fn test_resolve_direct_unicode() {
        assert_eq!(resolve_icon("\u{f028}"), "\u{f028}");
    }

    #[test]
    fn test_resolve_unknown_alias_fallback() {
        assert_eq!(resolve_icon("unknown-alias-xyz"), "⚠");
    }

    #[test]
    fn test_tray_default_aliases_resolve() {
        assert_eq!(resolve_icon("fa-chevron-up"), "\u{f077}");
        assert_eq!(resolve_icon("fa-circle-exclamation"), "\u{f06a}");
        assert_eq!(resolve_icon("fa-circle-dot"), "\u{f192}");
        assert_eq!(resolve_icon("fa-triangle-exclamation"), "\u{f071}");
    }

    #[test]
    fn test_resolve_icon_or_never_warning() {
        // Theme icon names are not FA aliases: with resolve_icon_or they fall back
        // to the caller-provided glyph, never the broken-widget marker.
        assert_eq!(
            resolve_icon_or("nm-signal-100", FALLBACK_UNKNOWN_GLYPH),
            FALLBACK_UNKNOWN_GLYPH
        );
        assert_eq!(resolve_icon("nm-signal-100"), "⚠");
        assert_eq!(
            resolve_icon_or("fa-bluetooth", FALLBACK_UNKNOWN_GLYPH),
            "\u{f293}"
        );
    }
}
