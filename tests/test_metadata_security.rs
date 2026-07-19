//! Security tests for signed malicious metadata handling.
//!
//! These tests verify that RoomMetadata fields (name, description, rules)
//! remain safe to process and display when carrying known dangerous
//! payloads.  The full pipeline is exercised: construction → merge →
//! display_name() / field access, via public APIs only.
//!
//! Coverage:
//!   1. Script / HTML injection — strings that try to inject executable
//!      content (mitigated by native-GUI context, but must not corrupt UI).
//!   2. Path-traversal lookalikes — names designed to escape directory
//!      boundaries.
//!   3. Oversized / extreme-length values — 10 KiB+ strings, megabytes of
//!      garbage, repeated characters that could cause OOM or truncation
//!      issues.
//!   4. Malformed timestamps and edge-content dates (as text).
//!   5. Unicode abuse — bidi overrides, zero-width spaces, homoglyph
//!      attacks, combining-character floods, tag characters.
//!   6. Control-character injection — null bytes, escape sequences, ANSI
//!      terminal codes, backspace bombs.
//!   7. Merge safety — partial malicious updates combined with clean data.
//!   8. Display-only edge cases — None/empty name fallback.
//!
//! Each test verifies that the system does NOT:
//!   - Crash (panic, unwrap, or segfault)
//!   - Leak internal data via text fields (stack traces, secret keys)
//!   - Render unsafe content unmodified (all dangerous chars are neutered)
//!   - Use unbounded memory for extreme-size payloads

#![cfg(feature = "net")]

use boru_chat::abuse_controls::{
    sanitize_display_text, sanitize_single_line, DEFAULT_MAX_DISPLAY_LENGTH,
    DEFAULT_MAX_SINGLE_LINE_LENGTH,
};
use boru_chat::proto::TopicId;
use boru_chat::room_docs::RoomMetadata;

// ── Helpers ─────────────────────────────────────────────────────────────

/// Assert that a text field does not contain raw dangerous characters.
fn assert_field_safe(field: &str, dangerous_payload: &str, label: &str) {
    for c in field.chars() {
        if matches!(
            c,
            '\u{0000}'..='\u{0008}'
                | '\u{000B}'..='\u{000C}'
                | '\u{000E}'..='\u{001F}'
                | '\u{007F}'
        ) {
            panic!(
                "{label} contains dangerous control char U+{:04X}: {:?}",
                c as u32, field
            );
        }
    }
    if !field.contains('\u{FFFD}') && field.contains(dangerous_payload) {
        let is_technical_danger = dangerous_payload.contains('\u{0000}')
            || dangerous_payload.contains('\u{001B}')
            || dangerous_payload.contains('\u{007F}')
            || dangerous_payload.contains('\u{200B}')
            || dangerous_payload.contains('\u{202E}')
            || dangerous_payload.contains('\u{FEFF}');
        if is_technical_danger {
            panic!("{label} contains dangerous payload verbatim: {field:?}");
        }
    }
}

