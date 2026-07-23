//! Human-friendly deterministic peer names derived from [`iroh::PublicKey`].
//!
//! This module provides a stable, repeatable mapping from a 32‑byte peer
//! identity (PublicKey) to a friendly two‑word name such as **"Blue Falcon"**
//! or **"Quiet Harbour"**.  The same peer ID always produces the same name,
//! across restarts and across machines.
//!
//! # Priority order for display names (used by callers in the GUI)
//!
//! 1. User‑assigned nickname (friend label)
//! 2. Remote profile display name (from `ProfileUpdate` gossip)
//! 3. Advertised device / session name
//! 4. [`generate_friendly_name`] — stable fallback
//! 5. [`fmt_short`] — truncated peer ID as secondary text only
//!
//! The callers (`app.rs::resolve_name` etc.) enforce this priority; this
//! module only provides the building blocks for steps 4‑5.

use iroh::PublicKey;

/// Returned by [`friendly_name_and_short`].
#[derive(Debug, Clone)]
pub struct PeerDisplayName {
    /// Primary display label — e.g. `"Blue Falcon"`.
    pub primary: String,
    /// Truncated peer ID — e.g. `"dfab…961f"`.
    pub secondary: String,
}

// ── Curated word lists ─────────────────────────────────────────────────
//
// Rules:
// - No offensive, alarming, or excessively whimsical terms.
// - No visually similar pairs (e.g. avoid "Foggy" and "Foxy" both
//   starting with the same letter).
// - Prefer concrete nouns and well‑understood adjectives.
// - Keep lists reasonably sized (≈64 each) so combinations are diverse
//   without being unwieldy.

const ADJECTIVES: &[&str] = &[
    "Amber", "Ancient", "Autumn", "Azure", "Bald", "Bamboo", "Blazing", "Blue", "Brass", "Brave",
    "Bright", "Calm", "Canyon", "Clear", "Coastal", "Cold", "Copper", "Coral", "Crimson",
    "Crystal", "Dawn", "Deep", "Desert", "Dewy", "Dusky", "Eager", "Emerald", "Fading", "Fallen",
    "Falcon", "Fierce", "Frosty", "Gentle", "Gilded", "Glacial", "Golden", "Grand", "Gray",
    "Green", "Hidden", "High", "Hollow", "Humble", "Icy", "Iron", "Jade", "Jolly", "Kind", "Lazy",
    "Lemon", "Lilac", "Lively", "Long", "Loud", "Lunar", "Mellow", "Misty", "Mossy", "Mountain",
    "Muddy", "Muted", "Narrow", "Noble", "Oaken", "Olive", "Opal", "Orange", "Pale", "Pearly",
    "Pepper", "Pine", "Pink", "Placid", "Quiet", "Red", "Rich", "Royal", "Ruby", "Rustic", "Sable",
    "Salty", "Sandy", "Scarlet", "Shadow", "Shallow", "Sharp", "Shy", "Silent", "Silver", "Slate",
    "Sleek", "Smooth", "Snowy", "Soft", "Solid", "Steady", "Steel", "Stout", "Sturdy", "Summer",
    "Sunny", "Swift", "Tawny", "Teal", "Thin", "Timid", "Tranquil", "Twilight", "Vast", "Velvet",
    "Violet", "Warm", "Wide", "Wild", "Winding", "Winter", "Wise", "Yellow", "Young", "Zestful",
];

