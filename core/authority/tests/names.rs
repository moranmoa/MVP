//! Display names (B23, B23-z): the strict rule for channel and server names,
//! and the one exception for a person's display name, a zero-width joiner
//! inside an emoji ZWJ sequence.

use authority::{channel_name_ok, display_name_ok, display_text_ok};

const VALID_SEQUENCES: [&str; 8] = [
    "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", // family 👨‍👩‍👧
    "\u{1F9D1}\u{200D}\u{1F680}",                  // astronaut 🧑‍🚀
    "\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}",         // 👩🏽‍💻, skin tone before the ZWJ
    "\u{1F469}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F468}", // 👩‍❤️‍👨, VS16 before the ZWJ
    "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",          // rainbow flag 🏳️‍🌈
    "\u{1F415}\u{200D}\u{1F9BA}",                  // service dog 🐕‍🦺
    "Dana \u{1F469}\u{200D}\u{1F52C}",             // a name with a scientist
    "\u{05D3}\u{05E0}\u{05D4} \u{1F9D1}\u{200D}\u{1F373}", // דנה 🧑‍🍳
];

#[test]
fn display_names_allow_emoji_zwj_sequences() {
    for name in VALID_SEQUENCES {
        assert!(display_name_ok(name), "{name:?}");
    }
}

#[test]
fn channel_and_server_names_stay_strict() {
    for name in VALID_SEQUENCES {
        assert!(!display_text_ok(name), "{name:?}");
        assert!(!channel_name_ok(name), "{name:?}");
    }
}

/// A ZWJ anywhere but between two pictographic emoji is refused.
#[test]
fn a_stray_zwj_is_refused() {
    let stray = [
        "\u{200D}\u{1F469}",                  // leading
        "\u{1F469}\u{200D}",                  // trailing
        "\u{200D}",                           // alone
        "\u{1F469}\u{200D}\u{200D}\u{1F4BB}", // doubled
        "a\u{200D}b",                         // between letters
        "ad\u{200D}min",                      // hiding inside a word
        "\u{1F469}\u{200D}b",                 // emoji, then a letter
        "a\u{200D}\u{1F4BB}",                 // a letter, then an emoji
        "\u{1F469} \u{200D}\u{1F4BB}",        // a space before it
        "\u{1F469}\u{200D} \u{1F4BB}",        // a space after it
        "\u{1F3FD}\u{200D}\u{1F4BB}",         // a skin tone alone before it
        "\u{FE0F}\u{200D}\u{1F4BB}",          // VS16 alone before it
        "\u{1F469}\u{200D}\u{1F3FD}",         // a skin tone after it
        "\u{1F469}\u{200D}\u{FE0F}",          // VS16 after it
        "1\u{200D}\u{1F4BB}",                 // a digit before it
        "\u{05D0}\u{200D}\u{05D1}",           // between Hebrew letters
    ];
    for name in stray {
        assert!(!display_name_ok(name), "{name:?}");
    }
}

/// Every other B23 rule still applies to display names: bidi controls,
/// control characters and the other zero-width characters.
#[test]
fn display_names_keep_the_other_rules() {
    let hostile = [
        "\u{202A}x",
        "\u{202B}x",
        "\u{202C}x",
        "\u{202D}x",
        "a\u{202E}gnp.exe",
        "\u{2066}x",
        "\u{2067}x",
        "\u{2068}x",
        "\u{2069}x",
        "a\u{200E}b",
        "a\u{200F}b",
        "a\u{061C}b",
        "a\u{0000}b",
        "a\nb",
        "a\tb",
        "a\u{007F}b",
        "a\u{0085}b",
        "a\u{009B}b",
        "ad\u{200B}min",
        "a\u{200C}b",
        "a\u{2060}b",
        "\u{FEFF}general",
        // A valid sequence doesn't excuse a hostile character next to it.
        "\u{1F468}\u{200D}\u{1F469}\u{202E}",
        "\u{1F468}\u{200B}\u{1F469}",
    ];
    for name in hostile {
        assert!(!display_name_ok(name), "{name:?}");
    }
    for name in ["Dana", "דנה", "عام", "😀", "café", "a b", "\u{1F469}\u{1F3FD}"] {
        assert!(display_name_ok(name), "{name:?}");
    }
}