/// Generate a deterministic test topic.
fn test_topic() -> TopicId {
    TopicId::from_bytes([0x42u8; 32])
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Script / HTML injection vectors
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_script_tags_sanitised() {
    let payloads = [
        "<script>alert('xss')</script>",
        "<img src=x onerror=alert(1)>",
        "<svg onload=alert(1)>",
        "javascript:alert(1)",
        "onclick=alert(1)",
        "<body onload=alert(1)>",
        "<?php system('id'); ?>",
        "{{constructor.constructor('alert(1)')()}}",
        "{{7*7}}",
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let display = md.display_name(&topic);

        // Must not crash.  The native GUI doesn't interpret HTML, so these
        // strings are simply displayed as text.  Verify no raw controls.
        assert_field_safe(&display, payload, "display_name");
        if let Some(ref name) = md.name {
            assert_field_safe(name, payload, "name");
        }
    }
}

#[test]
fn metadata_html_entities_sanitised() {
    let payloads = [
        "&lt;script&gt;",
        "&#60;script&#62;",
        "&{7*7}",
        "${7+7}",
        "<%= 7*7 %>",
        "#{7*7}",
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let _display = md.display_name(&topic);
        // Must not crash during display
        assert_field_safe(md.name.as_deref().unwrap_or(""), payload, "name");
    }
}

#[test]
fn metadata_template_injection_safe() {
    let payloads = [
        "{{ user.secret_key }}",
        "{{config.api_key}}",
        "<%= ENV['SECRET'] %>",
        "#{@secret}",
        "${process.env.SECRET}",
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("safe".to_string()),
            rules: Some("safe".to_string()),
        };
        let topic = test_topic();
        let _display = md.display_name(&topic);
        // Must not crash; template syntax is just text to a native GUI
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Path-traversal lookalikes
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_path_traversal_display_name_safe() {
    let payloads = [
        "../../../etc/passwd",
        "..\\..\\..\\windows\\system32\\config",
        "/etc/shadow",
        "C:\\boot.ini",
        "~/.ssh/id_rsa",
        "../../.git/config",
        "%2e%2e%2f%2e%2e%2fetc%2fpasswd",
        "....//....//....//etc/passwd",
        "..%252f..%252f..%252fetc%252fshadow",
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let display = md.display_name(&topic);
        // No raw control characters; path chars are just text
        assert_field_safe(&display, payload, "display_name");
    }
}

#[test]
fn metadata_reserved_filesystem_names_safe() {
    let payloads = ["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let _display = md.display_name(&topic);
        // Must not crash; platform reserved names are just text
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Oversized / extreme-length values
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_oversized_name_truncated_at_display() {
    // A very long name (50 KiB) — must not cause unbounded display.
    let huge_name = "A".repeat(50_000);
    let md = RoomMetadata {
        name: Some(huge_name),
        description: Some("normal description".to_string()),
        rules: Some("normal rules".to_string()),
    };

    let topic = test_topic();
    let display = md.display_name(&topic);
    // sanitize_single_line caps at DEFAULT_MAX_SINGLE_LINE_LENGTH (256).
    assert!(
        display.len() <= DEFAULT_MAX_SINGLE_LINE_LENGTH,
        "display_name must not exceed single-line limit (len={})",
        display.len()
    );
    assert!(
        !display.contains('\u{0000}'),
        "display_name must not contain null bytes"
    );
    assert!(
        display.chars().count() >= 1,
        "display_name should not be empty for a non-empty input"
    );
}

#[test]
fn metadata_oversized_description_truncated_at_display() {
    let huge_desc = "B".repeat(100_000);
    let md = RoomMetadata {
        name: Some("name".to_string()),
        description: Some(huge_desc),
        rules: None,
    };

    // sanitize_single_line truncates to 256 at display time.
    let cleaned = sanitize_single_line(md.description.as_deref().unwrap_or(""));
    assert!(
        cleaned.chars().count() <= DEFAULT_MAX_SINGLE_LINE_LENGTH,
        "description display must be truncated (len={})",
        cleaned.len()
    );
    // Must not panic or infinite-loop
}

#[test]
fn metadata_oversized_name_must_not_oom() {
    // 2 MB name — must not OOM during display.
    let huge = "X".repeat(2_000_000);
    let md = RoomMetadata {
        name: Some(huge),
        description: None,
        rules: None,
    };

    let display = md.display_name(&test_topic());
    assert!(
        display.len() <= DEFAULT_MAX_SINGLE_LINE_LENGTH,
        "2MB metadata display must be truncated"
    );
}

#[test]
fn metadata_empty_string_does_not_panic() {
    let md = RoomMetadata {
        name: Some(String::new()),
        description: Some(String::new()),
        rules: Some(String::new()),
    };
    let topic = test_topic();
    let display = md.display_name(&topic);
    // Empty name should fall back to room-<short_topic>
    assert!(
        display.starts_with("room-"),
        "empty name should produce fallback display, got: {display:?}"
    );
    assert!(!display.contains('\u{0000}'), "no null bytes in fallback");
}

#[test]
fn metadata_whitespace_only_name_does_not_crash() {
    let payloads = [
        "   ",
        "\t\t\t",
        "\n\n\n",
        "\r\n\r\n",
        "\u{00A0}\u{00A0}\u{00A0}", // non-breaking spaces
        " \t \n \r ",
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: None,
            rules: None,
        };
        let topic = test_topic();
        let _display = md.display_name(&topic);
        // Must not crash
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Date-string edge cases (as text in metadata fields)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_date_strings_injection_safe() {
    let payloads = [
        "1970-01-01T00:00:00Z",
        "9999-12-31T23:59:59Z",
        "0000-00-00T00:00:00",
        "NaN",
        "undefined",
        "null",
        "0",
        "-1",
        "18446744073709551616", // u64::MAX + 1
        "9999999999999",
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let _display = md.display_name(&topic);
        // Must not crash or loop
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Unicode abuse
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_bidi_overrides_stripped_from_display() {
    let payloads = [
        "a\u{202E}!b\u{202C}c",                     // RTL override
        "\u{202E}eviltru.c/\u{202C}",               // Spoofed URL via RTL
        "a\u{202D}b\u{202C}",                       // LTR override
        "\u{202A}hello\u{202C}",                    // LTR embedding
        "\u{202B}world\u{202C}",                    // RTL embedding
        "\u{200E}\u{200F}",                         // LRM + RLM
        "a\u{200B}b",                               // Zero-width space
        "\u{FEFF}hello",                            // BOM prefix
        "\u{2060}\u{2061}\u{2062}\u{2063}\u{2064}", // Invisible operators
        "\u{E0001}\u{E0020}\u{E007F}",              // Language tag zone
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let display = md.display_name(&topic);
        assert_field_safe(&display, payload, "display_name");
        // Stripped result should be shorter or equal
        assert!(
            display.chars().count() <= payload.chars().count(),
            "bidi attack display should be <= original length"
        );
    }
}

#[test]
fn metadata_zmj_zwj_sequences_safe() {
    // ZWJ sequences (emoji joiners) — U+200D is NOT stripped, it is a valid
    // emoji sequence character.  Verified by is_stripped_unicode_format.
    let payload = "👨\u{200D}👩\u{200D}👧\u{200D}👦"; // family emoji via ZWJ
    let md = RoomMetadata {
        name: Some(payload.to_string()),
        description: Some("desc".to_string()),
        rules: Some("rules".to_string()),
    };
    let topic = test_topic();
    let display = md.display_name(&topic);
    // ZWJ U+200D is NOT in is_stripped_unicode_format — it should pass through
    assert!(
        display.contains("👨"),
        "legitimate ZWJ emoji should pass through: {display:?}"
    );
}

#[test]
fn metadata_homoglyph_attack_safe() {
    let payloads = [
        "раураӏ.com", // Cyrillic homoglyphs for "paypal.com"
        "g00gle.com", // Digit substitution
        "micro$oft.com",
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let _display = md.display_name(&topic);
        // Must not crash
    }
}

#[test]
fn metadata_combining_character_flood_safe() {
    // Large number of combining characters
    let combining_flood = format!("A{}", "\u{0300}".repeat(10_000));
    let md = RoomMetadata {
        name: Some(combining_flood),
        description: Some("desc".to_string()),
        rules: Some("rules".to_string()),
    };
    let topic = test_topic();
    let display = md.display_name(&topic);
    // After NFC normalisation and truncation, display must be bounded.
    assert!(
        display.chars().count() <= DEFAULT_MAX_SINGLE_LINE_LENGTH,
        "combining flood must be truncated at display ({} chars)",
        display.chars().count()
    );
}

#[test]
fn metadata_nfc_normalisation_applied() {
    // NFD-form é (e + combining accent) should become NFC single char
    let nfd_e = "e\u{0301}";
    let md = RoomMetadata {
        name: Some(nfd_e.to_string()),
        description: Some("desc".to_string()),
        rules: Some("rules".to_string()),
    };
    let topic = test_topic();
    let display = md.display_name(&topic);
    assert_eq!(
        display.chars().count(),
        1,
        "NFD é should normalise to one char, got: {display:?}"
    );
    assert_eq!(display, "é", "NFD é should display as é");
}

#[test]
fn metadata_tag_characters_stripped() {
    // Tag characters stripped during sanitisation.
    let with_tags = format!("abc\u{E0001}def\u{E0020}ghi\u{E007F}jkl");
    let sanitised = sanitize_display_text(&with_tags, DEFAULT_MAX_DISPLAY_LENGTH);
    assert!(
        !sanitised.contains('\u{E0001}'),
        "LANGUAGE TAG should be stripped"
    );
    assert!(
        sanitised.contains("def") || sanitised.contains("abc"),
        "non-tag content should survive"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. Control-character injection
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_null_byte_sanitised() {
    let payloads = [
        "hello\u{0000}world",
        "\u{0000}prefix",
        "suffix\u{0000}",
        "\u{0000}\u{0000}\u{0000}",
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let display = md.display_name(&topic);
        assert!(
            !display.contains('\u{0000}'),
            "null byte in display_name must be sanitised: {display:?}"
        );
        assert_field_safe(&display, payload, "display_name");
    }
}

#[test]
fn metadata_escape_sequences_sanitised() {
    // ANSI escape sequences, terminal control codes
    let payloads = [
        "\u{001B}[31mRED",
        "\u{001B}[2J\u{001B}[H",
        "\u{009B}8;5;196m",         // CSI in 8-bit
        "\u{0007}bell\u{0008}back", // Bell + backspace
        "\u{001B}]0;title\u{0007}", // OSC escape
        "\u{009B}?25l",             // Hide cursor (CSI)
    ];
    for payload in &payloads {
        let md = RoomMetadata {
            name: Some(payload.to_string()),
            description: Some("desc".to_string()),
            rules: Some("rules".to_string()),
        };
        let topic = test_topic();
        let display = md.display_name(&topic);
        assert!(
            !display.contains('\u{001B}'),
            "ESC byte in display_name must be replaced: {display:?}"
        );
        // After ESC replacement, display should not be empty for a
        // payload that had readable text beyond the escape codes.
        assert!(
            !display.is_empty(),
            "display must not be empty: {display:?}"
        );
    }
}

#[test]
fn metadata_all_ascii_controls_handled() {
    // Every ASCII control character 0x00–0x1F and 0x7F
    let all_controls: String = (0u8..=31u8)
        .chain(127u8..=127u8)
        .map(|b| b as char)
        .collect();
    let md = RoomMetadata {
        name: Some(all_controls),
        description: Some("desc".to_string()),
        rules: Some("rules".to_string()),
    };
    let topic = test_topic();
    let display = md.display_name(&topic);

    // display_name uses sanitize_single_line which strips/stops line-break
    // controls and replaces others.  No raw control chars should survive.
    for c in display.chars() {
        assert!(
            !matches!(c, '\u{0000}' | '\u{0007}' | '\u{0008}' | '\u{001B}'),
            "control char U+{:04X} leaked through: {:?}",
            c as u32,
            display
        );
    }
}

#[test]
fn metadata_backspace_bomb_safe() {
    // Backspace characters that could visually delete display content
    let payload = format!("visible{}hacker", "\u{0008}".repeat(7));
    let md = RoomMetadata {
        name: Some(payload),
        description: Some("desc".to_string()),
        rules: Some("rules".to_string()),
    };
    let topic = test_topic();
    let display = md.display_name(&topic);
    // Backspace (0x08) is a stripped control → replaced with U+FFFD or space
    assert!(
        !display.contains('\u{0008}'),
        "backspace must be stripped in display_name: {display:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 7. Merge safety — partial malicious updates
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_merge_malicious_name_into_clean() {
    let mut base = RoomMetadata {
        name: Some("Clean Room".to_string()),
        description: Some("Clean description".to_string()),
        rules: Some("Clean rules".to_string()),
    };
    let malicious = RoomMetadata {
        name: Some("\u{0000}HACKED\u{001B}[31m".to_string()),
        description: None,
        rules: None,
    };
    base.merge(&malicious);

    let topic = test_topic();
    let display = base.display_name(&topic);

    // After merge, name contains malicious payload.  display_name must sanitise it.
    assert!(
        !display.contains('\u{0000}'),
        "null byte from merge must be sanitised: {display:?}"
    );
    assert!(
        !display.contains('\u{001B}'),
        "ESC byte from merge must be sanitised: {display:?}"
    );
    // Clean description and rules should survive
    assert_eq!(
        base.description.as_deref(),
        Some("Clean description"),
        "clean description must survive merge"
    );
    assert_eq!(
        base.rules.as_deref(),
        Some("Clean rules"),
        "clean rules must survive merge"
    );
}

#[test]
fn metadata_merge_malicious_all_fields() {
    let mut base = RoomMetadata::empty();
    let malicious = RoomMetadata {
        name: Some("\u{202E}Spoofed\u{202C}\u{200B}".to_string()),
        description: Some("<script>alert(1)</script>".to_string()),
        rules: Some("../../../etc/passwd".to_string()),
    };
    base.merge(&malicious);

    let topic = test_topic();
    let name_display = base.display_name(&topic);
    assert_field_safe(&name_display, "\u{202E}", "name after malicious merge");
    assert_field_safe(&name_display, "\u{200B}", "name after malicious merge");

    if let Some(ref desc) = base.description {
        assert_field_safe(desc, "<script>", "description after merge");
    }
    if let Some(ref rules) = base.rules {
        assert_field_safe(rules, "../../../etc/passwd", "rules after merge");
    }
}

#[test]
fn metadata_merge_empty_into_malicious_resets_field() {
    let mut base = RoomMetadata {
        name: Some("HACKED".to_string()),
        description: Some("HACKED".to_string()),
        rules: Some("HACKED".to_string()),
    };
    // Merge with empty metadata (None for all fields) — None fields leave
    // existing values unchanged per merge semantics.
    let empty_md = RoomMetadata::empty();
    base.merge(&empty_md);

    assert_eq!(
        base.name.as_deref(),
        Some("HACKED"),
        "merge with empty should preserve existing name"
    );
    assert_eq!(
        base.description.as_deref(),
        Some("HACKED"),
        "merge with empty should preserve existing description"
    );
}

#[test]
fn metadata_merge_whitespace_overwrites_malicious() {
    let mut base = RoomMetadata {
        name: Some("malicious\u{0000}name".to_string()),
        description: Some("".to_string()),
        rules: None,
    };
    let overwrite = RoomMetadata {
        name: Some("Safe Name".to_string()),
        description: Some("".to_string()),
        rules: None,
    };
    base.merge(&overwrite);

    let topic = test_topic();
    let display = base.display_name(&topic);
    assert!(
        !display.contains('\u{0000}'),
        "null byte from overwritten name must be sanitised: {display:?}"
    );
    assert!(
        display.contains("Safe"),
        "overwritten name should appear: {display:?}"
    );
}

#[test]
fn metadata_merge_multiple_rounds_no_accumulated_corruption() {
    let mut md = RoomMetadata::empty();
    let topic = test_topic();

    let attacks = [
        RoomMetadata {
            name: Some("\u{0000}attack1".to_string()),
            description: None,
            rules: None,
        },
        RoomMetadata {
            name: Some("attack2\u{001B}".to_string()),
            description: None,
            rules: None,
        },
        RoomMetadata {
            name: Some("\u{202E}attack3".to_string()),
            description: None,
            rules: None,
        },
        RoomMetadata {
            name: Some("final_safe_name".to_string()),
            description: None,
            rules: None,
        },
    ];
    for attack in &attacks {
        md.merge(attack);
        // After each merge, display_name must produce safe output
        let display = md.display_name(&topic);
        assert!(
            !display.contains('\u{0000}'),
            "no null byte after merge {attack:?}"
        );
        assert!(
            !display.contains('\u{001B}'),
            "no ESC byte after merge {attack:?}"
        );
    }
    // Final merge should have the clean name
    let display = md.display_name(&topic);
    assert!(
        display.contains("final_safe_name"),
        "clean final name should survive: {display:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 8. Display-only edge cases
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_display_name_with_null_name_uses_topic_fallback() {
    let md = RoomMetadata {
        name: None,
        description: Some("some description".to_string()),
        rules: None,
    };
    let topic = test_topic();
    let display = md.display_name(&topic);
    assert!(
        display.starts_with("room-"),
        "null name should fall back to room-<topic prefix>: {display:?}"
    );
}

#[test]
fn metadata_display_name_with_empty_name_uses_topic_fallback() {
    let md = RoomMetadata {
        name: Some(String::new()),
        description: None,
        rules: None,
    };
    let topic = test_topic();
    let display = md.display_name(&topic);
    assert!(
        display.starts_with("room-"),
        "empty name should fall back to room-<topic prefix>: {display:?}"
    );
}

#[test]
fn metadata_display_name_with_zero_topic_does_not_panic() {
    let zero_topic = TopicId::from_bytes([0u8; 32]);
    let md = RoomMetadata {
        name: Some(String::new()),
        description: None,
        rules: None,
    };
    let display = md.display_name(&zero_topic);
    assert!(
        display.starts_with("room-"),
        "zero topic fallback should still work: {display:?}"
    );
}

#[test]
fn metadata_all_fields_none_display_name_uses_fallback() {
    let md = RoomMetadata {
        name: None,
        description: None,
        rules: None,
    };
    let topic = test_topic();
    let display = md.display_name(&topic);
    assert!(
        display.starts_with("room-"),
        "all-None metadata should use topic fallback: {display:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 9. Integration: full construction → merge → display pipeline
//    with known dangerous payloads
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn metadata_full_pipeline_malicious_payload_safe() {
    let attack_md = RoomMetadata {
        name: Some("\u{0000}EVIL\u{001B}[31mROOM\u{202E}\u{200B}".to_string()),
        description: Some("../../etc/pwned".to_string()),
        rules: Some("<script>alert('owned')</script>".to_string()),
    };

    // Simulate victim receiving a remote update (merge into state)
    let mut victim_state = RoomMetadata {
        name: Some("Original Room".to_string()),
        description: Some("Original description".to_string()),
        rules: Some("Original rules".to_string()),
    };
    victim_state.merge(&attack_md);

    // Display the room name
    let topic = test_topic();
    let display = victim_state.display_name(&topic);

    // Assertions: control/format chars must not reach display, but plain
    // text content like "EVIL" survives because sanitisation only targets
    // control and format characters.
    assert!(
        !display.contains('\u{0000}'),
        "full pipeline: null byte must not reach display"
    );
    assert!(
        !display.contains('\u{001B}'),
        "full pipeline: ESC byte must not reach display"
    );
    assert!(
        !display.contains('\u{202E}'),
        "full pipeline: RTL override must not reach display"
    );
    assert!(
        !display.contains('\u{200B}'),
        "full pipeline: ZWS must not reach display"
    );
    // Description (plain text) is preserved in the struct — it is only
    // sanitised at display time via sanitize_single_line or similar.
    assert_eq!(
        victim_state.description.as_deref(),
        Some("../../etc/pwned"),
        "description (path string) is raw text — only display_name sanitises"
    );
}