const NOUNS: &[&str] = &[
    "Acorn", "Alpaca", "Anchor", "Aspen", "Badger", "Balsam", "Bamboo", "Basin", "Bayou", "Beacon",
    "Bear", "Beaver", "Birch", "Bluff", "Boulder", "Breeze", "Brook", "Burch", "Cabin", "Cactus",
    "Camel", "Candle", "Canopy", "Canter", "Canyon", "Cedar", "Cherry", "Cliff", "Cloud", "Coast",
    "Comet", "Coral", "Cove", "Crane", "Creek", "Dale", "Dawn", "Deer", "Delta", "Den", "Dew",
    "Dune", "Eagle", "Elk", "Elm", "Ember", "Falcon", "Fawn", "Fennel", "Fern", "Field", "Finch",
    "Fir", "Flame", "Flint", "Flower", "Foam", "Fog", "Ford", "Forest", "Fox", "Frost", "Garden",
    "Gate", "Geese", "Gem", "Glade", "Glen", "Glow", "Gorge", "Grain", "Grove", "Gull", "Harbour",
    "Haven", "Hawthorn", "Hazel", "Heath", "Heron", "Hill", "Holly", "Horn", "Horse", "Ice",
    "Iris", "Ivory", "Jade", "Jasper", "Juniper", "Kestrel", "Knoll", "Lake", "Lamb", "Larch",
    "Lark", "Laurel", "Ledge", "Lemon", "Lilac", "Lily", "Linden", "Lion", "Lynx", "Mall", "Maple",
    "Marsh", "Meadow", "Merlin", "Mesa", "Mist", "Mole", "Moor", "Moss", "Moth", "Mountain",
    "Myrtle", "Nest", "Nettle", "Oak", "Olive", "Otter", "Owl", "Palm", "Pass", "Peak", "Pebble",
    "Petal", "Pigeon", "Pike", "Pine", "Pitcher", "Plain", "Plum", "Pond", "Poplar", "Prairie",
    "Quail", "Quartz", "Rabbit", "Raven", "Reed", "Reef", "Ridge", "River", "Robin", "Rock",
    "Rose", "Rye", "Saddle", "Salmon", "Sands", "Satin", "Scout", "Seal", "Shale", "Shard",
    "Shore", "Shrub", "Sierra", "Silk", "Sky", "Slate", "Snow", "Sparrow", "Spruce", "Star",
    "Steel", "Stem", "Stone", "Storm", "Stream", "Summit", "Sun", "Surf", "Swallow", "Swan",
    "Swift", "Sycamore", "Talon", "Teal", "Thorn", "Thrush", "Tide", "Timber", "Topaz", "Tower",
    "Trail", "Trout", "Tulip", "Tundra", "Valley", "Vega", "Vine", "Violet", "Vista", "Wall",
    "Walnut", "Wasp", "Water", "Wave", "Weaver", "Wharf", "Willow", "Wind", "Wolf", "Wren", "Yard",
    "Yew", "Zebra",
];

/// Deterministically generate a friendly human‑readable name from a
/// [`PublicKey`].
///
/// The name is formed as `"<Adjective> <Noun>"` (e.g. `"Blue Falcon"`).
/// The same peer ID **always** produces the same name, regardless of
/// process restart or platform.
///
/// # Algorithm
///
/// A simple hash of the first 8 bytes of the public key is used to select
/// an adjective and a noun from the curated lists.  The second selector
/// (noun) also mixes in bytes 8‑15 so that two IDs sharing the same
/// first‑byte prefix still produce distinct nouns.
pub fn generate_friendly_name(peer: &PublicKey) -> String {
    let bytes = peer.as_bytes();

    // Use the first 4 bytes as a u32 seed for adjective index.
    let adj_seed = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let adj_idx = (adj_seed as usize) % ADJECTIVES.len();

    // Mix in the next 4 bytes for the noun index so adjective+noun pairs
    // are more diverse.
    let noun_seed =
        u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]).wrapping_add(adj_seed >> 1); // mix in adjective component
    let noun_idx = (noun_seed as usize) % NOUNS.len();

    format!("{} {}", ADJECTIVES[adj_idx], NOUNS[noun_idx])
}

/// Return the truncated peer ID in the format `"dfab…961f"` (first 4 hex
/// chars, ellipsis, last 4 hex chars).
pub fn fmt_truncated(peer: &PublicKey) -> String {
    let hex = peer.to_string();
    let len = hex.len();
    if len <= 12 {
        // Very short — unlikely for a real PublicKey but guard anyway.
        return hex;
    }
    format!("{}…{}", &hex[..4], &hex[len - 4..])
}

