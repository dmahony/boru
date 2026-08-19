//! Local-only full-text search over decrypted, user-visible message metadata.
//!
//! Search is deliberately implemented entirely inside SQLite. No query or
//! result is sent to a peer, and attachment bytes are never indexed.

use super::*;

/// Optional restrictions for a local search query.
#[allow(missing_docs)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalSearchFilter {
    pub topic: Option<TopicId>,
    pub sender: Option<[u8; 32]>,
    pub after_ms: Option<u64>,
    pub before_ms: Option<u64>,
    /// Message kind (`text`, `file`, `image`, ...).
    pub kind: Option<String>,
}

/// A single local search hit.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSearchHit {
    pub id: i64,
    pub msg_hash: [u8; 32],
    pub topic: TopicId,
    pub sender: [u8; 32],
    pub timestamp_ms: u64,
    pub kind: String,
    pub body: String,
    pub filename: Option<String>,
}

/// A bounded page of search results.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSearchPage {
    pub hits: Vec<LocalSearchHit>,
    pub next_offset: Option<u64>,
}

impl super::Storage {
    /// Delete a locally persisted chat message and its search projection.
    pub fn delete_chat_message(&self, msg_hash: &[u8; 32]) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let id: Option<i64> = conn
            .query_row("SELECT id FROM chat_messages WHERE msg_hash=?1", [msg_hash.as_slice()], |row| row.get(0))
            .optional()
            .std_context("find chat message for deletion")?;
        let Some(id) = id else { return Ok(false); };
        conn.execute("DELETE FROM chat_messages_fts WHERE rowid=?1", [id])
            .std_context("delete local FTS row")?;
        conn.execute("DELETE FROM chat_messages WHERE id=?1", [id])
            .std_context("delete chat message")?;
        Ok(true)
    }

    /// Search local decrypted projections using SQLite FTS5.
    pub fn search_local(
        &self,
        query: &str,
        filter: &LocalSearchFilter,
        offset: u64,
        limit: u32,
    ) -> Result<LocalSearchPage> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(LocalSearchPage { hits: Vec::new(), next_offset: None });
        }
        let limit = limit.clamp(1, 200) as i64;
        let fts_query = fts_query(query);
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT c.id,c.msg_hash,c.topic,c.sender,c.timestamp_ms,c.search_kind,
                    c.search_body,c.search_filename
             FROM chat_messages_fts f
             JOIN chat_messages c ON c.id=f.rowid
             WHERE chat_messages_fts MATCH ?1",
        );
        sql.push_str(
            " AND (?2 IS NULL OR c.topic=?2)
              AND (?3 IS NULL OR c.sender=?3)
              AND (?4 IS NULL OR c.timestamp_ms>=?4)
              AND (?5 IS NULL OR c.timestamp_ms<=?5)
              AND (?6 IS NULL OR c.search_kind=?6)
              ORDER BY c.timestamp_ms DESC, c.id DESC LIMIT ?7 OFFSET ?8",
        );

        // Keep binding positional and deterministic while allowing omitted filters.
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(fts_query)];
        values.push(Box::new(filter.topic.as_ref().map(|t| t.as_bytes().to_vec())));
        values.push(Box::new(filter.sender.map(|s| s.to_vec())));
        values.push(Box::new(filter.after_ms.map(|v| v as i64)));
        values.push(Box::new(filter.before_ms.map(|v| v as i64)));
        values.push(Box::new(filter.kind.clone()));
        values.push(Box::new(limit + 1));
        values.push(Box::new(offset as i64));
        let refs: Vec<&dyn rusqlite::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let mut stmt = conn.prepare(&sql).std_context("prepare local search")?;
        let rows = stmt.query_map(refs.as_slice(), |row| {
            let hash: Vec<u8> = row.get(1)?;
            let topic: Vec<u8> = row.get(2)?;
            let sender: Vec<u8> = row.get(3)?;
            Ok(LocalSearchHit {
                id: row.get(0)?,
                msg_hash: hash.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                topic: TopicId::from_bytes(topic.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?),
                sender: sender.try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                timestamp_ms: row.get::<_, i64>(4)? as u64,
                kind: row.get(5)?,
                body: row.get(6)?,
                filename: row.get(7)?,
            })
        }).std_context("query local search")?;
        let mut hits = Vec::new();
        for row in rows { hits.push(row.std_context("read local search hit")?); }
        let next_offset = (hits.len() > limit as usize).then(|| offset + limit as u64);
        hits.truncate(limit as usize);
        Ok(LocalSearchPage { hits, next_offset })
    }

    /// Rebuild all searchable projections and the FTS index from signed rows.
    /// Invalid/legacy non-chat rows are retained but excluded from the index.
    pub fn rebuild_local_search_index(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM chat_messages_fts", []).std_context("clear local FTS")?;
        conn.execute("UPDATE chat_messages SET search_kind='',search_body='',search_filename=NULL", [])
            .std_context("clear local search projections")?;
        let mut stmt = conn.prepare("SELECT id,signed_bytes FROM chat_messages").std_context("prepare local search rebuild")?;
        let rows: Vec<(i64, Vec<u8>)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .std_context("query local search rebuild")?.collect::<rusqlite::Result<_>>()
            .std_context("read local search rebuild")?;
        drop(stmt);
        let mut indexed = 0;
        for (id, bytes) in rows {
            let Ok((_from, message, _sent_at)) = crate::chat_core::SignedMessage::verify_and_decode(&bytes) else { continue; };
            let (kind, body, filename) = searchable_message(&message);
            if body.is_empty() && filename.is_none() { continue; }
            conn.execute("UPDATE chat_messages SET search_kind=?1,search_body=?2,search_filename=?3 WHERE id=?4", params![kind, body, filename, id])
                .std_context("write local search projection")?;
            conn.execute("INSERT INTO chat_messages_fts(rowid,search_kind,search_body,search_filename) VALUES (?1,?2,?3,?4)", params![id, kind, body, filename])
                .std_context("write local FTS row")?;
            indexed += 1;
        }
        Ok(indexed)
    }
}

