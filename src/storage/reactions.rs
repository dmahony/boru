//! Durable reaction projection and tombstone storage.

use super::*;
use crate::reactions::{ReactionEvent, ReactionOp, ReactionState};

impl Storage {
    /// Apply an authenticated reaction event. A remove wins permanently for
    /// its `(message, actor, emoji)` key, making retries and reordering safe.
    pub fn apply_reaction_event(&self, event: &ReactionEvent, updated_at_ms: u64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = matches!(event.op, ReactionOp::Remove) as i64;
        let changed = conn.execute(
            "INSERT INTO reaction_events
                (message_id, actor, emoji, removed, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(message_id, actor, emoji) DO UPDATE SET
                removed = 1,
                updated_at_ms = MAX(reaction_events.updated_at_ms, excluded.updated_at_ms)
             WHERE reaction_events.removed = 0 AND excluded.removed = 1",
            params![
                event.message_id.as_slice(),
                event.actor.as_slice(),
                event.emoji,
                removed,
                updated_at_ms as i64
            ],
        )
        .std_context("apply reaction event")?;
        Ok(changed > 0)
    }

    /// Rebuild the deterministic reaction projection after restart.
    pub fn load_reaction_state(&self) -> Result<ReactionState> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT message_id, actor, emoji, removed
                 FROM reaction_events ORDER BY message_id, actor, emoji",
            )
            .std_context("prepare reaction state")?;
        let rows = stmt
            .query_map([], |row| {
                let message_id: [u8; 32] = row
                    .get::<_, Vec<u8>>(0)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let actor: [u8; 32] = row
                    .get::<_, Vec<u8>>(1)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok((message_id, actor, row.get::<_, String>(2)?, row.get::<_, i64>(3)? != 0))
            })
            .std_context("query reaction state")?;
        let mut state = ReactionState::default();
        for row in rows {
            let (message_id, actor, emoji, removed) = row.std_context("read reaction row")?;
            let event = if removed {
                ReactionEvent::remove(message_id, actor, emoji)
            } else {
                ReactionEvent::add(message_id, actor, emoji)
            };
            state.apply(event);
        }
        Ok(state)
    }
}