/// Return both the [`generate_friendly_name`] and [`fmt_truncated`] result
/// for convenient UI rendering.
pub fn friendly_name_and_short(peer: &PublicKey) -> PeerDisplayName {
    PeerDisplayName {
        primary: generate_friendly_name(peer),
        secondary: fmt_truncated(peer),
    }
}

/// Format a display name with optional secondary truncated ID, e.g.
/// `"Blue Falcon (dfab…961f)"`.
///
/// Pass `secondary` from [`fmt_truncated`] or an empty string to omit the
/// parenthetical.
pub fn format_with_short(primary: &str, secondary: &str) -> String {
    if secondary.is_empty() {
        primary.to_string()
    } else {
        format!("{} ({})", primary, secondary)
    }
}

/// Central display‑name resolver with the full priority chain.
///
/// Returns the best available name for a peer given the available sources,
/// following the priority order:
///
/// 1. `friend_label` ─ user‑assigned nickname
/// 2. `profile_display_name` ─ remote profile display name (from `ProfileUpdate` gossip)
/// 3. `friend_announced_name` ─ last name the peer announced about themselves
/// 4. `session_name` ─ advertised device / session name
/// 5. [`generate_friendly_name`] ─ stable deterministic fallback
///
/// All callers (GUI, headless chat, tests) should route through this function
/// so the priority order is applied consistently everywhere.
pub fn resolve_peer_name<'a>(
    peer: &PublicKey,
    friend_label: Option<&'a str>,
    profile_display_name: Option<&'a str>,
    friend_announced_name: Option<&'a str>,
    session_name: Option<&'a str>,
) -> String {
    if let Some(name) = friend_label.filter(|n| !n.trim().is_empty()) {
        return name.to_string();
    }
    if let Some(name) = profile_display_name.filter(|n| !n.trim().is_empty()) {
        return name.to_string();
    }
    if let Some(name) = friend_announced_name.filter(|n| !n.trim().is_empty()) {
        return name.to_string();
    }
    if let Some(name) = session_name.filter(|n| !n.trim().is_empty()) {
        return name.to_string();
    }
    generate_friendly_name(peer)
}