pub(crate) fn searchable_message(message: &crate::chat_core::Message) -> (&'static str, String, Option<String>) {
    use crate::chat_core::Message;
    match message {
        Message::Message { text } => ("text", text.clone(), None),
        Message::Edit { new_text, .. } => ("edit", new_text.clone(), None),
        Message::FileShare { name, .. } => ("file", String::new(), Some(name.clone())),
        Message::ImageShare { name, .. } => ("image", String::new(), Some(name.clone())),
        Message::AboutMe { name, .. } => ("profile", name.clone(), None),
        _ => ("", String::new(), None),
    }
}

/// Treat each input word as a literal FTS5 token, avoiding query syntax and
/// making punctuation/case-safe searches predictable.
fn fts_query(query: &str) -> String {
    query.split_whitespace()
        .filter(|part| !part.is_empty())
        .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
        .collect::<Vec<_>>().join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_core::{Message, SignedMessage};
    use iroh::SecretKey;

    fn add(storage: &Storage, text: &str, timestamp: u64) {
        let key = SecretKey::generate();
        let bytes = SignedMessage::sign_and_encode(&key, &Message::Message { text: text.into() }).unwrap();
        storage.insert_chat_message(&[timestamp as u8; 32], &TopicId::from_bytes([1; 32]), key.public().as_bytes(), timestamp, &bytes).unwrap();
    }

    #[test]
    fn unicode_punctuation_case_and_pagination_are_local() {
        let storage = Storage::memory().unwrap();
        add(&storage, "Hello, café 🦀", 1);
        add(&storage, "hello world", 2);
        let page = storage.search_local("CAFÉ", &Default::default(), 0, 10).unwrap();
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].body, "Hello, café 🦀");
        let page = storage.search_local("hello", &Default::default(), 0, 1).unwrap();
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.next_offset, Some(1));
        assert_eq!(
            storage
                .search_local("absent", &Default::default(), 0, 10)
                .unwrap()
                .hits
                .len(),
            0
        );
        assert!(storage.delete_chat_message(&[1; 32]).unwrap());
        assert_eq!(
            storage
                .search_local("café", &Default::default(), 0, 10)
                .unwrap()
                .hits
                .len(),
            0
        );
    }
}
