//! Structured peer-ID-backed mentions and composer autocomplete.
//!
//! Mentions are deliberately keyed by the author's public key rather than by
//! mutable display names.  The display label is retained only as presentation
//! metadata and for rendering messages received from older peers.
#![allow(missing_docs)]

use std::collections::HashSet;

/// A mention in a message, identified by the author's stable peer ID.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Mention {
    /// Ed25519 public-key bytes of the mentioned peer.
    pub peer_id: [u8; 32],
    /// Display label captured when the message was composed.
    pub label: String,
    /// Byte range in the message body occupied by the mention.
    pub start: u32,
    /// Exclusive end of the mention range.
    pub end: u32,
}

impl Mention {
    /// Construct a mention for a peer and a body range.
    pub fn new(peer_id: [u8; 32], label: impl Into<String>, start: usize, end: usize) -> Self {
        Self {
            peer_id,
            label: label.into(),
            start: start as u32,
            end: end as u32,
        }
    }

    /// Whether this metadata points at the supplied local peer.
    pub fn targets(&self, local_peer_id: &[u8; 32]) -> bool {
        &self.peer_id == local_peer_id
    }
}

/// A room member usable by autocomplete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MentionMember {
    pub peer_id: [u8; 32],
    pub label: String,
}

impl MentionMember {
    pub fn new(peer_id: [u8; 32], label: impl Into<String>) -> Self {
        Self {
            peer_id,
            label: label.into(),
        }
    }
}

/// Find a valid mention target for legacy text when structured metadata is absent.
/// Duplicate labels are deliberately rejected: a renamed or ambiguous user must
/// not receive a notification intended for another peer.
pub fn fallback_target(text: &str, members: &[MentionMember], local_peer_id: &[u8; 32]) -> bool {
    text.split_whitespace().any(|word| {
        let token = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-');
        let Some(name) = token.strip_prefix('@') else {
            return false;
        };
        let matches: Vec<_> = members
            .iter()
            .filter(|member| member.label.eq_ignore_ascii_case(name))
            .collect();
        matches.len() == 1 && matches[0].peer_id == *local_peer_id
    })
}

/// Whether a message mentions the local peer, using structured metadata first
/// and the old display-name format only as a compatibility fallback.
pub fn mentions_local(
    text: &str,
    mentions: &[Mention],
    members: &[MentionMember],
    local_peer_id: &[u8; 32],
) -> bool {
    mentions
        .iter()
        .any(|mention| mention.targets(local_peer_id))
        || (mentions.is_empty() && fallback_target(text, members, local_peer_id))
}

/// Keyboard actions understood by the autocomplete state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutocompleteKey {
    Up,
    Down,
    Enter,
    Escape,
}

/// Deterministic autocomplete state.  The UI can render `suggestions()` and
/// route keyboard or mouse selection through the methods below.
#[derive(Clone, Debug, Default)]
pub struct Autocomplete {
    query: String,
    selected: usize,
    open: bool,
}

impl Autocomplete {
    pub fn update(&mut self, composer: &str, cursor: usize, members: &[MentionMember]) {
        let prefix = composer[..cursor.min(composer.len())]
            .rsplit_once('@')
            .filter(|(_, tail)| !tail.chars().any(char::is_whitespace))
            .map(|(_, tail)| tail.to_lowercase());
        self.query = prefix.clone().unwrap_or_default();
        self.open = prefix.is_some() && !self.suggestions(members).is_empty();
        self.selected = self
            .selected
            .min(self.suggestions(members).len().saturating_sub(1));
    }

    pub fn suggestions<'a>(&self, members: &'a [MentionMember]) -> Vec<&'a MentionMember> {
        let mut seen = HashSet::new();
        members
            .iter()
            .filter(|member| {
                seen.insert(member.peer_id) && member.label.to_lowercase().contains(&self.query)
            })
            .collect()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn key(
        &mut self,
        key: AutocompleteKey,
        members: &[MentionMember],
    ) -> Option<MentionMember> {
        if key == AutocompleteKey::Escape {
            self.close();
            return None;
        }
        let suggestions = self.suggestions(members);
        if suggestions.is_empty() {
            return None;
        }
        match key {
            AutocompleteKey::Up => {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(suggestions.len() - 1)
            }
            AutocompleteKey::Down => self.selected = (self.selected + 1) % suggestions.len(),
            AutocompleteKey::Enter => return Some((*suggestions[self.selected]).clone()),
            AutocompleteKey::Escape => unreachable!(),
        }
        None
    }

    /// Mouse selection uses the same valid, deduplicated candidate list as keys.
    pub fn click(&mut self, index: usize, members: &[MentionMember]) -> Option<MentionMember> {
        let suggestions = self.suggestions(members);
        let selected = (*suggestions.get(index)?).clone();
        self.selected = index;
        Some(selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn structured_mentions_survive_duplicate_and_renamed_labels() {
        let mention = Mention::new(id(1), "old-name", 0, 9);
        let members = [
            MentionMember::new(id(1), "renamed"),
            MentionMember::new(id(2), "old-name"),
        ];
        assert!(mentions_local("@old-name", &[mention], &members, &id(1)));
        assert!(!fallback_target("@old-name", &members, &id(1)));
    }

    #[test]
    fn autocomplete_only_returns_room_members_and_handles_keys_mouse_escape() {
        let members = [
            MentionMember::new(id(1), "Alice"),
            MentionMember::new(id(2), "Al"),
            MentionMember::new(id(1), "Alice"),
        ];
        let mut state = Autocomplete::default();
        state.update("hello @a", 8, &members);
        assert_eq!(state.suggestions(&members).len(), 2);
        assert_eq!(state.key(AutocompleteKey::Down, &members), None);
        assert_eq!(state.click(1, &members).unwrap().peer_id, id(2));
        state.key(AutocompleteKey::Escape, &members);
        assert!(!state.is_open());
    }

    #[test]
    fn unread_detection_uses_peer_id_not_changed_display_name() {
        let mention = Mention::new(id(7), "before-rename", 0, 14);
        assert!(mentions_local("@after-rename", &[mention], &[], &id(7)));
        assert!(!mentions_local(
            "@after-rename",
            &[],
            &[MentionMember::new(id(8), "after-rename")],
            &id(7)
        ));
    }
}