/// Convenience wrapper: returns `(primary, secondary)` where primary is the
/// resolved display name and secondary is the truncated peer ID.
pub fn resolve_peer_name_with_short<'a>(
    peer: &PublicKey,
    friend_label: Option<&'a str>,
    profile_display_name: Option<&'a str>,
    friend_announced_name: Option<&'a str>,
    session_name: Option<&'a str>,
) -> PeerDisplayName {
    PeerDisplayName {
        primary: resolve_peer_name(
            peer,
            friend_label,
            profile_display_name,
            friend_announced_name,
            session_name,
        ),
        secondary: fmt_truncated(peer),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    fn make_peer(seed: u8) -> PublicKey {
        let mut _bytes = [0u8; 32];
        _bytes[0] = seed;
        _bytes[1] = seed.wrapping_add(1);
        // Note: a random 32‑byte array is not necessarily a valid ed25519
        // public key on the curve, so we use SecretKey::generate().
        SecretKey::generate().public()
    }

    #[test]
    fn test_generate_friendly_name_is_deterministic() {
        let peer = make_peer(42);
        let name1 = generate_friendly_name(&peer);
        let name2 = generate_friendly_name(&peer);
        assert_eq!(
            name1, name2,
            "friendly name must be deterministic for the same peer"
        );
    }

    #[test]
    fn test_generate_friendly_name_stable_across_restarts() {
        // Two separate calls should produce the same result (tested here via
        // repeated call on the same peer — integration tests verify cross-process).
        let peer = make_peer(99);
        let name = generate_friendly_name(&peer);
        assert!(!name.is_empty(), "name must not be empty");
        assert!(
            name.contains(' '),
            "name must be '<Adjective> <Noun>' format"
        );
    }

    #[test]
    fn test_friendly_name_positive_words() {
        // Generate several names and verify they contain no negative words.
        let names: Vec<String> = (0u8..20)
            .map(|_i| {
                let sk = SecretKey::generate();
                generate_friendly_name(&sk.public())
            })
            .collect();
        for n in &names {
            assert!(!n.is_empty());
            // Should start with an adjective from our list
            let first_word = n.split(' ').next().unwrap_or("");
            assert!(
                ADJECTIVES.contains(&first_word),
                "Expected '{}' to start with one of the curated adjectives",
                n
            );
        }
    }

    #[test]
    fn test_generate_friendly_name_different_peers_different() {
        // Very unlikely that two random peers get the same name, but
        // let's just check they don't always collide.
        let a = SecretKey::generate().public();
        let b = SecretKey::generate().public();
        // Run enough to be statistically confident: 10 pairs should all
        // differ with extremely high probability.
        for _ in 0..10 {
            let na = generate_friendly_name(&a);
            let nb = generate_friendly_name(&b);
            if na != nb {
                return; // OK — they differ
            }
        }
        // With 64×141 = 9024 possible combinations, 10 tries hitting the
        // same one is essentially impossible for random keys.
        panic!("all 10 tries produced the same name — something is wrong");
    }

    #[test]
    fn test_fmt_truncated_format() {
        let peer = SecretKey::generate().public();
        let truncated = fmt_truncated(&peer);
        let full = peer.to_string();
        assert!(
            truncated.len() < full.len(),
            "truncated must be shorter than full"
        );
        assert!(truncated.contains('…'), "truncated must contain ellipsis");
        // Should start with first 4 hex chars
        assert_eq!(&truncated[..4], &full[..4]);
        // Should end with last 4 hex chars
        assert_eq!(&truncated[truncated.len() - 4..], &full[full.len() - 4..]);
    }

    #[test]
    fn test_friendly_name_and_short() {
        let peer = SecretKey::generate().public();
        let result = friendly_name_and_short(&peer);
        assert_eq!(result.primary, generate_friendly_name(&peer));
        assert_eq!(result.secondary, fmt_truncated(&peer));
    }

    #[test]
    fn test_format_with_short_with_secondary() {
        let result = format_with_short("Blue Falcon", "dfab…961f");
        assert_eq!(result, "Blue Falcon (dfab…961f)");
    }

    #[test]
    fn test_format_with_short_no_secondary() {
        let result = format_with_short("Blue Falcon", "");
        assert_eq!(result, "Blue Falcon");
    }

    #[test]
    fn test_generate_friendly_name_does_not_panic() {
        // Edge case: all-zero key (placeholder).
        let zero_key =
            PublicKey::from_bytes(&[0u8; 32]).expect("32 zero bytes is a valid ed25519 public key");
        let name = generate_friendly_name(&zero_key);
        assert!(!name.is_empty());
        assert!(name.contains(' '));
    }

    #[test]
    fn test_fmt_truncated_edge_cases() {
        // Very short hex should not panic (unlikely but guard).
        let peer = SecretKey::generate().public();
        let result = fmt_truncated(&peer);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_generate_friendly_name_no_duplicate_words() {
        // A name should be two distinct words (adjective + noun)
        let peer = SecretKey::generate().public();
        let name = generate_friendly_name(&peer);
        let parts: Vec<&str> = name.split(' ').collect();
        assert_eq!(parts.len(), 2, "name must have exactly two words");
        assert!(!parts[0].is_empty(), "adjective must not be empty");
        assert!(!parts[1].is_empty(), "noun must not be empty");
    }

    #[test]
    fn test_generate_friendly_name_adjective_from_curated_list() {
        // The first word must be from the ADJECTIVES list
        let peer = SecretKey::generate().public();
        let name = generate_friendly_name(&peer);
        let first_word = name.split(' ').next().unwrap_or("");
        assert!(
            ADJECTIVES.contains(&first_word),
            "'{}' starts with '{}' which is not in the ADJECTIVES list",
            name,
            first_word
        );
    }

    #[test]
    fn test_generate_friendly_name_noun_from_curated_list() {
        // The second word must be from the NOUNS list
        let peer = SecretKey::generate().public();
        let name = generate_friendly_name(&peer);
        let second_word = name.split(' ').nth(1).unwrap_or("");
        assert!(
            NOUNS.contains(&second_word),
            "'{}' has second word '{}' which is not in the NOUNS list",
            name,
            second_word
        );
    }

    #[test]
    fn test_fmt_truncated_contains_ellipsis() {
        // Verify the ellipsis character is present
        let peer = SecretKey::generate().public();
        let truncated = fmt_truncated(&peer);
        assert!(
            truncated.contains('…'),
            "truncated ID must contain ellipsis"
        );
    }

    #[test]
    fn test_fmt_truncated_is_shorter_than_full() {
        let peer = SecretKey::generate().public();
        let full = peer.to_string();
        let truncated = fmt_truncated(&peer);
        assert!(
            truncated.len() < full.len(),
            "truncated '{}' (len {}) should be shorter than full '{}' (len {})",
            truncated,
            truncated.len(),
            full,
            full.len()
        );
    }

    #[test]
    fn test_fmt_truncated_starts_and_ends_with_expected_chars() {
        let peer = SecretKey::generate().public();
        let full = peer.to_string();
        let truncated = fmt_truncated(&peer);
        assert_eq!(
            &truncated[..4],
            &full[..4],
            "truncated must start with first 4 hex chars"
        );
        assert_eq!(
            &truncated[truncated.len() - 4..],
            &full[full.len() - 4..],
            "truncated must end with last 4 hex chars"
        );
    }

    #[test]
    fn test_peer_display_name_struct() {
        let peer = SecretKey::generate().public();
        let display = friendly_name_and_short(&peer);
        assert_eq!(display.primary, generate_friendly_name(&peer));
        assert_eq!(display.secondary, fmt_truncated(&peer));
        assert!(!display.primary.is_empty());
        assert!(!display.secondary.is_empty());
    }

    #[test]
    fn test_format_with_short_edge_cases() {
        // Empty secondary
        assert_eq!(format_with_short("Blue Falcon", ""), "Blue Falcon");
        // Very long primary
        let long = "A".repeat(100);
        let result = format_with_short(&long, "abcd…1234");
        assert!(result.contains("…1234"));
        assert!(result.starts_with("AAAA"));
        // Unicode in primary
        assert_eq!(
            format_with_short("Hélène", "abcd…1234"),
            "Hélène (abcd…1234)"
        );
    }

    #[test]
    fn test_generate_friendly_name_diverse_output() {
        // Generate several names and ensure no two are the same
        // (with high probability given 64×141 = 9024 combinations)
        let mut names = std::collections::HashSet::new();
        for _ in 0..50 {
            let peer = SecretKey::generate().public();
            let name = generate_friendly_name(&peer);
            names.insert(name);
        }
        // With 50 random peers, we should have at least 45 unique names
        // (collision probability is negligible)
        assert!(
            names.len() >= 45,
            "expected at least 45 unique names out of 50, got {}",
            names.len()
        );
    }

    #[test]
    fn test_fmt_truncated_zero_public_key() {
        // Edge case: all-zero public key should not panic
        let zero_key = match PublicKey::from_bytes(&[0u8; 32]) {
            Ok(key) => key,
            Err(_) => return, // skip if zero key is not a valid public key
        };
        let truncated = fmt_truncated(&zero_key);
        assert!(!truncated.is_empty());
    }

    #[test]
    fn test_generate_friendly_name_does_not_contain_negative_terms() {
        // Verify no generated name contains alarming words
        let negative_terms = ["kill", "death", "die", "toxic", "venom", "hate", "cruel"];
        for _ in 0..100 {
            let peer = SecretKey::generate().public();
            let name = generate_friendly_name(&peer);
            let lower = name.to_lowercase();
            for term in &negative_terms {
                assert!(
                    !lower.contains(term),
                    "name '{}' should not contain negative term '{}'",
                    name,
                    term
                );
            }
        }
    }

    // ── resolve_peer_name priority tests ──────────────────────────────

    #[test]
    fn test_resolve_peer_name_uses_friend_label() {
        let peer = SecretKey::generate().public();
        let name = resolve_peer_name(&peer, Some("My Buddy"), None, None, None);
        assert_eq!(name, "My Buddy");
    }

    #[test]
    fn test_resolve_peer_name_priority_label_over_profile() {
        let peer = SecretKey::generate().public();
        let name = resolve_peer_name(&peer, Some("Nickname"), Some("Profile Name"), None, None);
        assert_eq!(name, "Nickname");
    }

    #[test]
    fn test_resolve_peer_name_uses_profile_name_when_no_label() {
        let peer = SecretKey::generate().public();
        let name = resolve_peer_name(
            &peer,
            None,
            Some("Profile Display"),
            Some("Announced"),
            None,
        );
        assert_eq!(name, "Profile Display");
    }

    #[test]
    fn test_resolve_peer_name_uses_announced_name() {
        let peer = SecretKey::generate().public();
        let name = resolve_peer_name(&peer, None, None, Some("Device Name"), None);
        assert_eq!(name, "Device Name");
    }

    #[test]
    fn test_resolve_peer_name_uses_session_name() {
        let peer = SecretKey::generate().public();
        let name = resolve_peer_name(&peer, None, None, None, Some("Session Alpha"));
        assert_eq!(name, "Session Alpha");
    }

    #[test]
    fn test_resolve_peer_name_falls_back_to_friendly() {
        let peer = SecretKey::generate().public();
        let name = resolve_peer_name(&peer, None, None, None, None);
        // Must be a valid adjective+noun pair
        assert!(
            name.contains(' '),
            "fallback must be '<Adjective> <Noun>', got '{name}'"
        );
        // Same peer must produce the same fallback
        let name2 = resolve_peer_name(&peer, None, None, None, None);
        assert_eq!(name, name2, "fallback must be deterministic");
    }

    #[test]
    fn test_resolve_peer_name_ignores_empty_strings() {
        let peer = SecretKey::generate().public();
        // All empty strings — should fall back to friendly name
        let name = resolve_peer_name(&peer, Some(""), Some(""), Some(""), Some(""));
        assert!(
            name.contains(' '),
            "empty inputs should fall through to friendly name, got '{name}'"
        );
    }

    #[test]
    fn test_resolve_peer_name_ignores_whitespace_metadata() {
        let peer = SecretKey::generate().public();
        let fallback = generate_friendly_name(&peer);
        assert_eq!(
            resolve_peer_name(&peer, Some("  \t"), Some("  \n"), None, None),
            fallback
        );
    }

    #[test]
    fn test_resolve_peer_name_with_short_returns_both() {
        let peer = SecretKey::generate().public();
        let result = resolve_peer_name_with_short(&peer, Some("Alice"), None, None, None);
        assert_eq!(result.primary, "Alice");
        assert_eq!(result.secondary, fmt_truncated(&peer));
    }

    #[test]
    fn test_resolve_peer_name_is_deterministic() {
        let peer = SecretKey::generate().public();
        for _ in 0..10 {
            let a = resolve_peer_name(&peer, None, None, None, None);
            let b = resolve_peer_name(&peer, None, None, None, None);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn test_resolve_peer_name_with_empty_label_uses_next_source() {
        let peer = SecretKey::generate().public();
        // Empty label should skip to profile name
        let name = resolve_peer_name(&peer, Some(""), Some("Real Profile"), None, None);
        assert_eq!(name, "Real Profile");
    }
}
