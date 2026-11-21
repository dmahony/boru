//! Deterministic, opaque cursor pagination for snapshot message history.
use crate::snapshot::{MessageHistoryCursor, MessageHistoryItem, MessageHistoryPage};

const CURSOR_PREFIX: &str = "mh1.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    timestamp_ms: u64,
    id: u64,
}

fn encode(position: Position) -> MessageHistoryCursor {
    let payload = format!("{}:{}", position.timestamp_ms, position.id);
    let digest = blake3::hash(payload.as_bytes());
    MessageHistoryCursor::new(format!(
        "{CURSOR_PREFIX}{}:{}",
        hex::encode(payload),
        digest.to_hex()
    ))
}

fn decode(cursor: &MessageHistoryCursor) -> Result<Position, &'static str> {
    let value = cursor
        .as_str()
        .strip_prefix(CURSOR_PREFIX)
        .ok_or("unsupported cursor")?;
    let (payload_hex, digest_hex) = value.split_once(':').ok_or("malformed cursor")?;
    let payload = hex::decode(payload_hex).map_err(|_| "malformed cursor")?;
    let digest = hex::decode(digest_hex).map_err(|_| "malformed cursor")?;
    if digest.as_slice() != blake3::hash(&payload).as_bytes() {
        return Err("cursor checksum mismatch");
    }
    let payload = std::str::from_utf8(&payload).map_err(|_| "malformed cursor")?;
    let (timestamp, id) = payload.split_once(':').ok_or("malformed cursor")?;
    Ok(Position {
        timestamp_ms: timestamp.parse().map_err(|_| "malformed cursor")?,
        id: id.parse().map_err(|_| "malformed cursor")?,
    })
}

/// Return a chronological page. Items are ordered by `(timestamp_ms, id)`;
/// `id` must be a decimal unsigned integer so the cursor contains no storage token.
pub fn paginate(
    mut items: Vec<MessageHistoryItem>,
    cursor: Option<&MessageHistoryCursor>,
    limit: usize,
) -> Result<MessageHistoryPage, &'static str> {
    if limit == 0 {
        return Err("page limit must be nonzero");
    }
    let after = cursor.map(decode).transpose()?;
    let mut keyed = items
        .drain(..)
        .map(|item| {
            let id = item
                .id
                .parse::<u64>()
                .map_err(|_| "message id must be a decimal unsigned integer")?;
            Ok((
                Position {
                    timestamp_ms: item.timestamp_ms,
                    id,
                },
                item,
            ))
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    keyed.sort_by_key(|(position, _)| *position);
    let mut remaining = keyed
        .into_iter()
        .filter(|(position, _)| after.map_or(true, |after| *position > after));
    let mut selected = Vec::with_capacity(limit.saturating_add(1));
    selected.extend(remaining.by_ref().take(limit.saturating_add(1)));
    let has_more = selected.len() > limit;
    if has_more {
        selected.pop();
    }
    let next_cursor = if has_more {
        selected.last().map(|(position, _)| encode(*position))
    } else {
        None
    };
    Ok(MessageHistoryPage {
        schema_version: crate::snapshot::SNAPSHOT_SCHEMA_VERSION,
        items: selected.into_iter().map(|(_, item)| item).collect(),
        next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixtures() -> Vec<MessageHistoryItem> {
        [(4, "4"), (1, "1"), (2, "2"), (2, "3"), (3, "5")]
            .into_iter()
            .map(|(timestamp_ms, id)| MessageHistoryItem {
                id: id.into(),
                conversation_id: "c".into(),
                sender_id: "s".into(),
                timestamp_ms,
                kind: "text".into(),
                body: id.into(),
            })
            .collect()
    }
    #[test]
    fn cursor_round_trip_and_pages_are_stable() {
        let first = paginate(fixtures(), None, 2).unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        let cursor = first.next_cursor.unwrap();
        assert_eq!(
            decode(&cursor).unwrap(),
            Position {
                timestamp_ms: 2,
                id: 2
            }
        );
        let middle = paginate(fixtures(), Some(&cursor), 2).unwrap();
        assert_eq!(
            middle
                .items
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            ["3", "5"]
        );
        let final_page = paginate(fixtures(), middle.next_cursor.as_ref(), 2).unwrap();
        assert_eq!(
            final_page
                .items
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            ["4"]
        );
        assert!(final_page.next_cursor.is_none());
        assert!(paginate(vec![], None, 2).unwrap().items.is_empty());
    }
    #[test]
    fn fixture_walk_is_stable_complete_and_handles_edges() {
        let items: Vec<MessageHistoryItem> = serde_json::from_str(include_str!(
            "../tests/fixtures/message_history_pagination.json"
        ))
        .unwrap();
        let first = paginate(items.clone(), None, 2).unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2"]
        );
        let cursor = first.next_cursor.clone().unwrap();
        let middle = paginate(items.clone(), Some(&cursor), 2).unwrap();
        assert_eq!(middle, paginate(items.clone(), Some(&cursor), 2).unwrap());
        assert_eq!(
            middle
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["3", "4"]
        );
        assert!(middle.next_cursor.is_none());
        assert!(paginate(vec![], None, 2).unwrap().items.is_empty());

        let mut seen = Vec::new();
        let mut next = None;
        loop {
            let page = paginate(items.clone(), next.as_ref(), 1).unwrap();
            seen.extend(page.items.into_iter().map(|item| item.id));
            next = page.next_cursor;
            if next.is_none() {
                break;
            }
        }
        assert_eq!(seen, ["1", "2", "3", "4"]);
        assert_eq!(
            seen.iter().collect::<std::collections::HashSet<_>>().len(),
            4
        );
    }

    #[test]
    fn rejects_tampered_and_malformed_cursors() {
        let cursor = encode(Position {
            timestamp_ms: 2,
            id: 3,
        });
        let mut bytes = cursor.as_str().as_bytes().to_vec();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'0' { b'1' } else { b'0' };
        let tampered = MessageHistoryCursor::new(String::from_utf8(bytes).unwrap());
        assert!(decode(&tampered).is_err());
        assert!(paginate(
            fixtures(),
            Some(&MessageHistoryCursor::new("bad".into())),
            2
        )
        .is_err());
    }
}
