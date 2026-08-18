//! Conversation persistence — groups, direct-message (DM) envelopes/outbox,
//! the inbox/outbox delivery queues, outgoing chat messages, and chat-message
//! history.
//!
//! Each method is an `impl super::Storage` accessor over the shared SQLite
//! connection; no format or protocol changes live here (structural split
//! only, BORU-CORE-001).

use super::*;

impl super::Storage {
    /// Create a group, leaving creation idempotent by group id.
    pub fn create_group(&self, group: &GroupRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT OR IGNORE INTO groups (group_id,name,description,owner_public_key,current_epoch,created_at_ms,updated_at_ms,archived) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", params![group.group_id.as_slice(), group.name, group.description, group.owner_public_key, group.current_epoch as i64, group.created_at_ms as i64, group.updated_at_ms as i64, group.archived as i64]).std_context("create group")?;
        Ok(())
    }
    /// Fetch one group by id.
    pub fn get_group(&self, group_id: &[u8; 32]) -> Result<Option<GroupRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT group_id,name,description,owner_public_key,current_epoch,created_at_ms,updated_at_ms,archived FROM groups WHERE group_id=?1", [group_id.as_slice()], |r| {
            let b: Vec<u8> = r.get(0)?; let mut id=[0u8;32]; id.copy_from_slice(&b);
            Ok(GroupRow { group_id:id, name:r.get(1)?, description:r.get(2)?, owner_public_key:r.get(3)?, current_epoch:r.get::<_,i64>(4)? as u64, created_at_ms:r.get::<_,i64>(5)? as u64, updated_at_ms:r.get::<_,i64>(6)? as u64, archived:r.get::<_,i64>(7)? != 0 })
        }).optional().std_context("get group")
    }
    /// List groups, optionally including archived groups.
    pub fn list_groups(&self, include_archived: bool) -> Result<Vec<GroupRow>> {
        let conn = self.conn.lock().unwrap();
        let mut st=conn.prepare("SELECT group_id,name,description,owner_public_key,current_epoch,created_at_ms,updated_at_ms,archived FROM groups WHERE (?1 OR archived=0) ORDER BY updated_at_ms DESC").std_context("prepare group query")?;
        let rows = st
            .query_map([include_archived as i64], |r| {
                let b: Vec<u8> = r.get(0)?;
                let mut id = [0u8; 32];
                id.copy_from_slice(&b);
                Ok(GroupRow {
                    group_id: id,
                    name: r.get(1)?,
                    description: r.get(2)?,
                    owner_public_key: r.get(3)?,
                    current_epoch: r.get::<_, i64>(4)? as u64,
                    created_at_ms: r.get::<_, i64>(5)? as u64,
                    updated_at_ms: r.get::<_, i64>(6)? as u64,
                    archived: r.get::<_, i64>(7)? != 0,
                })
            })
            .std_context("query group rows")?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .std_context("collect group rows")?)
    }
    /// Update group display metadata and archive state.
    pub fn update_group_metadata(
        &self,
        group_id: &[u8; 32],
        name: &str,
        description: &str,
        updated_at_ms: u64,
        archived: bool,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("UPDATE groups SET name=?1,description=?2,updated_at_ms=?3,archived=?4 WHERE group_id=?5",params![name,description,updated_at_ms as i64,archived as i64,group_id.as_slice()]).std_context("update group metadata")? != 0)
    }
    /// Insert or replace a membership row (safe for retries).
    pub fn add_group_member(&self, member: &GroupMemberRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO group_members (group_id,public_key,role,joined_at_ms,invited_by,epoch_joined,state) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(group_id,public_key) DO UPDATE SET role=excluded.role,invited_by=excluded.invited_by,epoch_joined=excluded.epoch_joined,state=excluded.state",params![member.group_id.as_slice(),member.public_key,member.role,member.joined_at_ms as i64,member.invited_by,member.epoch_joined as i64,member.state]).std_context("add group member")?;
        Ok(())
    }
    /// Update an existing member's role/state.
    pub fn update_group_member(
        &self,
        group_id: &[u8; 32],
        public_key: &[u8],
        role: &str,
        state: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .execute(
                "UPDATE group_members SET role=?1,state=?2 WHERE group_id=?3 AND public_key=?4",
                params![role, state, group_id.as_slice(), public_key],
            )
            .std_context("update group member")?
            != 0)
    }
    /// Mark a member as left/removed without deleting the audit row.
    pub fn remove_group_member(
        &self,
        group_id: &[u8; 32],
        public_key: &[u8],
        state: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .execute(
                "UPDATE group_members SET state=?1 WHERE group_id=?2 AND public_key=?3",
                params![state, group_id.as_slice(), public_key],
            )
            .std_context("remove group member")?
            != 0)
    }
    /// List members in deterministic key order.
    pub fn list_group_members(&self, group_id: &[u8; 32]) -> Result<Vec<GroupMemberRow>> {
        let conn = self.conn.lock().unwrap();
        let mut st=conn.prepare("SELECT group_id,public_key,role,joined_at_ms,invited_by,epoch_joined,state FROM group_members WHERE group_id=?1 ORDER BY public_key").std_context("prepare group query")?;
        let rows = st
            .query_map([group_id.as_slice()], |r| {
                let b: Vec<u8> = r.get(0)?;
                let mut id = [0u8; 32];
                id.copy_from_slice(&b);
                Ok(GroupMemberRow {
                    group_id: id,
                    public_key: r.get(1)?,
                    role: r.get(2)?,
                    joined_at_ms: r.get::<_, i64>(3)? as u64,
                    invited_by: r.get(4)?,
                    epoch_joined: r.get::<_, i64>(5)? as u64,
                    state: r.get(6)?,
                })
            })
            .std_context("query group rows")?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .std_context("collect group rows")?)
    }
    /// Insert a group epoch idempotently.
    pub fn create_group_epoch(&self, epoch: &GroupEpochRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT OR IGNORE INTO group_epochs(group_id,epoch,topic_id,discovery_secret,created_at_ms) VALUES (?1,?2,?3,?4,?5)",params![epoch.group_id.as_slice(),epoch.epoch as i64,epoch.topic_id.as_ref() as &[u8],epoch.discovery_secret,epoch.created_at_ms as i64]).std_context("create group epoch")?;
        conn.execute("UPDATE groups SET current_epoch=MAX(current_epoch,?1),updated_at_ms=MAX(updated_at_ms,?2) WHERE group_id=?3",params![epoch.epoch as i64,epoch.created_at_ms as i64,epoch.group_id.as_slice()]).std_context("update group epoch")?;
        Ok(())
    }
    /// Return the newest epoch for a group.
    pub fn get_current_group_epoch(&self, group_id: &[u8; 32]) -> Result<Option<GroupEpochRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT group_id,epoch,topic_id,discovery_secret,created_at_ms FROM group_epochs WHERE group_id=?1 ORDER BY epoch DESC LIMIT 1",[group_id.as_slice()],|r|{let b:Vec<u8>=r.get(0)?;let mut id=[0u8;32];id.copy_from_slice(&b);let t:Vec<u8>=r.get(2)?;let mut topic=[0u8;32];topic.copy_from_slice(&t);Ok(GroupEpochRow{group_id:id,epoch:r.get::<_,i64>(1)? as u64,topic_id:topic.into(),discovery_secret:r.get(3)?,created_at_ms:r.get::<_,i64>(4)? as u64})}).optional().std_context("get current group epoch")
    }
    /// Look up the group for a given epoch topic, using the unique topic→group index.
    pub fn find_group_by_topic(&self, topic_id: &TopicId) -> Result<Option<GroupRow>> {
        // Resolve the group id inside a scoped lock and DROP the guard before
        // calling `get_group` — holding the connection mutex while re-locking
        // it deadlocks (std::sync::Mutex is not reentrant).
        let group_id: Option<[u8; 32]> = {
            let conn = self.conn.lock().unwrap();
            let topic_bytes: &[u8] = topic_id.as_ref();
            conn.query_row(
                "SELECT group_id FROM group_epochs WHERE topic_id=?1 LIMIT 1",
                [topic_bytes],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()
            .std_context("find group epoch by topic")?
            .map(|bytes| {
                let mut id = [0u8; 32];
                id.copy_from_slice(&bytes);
                id
            })
        };
        match group_id {
            Some(id) => self.get_group(&id),
            None => Ok(None),
        }
    }
    /// Whether `topic` is a known public room advertised in the public-room
    /// directory (`directory_ads`).
    ///
    /// Public rooms are readable by any authenticated peer, so the backfill
    /// authorization layer treats advertised topics as open.  This is a
    /// **live** query — an advertisement that is later evicted stops
    /// authorizing the topic at request time.
    pub fn is_public_room_topic(&self, topic_id: &TopicId) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let allowed: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM directory_ads WHERE topic = ?1 LIMIT 1)",
                [topic_id.as_ref() as &[u8]],
                |row| row.get(0),
            )
            .std_context("is public room topic")?;
        Ok(allowed)
    }
    /// Insert an invitation idempotently.
    pub fn create_group_invite(&self, invite: &GroupInviteRow) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        // Ensure the groups row exists so the foreign-key constraint on
        // group_invites.group_id is satisfied.  The invitee may not yet
        // have a local groups row.  Use INSERT OR IGNORE — if the row
        // already exists (e.g. inviter side), this is a no-op.
        conn.execute(
            "INSERT OR IGNORE INTO groups(group_id,name,description,owner_public_key,current_epoch,created_at_ms,updated_at_ms) VALUES (?1,?2,'',?3,?4,?5,?5)",
            params![
                invite.group_id.as_slice(),
                "",                          // name filled in later when joined
                invite.inviter_public_key,   // proxy for owner
                invite.epoch as i64,
                invite.created_at_ms as i64,
            ],
        )
        .std_context("ensure groups row for invite")?;
        conn.execute("INSERT OR IGNORE INTO group_invites(invite_id,group_id,inviter_public_key,recipient_public_key,epoch,status,created_at_ms,expires_at_ms,ticket,group_name) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![invite.invite_id.as_slice(),invite.group_id.as_slice(),invite.inviter_public_key,invite.recipient_public_key,invite.epoch as i64,invite.status,invite.created_at_ms as i64,invite.expires_at_ms as i64,invite.ticket,invite.group_name]).std_context("create group invite")?;
        Ok(())
    }
    /// List pending, non-expired invitations for a recipient.
    pub fn get_pending_group_invites(
        &self,
        recipient_public_key: &[u8],
        now_ms: u64,
    ) -> Result<Vec<GroupInviteRow>> {
        let conn = self.conn.lock().unwrap();
        let mut st=conn.prepare("SELECT invite_id,group_id,inviter_public_key,recipient_public_key,epoch,status,created_at_ms,expires_at_ms,ticket,group_name FROM group_invites WHERE recipient_public_key=?1 AND status='Pending' AND expires_at_ms>?2 ORDER BY created_at_ms").std_context("prepare pending invites")?;
        let rows = st
            .query_map(params![recipient_public_key, now_ms as i64], |r| {
                let a: Vec<u8> = r.get(0)?;
                let b: Vec<u8> = r.get(1)?;
                let mut aid = [0u8; 32];
                let mut gid = [0u8; 32];
                aid.copy_from_slice(&a);
                gid.copy_from_slice(&b);
                Ok(GroupInviteRow {
                    invite_id: aid,
                    group_id: gid,
                    inviter_public_key: r.get(2)?,
                    recipient_public_key: r.get(3)?,
                    epoch: r.get::<_, i64>(4)? as u64,
                    status: r.get(5)?,
                    created_at_ms: r.get::<_, i64>(6)? as u64,
                    expires_at_ms: r.get::<_, i64>(7)? as u64,
                    ticket: r.get(8)?,
                    group_name: r.get(9)?,
                })
            })
            .std_context("query group rows")?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .std_context("collect group rows")?)
    }
    /// Update invitation state; repeated updates to the same state are harmless.
    pub fn update_group_invite_state(&self, invite_id: &[u8; 32], state: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .execute(
                "UPDATE group_invites SET status=?1 WHERE invite_id=?2",
                params![state, invite_id.as_slice()],
            )
            .std_context("update group invite")?
            != 0)
    }
    /// Atomically create and queue an outgoing direct message.
    pub fn queue_outgoing_dm(
        &self,
        conversation_id: [u8; 32],
        sender: PublicKey,
        request_key: &str,
        plaintext: &str,
        recipient: MailboxPublicKey,
        sender_secret: &SecretKey,
    ) -> Result<OutgoingDm> {
        self.queue_outgoing_dm_inner(
            conversation_id,
            sender,
            request_key,
            plaintext,
            recipient,
            sender_secret,
            None,
        )
    }
    /// Queue an outgoing DM while injecting a deterministic failure.
    #[expect(clippy::too_many_arguments)]
    pub fn queue_outgoing_dm_with_fault(
        &self,
        conversation_id: [u8; 32],
        sender: PublicKey,
        request_key: &str,
        plaintext: &str,
        recipient: MailboxPublicKey,
        sender_secret: &SecretKey,
        fault: OutgoingDmFault,
    ) -> Result<OutgoingDm> {
        self.queue_outgoing_dm_inner(
            conversation_id,
            sender,
            request_key,
            plaintext,
            recipient,
            sender_secret,
            Some(fault),
        )
    }
    #[expect(clippy::too_many_arguments)]
    fn queue_outgoing_dm_inner(
        &self,
        conversation_id: [u8; 32],
        sender: PublicKey,
        request_key: &str,
        plaintext: &str,
        recipient: MailboxPublicKey,
        sender_secret: &SecretKey,
        fault: Option<OutgoingDmFault>,
    ) -> Result<OutgoingDm> {
        if sender != sender_secret.public() {
            return Err(anyhow!("sender does not match sender secret key").into());
        }
        if request_key.is_empty() {
            return Err(anyhow!("request key must not be empty").into());
        }
        let plaintext = plaintext.as_bytes().to_vec();
        let message_id = *blake3::hash(
            &[
                b"boru-chat/dm/request/v1".as_slice(),
                sender.as_bytes(),
                &conversation_id,
                request_key.as_bytes(),
            ]
            .concat(),
        )
        .as_bytes();
        let recipient_id = recipient.identity;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin outgoing dm transaction")?;
        if let Some((
            stored_id,
            stored_conversation,
            stored_sender,
            stored_recipient,
            stored_plaintext,
            stored_logical,
            stored_envelope,
        )) = tx
            .query_row(
                "SELECT m.message_id, m.conversation_id, m.sender_id, m.recipient_id,
                    m.plaintext, m.logical_message, o.envelope
             FROM dm_messages m JOIN dm_outbox o USING (message_id)
             WHERE m.request_key = ?1",
                [request_key],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                    ))
                },
            )
            .optional()
            .std_context("look up outgoing dm idempotency key")?
        {
            if stored_plaintext != plaintext
                || stored_id.as_slice() != message_id
                || stored_conversation.as_slice() != conversation_id
                || stored_sender.as_slice() != sender.as_bytes()
                || stored_recipient.as_slice() != recipient_id.as_bytes()
            {
                return Err(anyhow!("idempotency key is already bound to another message").into());
            }
            let mut id = [0; 32];
            id.copy_from_slice(&stored_id);
            let envelope = MailboxEnvelope::decode(&stored_envelope)
                .std_context("decode stored mailbox envelope")?;
            let sequence = postcard::from_bytes::<LogicalDm>(&stored_logical)
                .std_context("decode stored logical message")?
                .sequence;
            tx.commit().std_context("commit idempotent outgoing dm")?;
            return Ok(OutgoingDm {
                message_id: id,
                sequence,
                logical_message: stored_logical.to_vec(),
                envelope,
            });
        }
        let sequence = tx.query_row("SELECT next_sequence FROM dm_sender_sequences WHERE conversation_id = ?1 AND sender_id = ?2", params![conversation_id.as_slice(), sender.as_bytes()], |row| row.get::<_, i64>(0)).optional().std_context("read outgoing dm sequence")?.unwrap_or(1) as u64;
        let unsigned = crate::protocol_signing::canonical_signed_bytes(
            LOGICAL_DM_PROTOCOL,
            LOGICAL_DM_VERSION,
            &(
                conversation_id,
                sender,
                recipient_id,
                sequence,
                message_id,
                &plaintext,
            ),
        )
        .std_context("encode logical dm")?;
        let logical = LogicalDm {
            conversation_id,
            sender,
            recipient: recipient_id,
            sequence,
            message_id,
            plaintext: plaintext.clone(),
            signature: sender_secret.sign(&unsigned).to_bytes().to_vec(),
        };
        let logical_message =
            postcard::to_stdvec(&logical).std_context("encode signed logical dm")?;
        if fault == Some(OutgoingDmFault::Encryption) {
            return Err(anyhow!("injected mailbox encryption failure").into());
        }
        let envelope = seal_for(sender_secret, recipient, &logical_message)?;
        let envelope_bytes =
            postcard::to_stdvec(&envelope).std_context("encode mailbox envelope")?;
        let now = now_ms() as i64;
        tx.execute("INSERT OR IGNORE INTO dm_conversations (conversation_id, peer_id, created_at_ms) VALUES (?1, ?2, ?3)", params![conversation_id.as_slice(), recipient_id.as_bytes(), now]).std_context("create dm conversation")?;
        tx.execute("INSERT INTO dm_sender_sequences (conversation_id, sender_id, next_sequence) VALUES (?1, ?2, ?3) ON CONFLICT(conversation_id, sender_id) DO UPDATE SET next_sequence = excluded.next_sequence", params![conversation_id.as_slice(), sender.as_bytes(), (sequence + 1) as i64]).std_context("advance dm sender sequence")?;
        tx.execute("INSERT INTO dm_messages (message_id, conversation_id, sender_id, recipient_id, sequence, request_key, plaintext, logical_message, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", params![message_id.as_slice(), conversation_id.as_slice(), sender.as_bytes(), recipient_id.as_bytes(), sequence as i64, request_key, &plaintext, &logical_message, now]).std_context("insert visible dm message")?;
        tx.execute("INSERT INTO dm_outbox (message_id, recipient_id, envelope, created_at_ms) VALUES (?1, ?2, ?3, ?4)", params![message_id.as_slice(), recipient_id.as_bytes(), &envelope_bytes, now]).std_context("insert dm outbox envelope")?;
        if fault == Some(OutgoingDmFault::Database) {
            return Err(anyhow!("injected database failure").into());
        }
        tx.commit().std_context("commit outgoing dm transaction")?;
        Ok(OutgoingDm {
            message_id,
            sequence,
            logical_message,
            envelope,
        })
    }
    #[allow(missing_docs)]
    pub fn next_dm_sequence(&self, conversation_id: [u8; 32], sender: PublicKey) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row("SELECT next_sequence FROM dm_sender_sequences WHERE conversation_id = ?1 AND sender_id = ?2", params![conversation_id.as_slice(), sender.as_bytes()], |row| row.get::<_, i64>(0)).optional().std_context("read next dm sequence")?.unwrap_or(1) as u64)
    }
    #[allow(missing_docs)]
    pub fn get_dm_message(&self, message_id: &MessageId) -> Result<Option<DmMessageRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT conversation_id, sender_id, recipient_id, sequence, request_key, plaintext, created_at_ms FROM dm_messages WHERE message_id = ?1", [message_id.as_slice()], |row| {
            let c: Vec<u8> = row.get(0)?; let mut conversation_id = [0; 32]; conversation_id.copy_from_slice(&c);
            let sender_bytes: Vec<u8> = row.get(1)?; let recipient_bytes: Vec<u8> = row.get(2)?;
            Ok(DmMessageRow { message_id: *message_id, conversation_id, sender: PublicKey::try_from(sender_bytes.as_slice()).map_err(|_| rusqlite::Error::InvalidQuery)?, recipient: PublicKey::try_from(recipient_bytes.as_slice()).map_err(|_| rusqlite::Error::InvalidQuery)?, sequence: row.get::<_, i64>(3)? as u64, request_key: row.get(4)?, plaintext: row.get(5)?, created_at_ms: row.get::<_, i64>(6)? as u64 })
        }).optional().std_context("get dm message")
    }
    /// List direct-message history using a clock-independent, deterministic
    /// order. A sender's persistent sequence is the primary key; sender and
    /// message id are stable tie-breakers for messages from different senders.
    ///
    /// `offset`/`limit` pagination is stable because this order never uses the
    /// local insertion time or a remote timestamp. Retries therefore remain a
    /// single row and cannot move an existing message in history.
    pub fn list_dm_messages(
        &self,
        conversation_id: [u8; 32],
        offset: u32,
        limit: Option<u32>,
    ) -> Result<Vec<DmMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let pagination = match limit {
            Some(n) => format!(" LIMIT {n} OFFSET {offset}"),
            None => format!(" LIMIT -1 OFFSET {offset}"),
        };
        let sql = format!(
            "SELECT message_id, sender_id, recipient_id, sequence, request_key,
                    plaintext, created_at_ms
             FROM dm_messages
             WHERE conversation_id = ?1
             ORDER BY sequence ASC, sender_id ASC, message_id ASC{}",
            pagination
        );
        let mut stmt = conn.prepare(&sql).std_context("prepare list dm messages")?;
        let mut rows = stmt
            .query([conversation_id.as_slice()])
            .std_context("query dm messages")?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().std_context("next dm message")? {
            let message_id: Vec<u8> = row.get(0).map_err(|e| anyhow!(e))?;
            let sender_bytes: Vec<u8> = row.get(1).map_err(|e| anyhow!(e))?;
            let recipient_bytes: Vec<u8> = row.get(2).map_err(|e| anyhow!(e))?;
            let conversation_id_bytes = conversation_id;
            result.push(DmMessageRow {
                message_id: message_id
                    .try_into()
                    .map_err(|_| anyhow!("invalid stored dm message id"))?,
                conversation_id: conversation_id_bytes,
                sender: PublicKey::try_from(sender_bytes.as_slice())
                    .map_err(|_| anyhow!("invalid stored dm sender"))?,
                recipient: PublicKey::try_from(recipient_bytes.as_slice())
                    .map_err(|_| anyhow!("invalid stored dm recipient"))?,
                sequence: row.get::<_, i64>(3).map_err(|e| anyhow!(e))? as u64,
                request_key: row.get(4).map_err(|e| anyhow!(e))?,
                plaintext: row.get(5).map_err(|e| anyhow!(e))?,
                created_at_ms: row.get::<_, i64>(6).map_err(|e| anyhow!(e))? as u64,
            });
        }
        Ok(result)
    }
    #[allow(missing_docs)]
    pub fn get_dm_outbox(&self, message_id: &MessageId) -> Result<Option<DmOutboxRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT recipient_id, envelope FROM dm_outbox WHERE message_id = ?1",
            [message_id.as_slice()],
            |row| {
                let recipient_bytes: Vec<u8> = row.get(0)?;
                let envelope_bytes: Vec<u8> = row.get(1)?;
                Ok(DmOutboxRow {
                    message_id: *message_id,
                    recipient: PublicKey::try_from(recipient_bytes.as_slice())
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    envelope: postcard::from_bytes(&envelope_bytes)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            },
        )
        .optional()
        .std_context("get dm outbox")
    }
    /// Process an acknowledgement from a recipient without exposing any
    /// partially-applied state.  The acknowledgement id is the stable
    /// mailbox-envelope id (not the logical DM id).
    pub fn process_outgoing_ack(&self, from: PublicKey, ack: &MailboxAck) -> Result<bool> {
        self.process_outgoing_ack_inner(from, ack, None)
    }
    /// Test-only fault injection point for acknowledgement transaction tests.
    pub fn process_outgoing_ack_with_fault(
        &self,
        from: PublicKey,
        ack: &MailboxAck,
        fault: AckProcessingFault,
    ) -> Result<bool> {
        self.process_outgoing_ack_inner(from, ack, Some(fault))
    }
    fn process_outgoing_ack_inner(
        &self,
        from: PublicKey,
        ack: &MailboxAck,
        fault: Option<AckProcessingFault>,
    ) -> Result<bool> {
        const MAX_ACK_MESSAGE_ID_LEN: usize = 128;
        if ack.message_id.len() > MAX_ACK_MESSAGE_ID_LEN {
            return Err(anyhow!("acknowledgement message id is too long").into());
        }
        // Verify the signed contract before taking the database lock.
        ack.verify(from)?;
        let id_bytes = hex::decode(&ack.message_id)
            .map_err(|e| anyhow!("invalid acknowledgement message id: {e}"))?;
        if id_bytes.len() != 32 {
            return Err(anyhow!("acknowledgement message id must be 32 bytes").into());
        }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin acknowledgement transaction")?;

        // Duplicate valid acknowledgements are harmless, including after the
        // sender has already removed its outbox row.
        let already_recorded: bool = tx
            .query_row(
                "SELECT 1 FROM dm_acknowledgements WHERE message_id = ?1",
                [id_bytes.as_slice()],
                |_| Ok(true),
            )
            .optional()
            .std_context("look up acknowledgement")?
            .unwrap_or(false);
        if already_recorded {
            tx.commit()
                .std_context("commit duplicate acknowledgement")?;
            return Ok(false);
        }

        // Find the outbox row by the mailbox envelope's stable id.  This
        // prevents an acknowledgement for a different envelope from being
        // attached to a merely similar logical message.
        let mut stmt = tx
            .prepare(
                "SELECT m.message_id, m.sender_id, m.recipient_id, o.recipient_id,
                        o.envelope
                 FROM dm_messages m
                 JOIN dm_outbox o ON o.message_id = m.message_id
                 WHERE m.acknowledged_at_ms IS NULL",
            )
            .std_context("prepare acknowledgement message lookup")?;
        let mut rows = stmt
            .query([])
            .std_context("query acknowledgement message lookup")?;
        let mut matched: Option<AckMatchRow> = None;
        while let Some(row) = rows.next().std_context("next acknowledgement row")? {
            let logical_id: Vec<u8> = row
                .get(0)
                .std_context("get stored acknowledgement message id")?;
            let envelope_bytes: Vec<u8> = row
                .get(4)
                .std_context("get stored acknowledgement envelope")?;
            let envelope = MailboxEnvelope::decode(&envelope_bytes)
                .std_context("decode stored acknowledgement envelope")?;
            if envelope.message_id().as_bytes() == ack.message_id.as_bytes() {
                let stored_sender: Vec<u8> =
                    row.get(1).std_context("get acknowledgement sender")?;
                let stored_recipient: Vec<u8> =
                    row.get(2).std_context("get acknowledgement recipient")?;
                let outbox_recipient: Vec<u8> = row.get(3).std_context("get outbox recipient")?;
                matched = Some((
                    logical_id
                        .try_into()
                        .map_err(|_| anyhow!("invalid stored message id"))?,
                    stored_sender,
                    stored_recipient,
                    outbox_recipient,
                ));
                break;
            }
        }
        drop(rows);
        drop(stmt);
        let Some((logical_id, sender_id, message_recipient, outbox_recipient)) = matched else {
            return Err(anyhow!("acknowledgement refers to an unknown message").into());
        };
        if sender_id.as_slice() != ack.original_sender.as_bytes() {
            return Err(anyhow!("acknowledgement original sender mismatch").into());
        }
        if message_recipient != outbox_recipient || message_recipient.as_slice() != from.as_bytes()
        {
            return Err(anyhow!("acknowledgement recipient mismatch").into());
        }

        let acked_at = ack.acknowledged_at_ms as i64;
        tx.execute(
            "INSERT INTO dm_acknowledgements
             (message_id, original_sender_id, recipient_id, acknowledged_at_ms, status, signature)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id_bytes.as_slice(),
                ack.original_sender.as_bytes(),
                ack.recipient.as_bytes(),
                acked_at,
                ack.status.as_deref(),
                ack.signature.as_slice(),
            ],
        )
        .std_context("insert acknowledgement")?;
        tx.execute(
            "UPDATE dm_messages SET acknowledged_at_ms = ?1 WHERE message_id = ?2",
            params![acked_at, &logical_id],
        )
        .std_context("mark message acknowledged")?;
        tx.execute("DELETE FROM dm_outbox WHERE message_id = ?1", [&logical_id])
            .std_context("remove acknowledged outbox entry")?;
        if fault == Some(AckProcessingFault::Database) {
            return Err(anyhow!("injected acknowledgement database failure").into());
        }
        tx.commit()
            .std_context("commit acknowledgement transaction")?;
        Ok(true)
    }
    /// Return whether a logical DM has been acknowledged.
    pub fn dm_acknowledged(&self, message_id: &MessageId) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT acknowledged_at_ms IS NOT NULL FROM dm_messages WHERE message_id = ?1",
                [message_id.as_slice()],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .std_context("check dm acknowledgement")?
            .unwrap_or(false))
    }
    /// Query pending outbound envelopes addressed to a specific recipient,
    /// bounded by count and total encoded size, ordered by creation time.
    ///
    /// Returns (envelopes, has_more). When the returned page is empty, has_more
    /// is always false. The caller uses the last envelope's `created_at` as a
    /// continuation cursor for the next page.
    ///
    /// Validation and replay protection:
    /// - Expired envelopes (older than DEFAULT_MAILBOX_TTL) are excluded
    /// - Already-served message IDs (via record_sync_served) are excluded
    /// - The requester-supplied since_ms is used for cursor-based pagination
    pub fn query_pending_outbound_for_recipient(
        &self,
        recipient: &PublicKey,
        since_ms: u64,
        max_count: usize,
        max_bytes: usize,
    ) -> Result<(Vec<MailboxEnvelope>, bool)> {
        let conn = self.conn.lock().unwrap();
        let now = now_ms();
        let ttl_ms = crate::mailbox::DEFAULT_MAILBOX_TTL.as_millis() as u64;
        let expiry_cutoff = now.saturating_sub(ttl_ms);
        let effective_since = since_ms.max(expiry_cutoff);
        let mut stmt = conn
            .prepare(
                "SELECT o.message_id, o.envelope, o.created_at_ms
                 FROM dm_outbox o
                 WHERE o.recipient_id = ?1
                   AND o.created_at_ms >= ?2
                   AND o.created_at_ms >= ?4
                   AND NOT EXISTS (
                       SELECT 1 FROM sync_dedup d
                       WHERE d.message_id = o.message_id
                         AND d.recipient_id = o.recipient_id
                   )
                 ORDER BY o.created_at_ms ASC, o.message_id ASC
                 LIMIT ?3",
            )
            .std_context("prepare query_pending_outbound_for_recipient")?;
        // Query max_count + 1 to detect has_more
        let limit = (max_count + 1) as i64;
        let mut rows = stmt
            .query(params![
                recipient.as_bytes(),
                effective_since as i64,
                limit,
                expiry_cutoff as i64
            ])
            .std_context("query pending outbound")?;

        let mut envelopes = Vec::with_capacity(max_count);
        let mut total_bytes = 0usize;
        let mut has_extra = false;

        while let Some(row) = rows.next().std_context("next outbound row")? {
            let _message_id_blob: Vec<u8> = row.get(0).std_context("get message_id")?;
            let envelope_bytes: Vec<u8> = row.get(1).std_context("get envelope bytes")?;
            let _created_at_ms: i64 = row.get(2).std_context("get created_at_ms")?;

            // If we already have a full page, just note there's an extra row
            if envelopes.len() >= max_count {
                has_extra = true;
                continue;
            }

            let envelope =
                MailboxEnvelope::decode(&envelope_bytes).std_context("decode envelope")?;
            let encoded_size = envelope_bytes.len();

            // Check size bound
            if total_bytes.saturating_add(encoded_size) > max_bytes && !envelopes.is_empty() {
                has_extra = true;
                continue;
            }

            total_bytes += encoded_size;
            envelopes.push(envelope);
        }

        let has_more = has_extra;
        Ok((envelopes, has_more))
    }
    /// Record that a set of message IDs were served via SyncResponse to a
    /// specific recipient.  Subsequent sync requests from the same recipient
    /// will exclude these envelopes, providing replay protection.
    pub fn record_sync_served(
        &self,
        recipient: &PublicKey,
        message_ids: &[[u8; 32]],
    ) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let now = now_ms() as i64;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .std_context("begin sync_dedup transaction")?;
        let mut stmt = tx
            .prepare(
                "INSERT OR IGNORE INTO sync_dedup (message_id, recipient_id, served_at_ms)
                 VALUES (?1, ?2, ?3)",
            )
            .std_context("prepare sync_dedup insert")?;
        for msg_id in message_ids {
            stmt.execute(params![msg_id.as_slice(), recipient.as_bytes(), now])
                .std_context("insert sync_dedup")?;
        }
        drop(stmt);
        tx.commit().std_context("commit sync_dedup transaction")?;
        Ok(())
    }
    /// Remove sync dedup entries older than the retention window.  Call this
    /// periodically or during startup to keep the sync_dedup table from growing
    /// unboundedly as old envelopes expire and are naturally excluded.
    pub fn prune_sync_dedup(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let ttl_ms = crate::mailbox::DEFAULT_MAILBOX_TTL.as_millis() as u64;
        let cutoff = (now_ms() as i64).saturating_sub(ttl_ms as i64);
        conn.execute(
            "DELETE FROM sync_dedup WHERE served_at_ms < ?1",
            params![cutoff],
        )
        .std_context("prune sync_dedup")?;
        Ok(())
    }
    /// Idempotent insert into inbox.
    pub fn insert_inbox(&self, env: &StoredEnvelope) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let acked = env.acked_at_ms.map(|v| v as i64);
        conn.execute(
            "INSERT INTO inbox (
                msg_id, conversation_id, author_user_id, author_device_id,
                created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(msg_id) DO NOTHING",
            params![
                env.msg_id.as_slice(),
                env.conversation_id.as_slice(),
                env.author_user_id.as_bytes(),
                env.author_device_id.as_bytes(),
                env.created_at_ms as i64,
                env.expires_at_ms as i64,
                env.ciphertext.as_ref(),
                env.signature.as_slice(),
                acked,
            ],
        )
        .std_context("insert inbox")?;
        Ok(())
    }
    /// Retrieve an inbox message by id.
    pub fn get_inbox(&self, msg_id: &MessageId) -> Result<Option<StoredEnvelope>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id, author_user_id, author_device_id,
                        created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
                 FROM inbox WHERE msg_id = ?1",
            )
            .std_context("prepare get_inbox")?;
        let mut rows = stmt
            .query([msg_id.as_slice()])
            .std_context("query get_inbox")?;
        if let Some(row) = rows.next().std_context("next row")? {
            Ok(Some(row_to_envelope(msg_id, row)?))
        } else {
            Ok(None)
        }
    }
    /// List all inbox messages (with optional limit).
    pub fn list_inbox(&self, limit: Option<u32>) -> Result<Vec<StoredEnvelope>> {
        let conn = self.conn.lock().unwrap();
        let sql = match limit {
            Some(n) => format!(
                "SELECT msg_id, conversation_id, author_user_id, author_device_id,
                        created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
                 FROM inbox ORDER BY created_at_ms DESC LIMIT {n}"
            ),
            None => String::from(
                "SELECT msg_id, conversation_id, author_user_id, author_device_id,
                        created_at_ms, expires_at_ms, ciphertext, signature, acked_at_ms
                 FROM inbox ORDER BY created_at_ms DESC",
            ),
        };
        let mut stmt = conn.prepare(&sql).std_context("prepare list_inbox")?;
        let mut rows = stmt.query([]).std_context("query list_inbox")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            let msg_id_blob: Vec<u8> = row.get(0).std_context("get msg_id")?;
            let mut msg_id = [0u8; 32];
            msg_id.copy_from_slice(&msg_id_blob);
            results.push(row_to_envelope_bare(&msg_id, row)?);
        }
        Ok(results)
    }
    /// Make one pending outbox row due immediately (manual retry).
    pub fn retry_outbox_now(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        now_ms: u64,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE outbox SET next_attempt_at_ms = ?1 WHERE msg_id = ?2 AND recipient_device_id = ?3 AND status != ?4 AND status != ?5",
            params![now_ms as i64, msg_id.as_slice(), recipient_device_id.as_bytes(), DeliveryStatus::Acked as u8, DeliveryStatus::Expired as u8],
        ).std_context("retry outbox now")?;
        Ok(changed)
    }
    /// Make all non-terminal messages for a newly discovered peer due now.
    pub fn wake_outbox_for_peer(
        &self,
        recipient_device_id: iroh::PublicKey,
        now_ms: u64,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE outbox SET next_attempt_at_ms = ?1 WHERE recipient_device_id = ?2 AND status != ?3 AND status != ?4",
            params![now_ms as i64, recipient_device_id.as_bytes(), DeliveryStatus::Acked as u8, DeliveryStatus::Expired as u8],
        ).std_context("wake outbox peer")?;
        Ok(changed)
    }
    /// Enqueue a message for delivery.
    pub fn enqueue_outbox(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        next_attempt_at_ms: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO outbox (
                msg_id, recipient_device_id, status, attempts, next_attempt_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(msg_id, recipient_device_id) DO NOTHING",
            params![
                msg_id.as_slice(),
                recipient_device_id.as_bytes(),
                DeliveryStatus::Pending as u8,
                0,
                next_attempt_at_ms as i64,
            ],
        )
        .std_context("insert outbox")?;
        Ok(())
    }
    /// Mark an outbox message as acked.
    pub fn mark_acked(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE outbox SET status = ?1, lease_owner = NULL, locked_until_ms = NULL
             WHERE msg_id = ?2 AND recipient_device_id = ?3",
            params![
                DeliveryStatus::Acked as u8,
                msg_id.as_slice(),
                recipient_device_id.as_bytes(),
            ],
        )
        .std_context("mark acked")?;
        Ok(())
    }
    /// Record that the transport layer successfully handed bytes to the
    /// remote peer.  Transitions `Sending` → `Sent`.  This is distinct
    /// from an end-to-end ACK (see [`mark_acked`](crate::storage::Storage::mark_acked)).
    ///
    /// `Sent` rows are still eligible for retry: they become claimable
    /// again once `next_attempt_at_ms` (the retry delay) has passed.
    /// Callers should pass `now + retry_policy.delay_ms(...)` so the row
    /// is NOT instantly re-claimable by the next `run_once` batch claim —
    /// otherwise the concurrent delivery loop re-claims the same rows
    /// forever.
    ///
    /// Idempotent: does nothing if the row is already `Acked` or
    /// `Expired` (guarded by WHERE).
    pub fn mark_sent(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        next_attempt_at_ms: u64,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE outbox SET status = ?1, next_attempt_at_ms = ?2
             WHERE msg_id = ?3 AND recipient_device_id = ?4
               AND status != ?5 AND status != ?6",
            params![
                DeliveryStatus::Sent as u8,
                next_attempt_at_ms as i64,
                msg_id.as_slice(),
                recipient_device_id.as_bytes(),
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
            ],
        )
        .std_context("mark sent")?;
        Ok(())
    }
    /// Record a delivery attempt.
    pub fn record_attempt(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        next_attempt_at_ms: u64,
        error_code: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now_ms = now_ms();
        conn.execute(
            "UPDATE outbox SET
                attempts = attempts + 1,
                next_attempt_at_ms = ?1,
                last_error_code = ?2,
                last_attempt_at_ms = ?3,
                status = ?4
             WHERE msg_id = ?5 AND recipient_device_id = ?6 AND status != ?7",
            params![
                next_attempt_at_ms as i64,
                error_code,
                now_ms as i64,
                DeliveryStatus::Sent as u8,
                msg_id.as_slice(),
                recipient_device_id.as_bytes(),
                DeliveryStatus::Acked as u8,
            ],
        )
        .std_context("record attempt")?;
        Ok(())
    }
    /// Fetch pending messages due for retry.
    ///
    /// Excludes ``Acked``, ``Expired``, and ``Sending`` rows — the latter
    /// are claimed by an in-flight delivery and will become eligible again
    /// after recovery if the worker crashes.
    pub fn fetch_due_outbox(&self, now_ms: u64) -> Result<Vec<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT msg_id, recipient_device_id, status, attempts,
                        next_attempt_at_ms, last_error_code, last_attempt_at_ms,
                        lease_owner, locked_until_ms, expires_at_ms
                 FROM outbox
                 WHERE status != ?1 AND status != ?2 AND status != ?3 AND next_attempt_at_ms <= ?4
                   AND (locked_until_ms IS NULL OR locked_until_ms <= ?4)
                 ORDER BY rowid
                 LIMIT ?5",
            )
            .std_context("prepare fetch_due_outbox")?;
        let mut rows = stmt
            .query(params![
                DeliveryStatus::Acked as u8,
                DeliveryStatus::Expired as u8,
                DeliveryStatus::Sending as u8,
                now_ms as i64,
                MAX_OUTBOX_CLAIM_LIMIT as i64,
            ])
            .std_context("query due outbox")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row_to_outbox(row)?);
        }
        Ok(results)
    }
    /// Atomically claim the oldest due outbox row for a worker.
    ///
    /// The claim transaction is deliberately short: no network activity may
    /// occur while the SQLite write lock is held. Expired leases are eligible
    /// for recovery, and a bounded limit prevents an untrusted queue from
    /// producing an unbounded query.
    pub fn claim_due_outbox(
        &self,
        now_ms: u64,
        lease_owner: &str,
        lease_duration_ms: u64,
        limit: u32,
    ) -> Result<Option<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .std_context("begin outbox claim")?;
        let limit = limit.clamp(1, MAX_OUTBOX_CLAIM_LIMIT) as i64;
        let candidate: Option<(MessageId, Vec<u8>)> = tx
            .query_row(
                "SELECT msg_id, recipient_device_id FROM outbox
                 WHERE status != ?1 AND status != ?2 AND next_attempt_at_ms <= ?3
                   AND (locked_until_ms IS NULL OR locked_until_ms <= ?3)
                 ORDER BY next_attempt_at_ms, rowid LIMIT ?4",
                params![
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8,
                    now_ms as i64,
                    limit
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .std_context("select outbox claim candidate")?;
        let Some((msg_blob, recipient_blob)) = candidate else {
            tx.commit().std_context("commit empty outbox claim")?;
            return Ok(None);
        };
        let locked_until = now_ms.saturating_add(lease_duration_ms);
        let changed = tx
            .execute(
                "UPDATE outbox SET lease_owner = ?1, locked_until_ms = ?2
                 WHERE msg_id = ?3 AND recipient_device_id = ?4
                   AND (locked_until_ms IS NULL OR locked_until_ms <= ?5)",
                params![
                    lease_owner,
                    locked_until as i64,
                    &msg_blob,
                    &recipient_blob,
                    now_ms as i64
                ],
            )
            .std_context("claim outbox row")?;
        if changed != 1 {
            tx.rollback().std_context("rollback lost outbox claim")?;
            return Ok(None);
        }
        let mut stmt = tx
            .prepare(
                "SELECT msg_id, recipient_device_id, status, attempts,
                        next_attempt_at_ms, last_error_code, last_attempt_at_ms,
                        lease_owner, locked_until_ms, expires_at_ms
                 FROM outbox WHERE msg_id = ?1 AND recipient_device_id = ?2",
            )
            .std_context("prepare claimed outbox row")?;
        let mut rows = stmt
            .query(params![&msg_blob, &recipient_blob])
            .std_context("query claimed outbox row")?;
        let row_ref = rows
            .next()
            .std_context("next claimed outbox row")?
            .ok_or_else(|| anyhow!("claimed outbox row disappeared"))?;
        let row = row_to_outbox(row_ref)?;
        drop(rows);
        drop(stmt);
        tx.commit().std_context("commit outbox claim")?;
        Ok(Some(row))
    }
    /// Atomically claim up to `limit` due outbox rows in a single transaction.
    ///
    /// Each row is set with `lease_owner` and `locked_until_ms`. Rows are
    /// returned in their natural order (oldest `next_attempt_at_ms` first,
    /// then rowid).  An empty vec means no claimable rows were found.
    pub fn claim_n_due_outbox(
        &self,
        now_ms: u64,
        lease_owner: &str,
        lease_duration_ms: u64,
        limit: u32,
    ) -> Result<Vec<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .std_context("begin batch outbox claim")?;
        let limit = limit.clamp(1, MAX_OUTBOX_CLAIM_LIMIT) as i64;
        let locked_until = now_ms.saturating_add(lease_duration_ms);

        // Select candidates first.
        let mut candidates: Vec<(MessageId, Vec<u8>)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT msg_id, recipient_device_id FROM outbox
                     WHERE status != ?1 AND status != ?2 AND next_attempt_at_ms <= ?3
                       AND (locked_until_ms IS NULL OR locked_until_ms <= ?3)
                     ORDER BY next_attempt_at_ms, rowid LIMIT ?4",
                )
                .std_context("prepare batch claim select")?;
            let mut rows = stmt
                .query(params![
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8,
                    now_ms as i64,
                    limit
                ])
                .std_context("query batch claim candidates")?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().std_context("next batch claim row")? {
                let msg_id: MessageId = row.get(0).std_context("get msg_id")?;
                let recipient: Vec<u8> = row.get(1).std_context("get recipient")?;
                out.push((msg_id, recipient));
            }
            out
        };

        if candidates.is_empty() {
            tx.commit().std_context("commit empty batch outbox claim")?;
            return Ok(Vec::new());
        }

        // Lock each candidate — UPDATE returns changed=1 for each success.
        candidates.retain(|(msg_id, recipient)| {
            let changed = tx
                .execute(
                    "UPDATE outbox SET lease_owner = ?1, locked_until_ms = ?2
                     WHERE msg_id = ?3 AND recipient_device_id = ?4
                       AND (locked_until_ms IS NULL OR locked_until_ms <= ?5)",
                    params![
                        lease_owner,
                        locked_until as i64,
                        msg_id.as_slice(),
                        recipient,
                        now_ms as i64
                    ],
                )
                .unwrap_or(0);
            changed == 1
        });

        // Fetch full row data for the locked entries.
        let mut results = Vec::with_capacity(candidates.len());
        for (msg_id, recipient) in &candidates {
            let mut stmt = tx
                .prepare(
                    "SELECT msg_id, recipient_device_id, status, attempts,
                            next_attempt_at_ms, last_error_code, last_attempt_at_ms,
                            lease_owner, locked_until_ms, expires_at_ms
                     FROM outbox WHERE msg_id = ?1 AND recipient_device_id = ?2",
                )
                .std_context("prepare batch claimed row")?;
            let mut rows = stmt
                .query(params![msg_id.as_slice(), recipient])
                .std_context("query batch claimed row")?;
            if let Some(row_ref) = rows.next().std_context("next batch claimed row")? {
                if let Ok(row) = row_to_outbox(row_ref) {
                    results.push(row);
                }
            }
        }

        tx.commit().std_context("commit batch outbox claim")?;
        Ok(results)
    }
    /// Atomically claim due `Pending` or `Sent` outbox rows whose
    /// `next_attempt_at_ms` has arrived, transitioning them to `Sending`
    /// and setting `last_attempt_at_ms` to `now_ms`.
    ///
    /// Returns the claimed rows in natural order (oldest `next_attempt_at_ms`
    /// first).  An empty vec means no claimable rows were found.
    ///
    /// This is the durable claim primitive for single-owner workers.
    /// The `Sending` status acts as a per-row lock that prevents two workers
    /// from concurrently picking up the same row.  Stale `Sending` rows are
    /// recovered by [`recover_stale_sending_deliveries`](crate::storage::Storage::recover_stale_sending_deliveries).
    pub fn claim_pending_deliveries(&self, limit: u32, now_ms: u64) -> Result<Vec<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .std_context("begin claim_pending_deliveries")?;
        let limit = limit.clamp(1, MAX_OUTBOX_CLAIM_LIMIT) as i64;

        // Select candidates — due Pending or Sent rows.
        let mut candidates: Vec<(MessageId, Vec<u8>)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT msg_id, recipient_device_id FROM outbox
                     WHERE (status = ?1 OR status = ?2) AND next_attempt_at_ms <= ?3
                       AND (locked_until_ms IS NULL OR locked_until_ms <= ?3)
                     ORDER BY next_attempt_at_ms, rowid LIMIT ?4",
                )
                .std_context("prepare claim_pending_deliveries select")?;
            let mut rows = stmt
                .query(params![
                    DeliveryStatus::Pending as u8,
                    DeliveryStatus::Sent as u8,
                    now_ms as i64,
                    limit,
                ])
                .std_context("query claim_pending_deliveries candidates")?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().std_context("next claim_pending row")? {
                let msg_id: MessageId = row.get(0).std_context("get msg_id")?;
                let recipient: Vec<u8> = row.get(1).std_context("get recipient")?;
                out.push((msg_id, recipient));
            }
            out
        };

        if candidates.is_empty() {
            tx.commit()
                .std_context("commit empty claim_pending_deliveries")?;
            return Ok(Vec::new());
        }

        // Atomically transition each candidate to Sending.
        candidates.retain(|(msg_id, recipient)| {
            tx.execute(
                "UPDATE outbox SET status = ?1, last_attempt_at_ms = ?2
                 WHERE msg_id = ?3 AND recipient_device_id = ?4
                   AND (status = ?5 OR status = ?6)",
                params![
                    DeliveryStatus::Sending as u8,
                    now_ms as i64,
                    msg_id.as_slice(),
                    recipient,
                    DeliveryStatus::Pending as u8,
                    DeliveryStatus::Sent as u8,
                ],
            )
            .unwrap_or(0)
                == 1
        });

        // Fetch full row data for successfully claimed entries.
        let mut results = Vec::with_capacity(candidates.len());
        for (msg_id, recipient) in &candidates {
            let mut stmt = tx
                .prepare(
                    "SELECT msg_id, recipient_device_id, status, attempts,
                            next_attempt_at_ms, last_error_code, last_attempt_at_ms,
                            lease_owner, locked_until_ms, expires_at_ms
                     FROM outbox WHERE msg_id = ?1 AND recipient_device_id = ?2",
                )
                .std_context("prepare claim_pending row fetch")?;
            let mut rows = stmt
                .query(params![msg_id.as_slice(), recipient])
                .std_context("query claim_pending row")?;
            if let Some(row_ref) = rows.next().std_context("next claim_pending fetched row")? {
                if let Ok(row) = row_to_outbox(row_ref) {
                    results.push(row);
                }
            }
        }

        tx.commit().std_context("commit claim_pending_deliveries")?;
        Ok(results)
    }
    /// Recover rows stuck in `Sending` back to `Pending` so they become
    /// eligible for re-claiming.  A row is considered stale when its
    /// `last_attempt_at_ms` is older than `stale_age_ms` before `now_ms`.
    ///
    /// The retry count is preserved; only the status and error code are
    /// updated.  A `SendingRecovered` diagnostic event is recorded with
    /// the count of recovered rows.
    pub fn recover_stale_sending_deliveries(&self, now_ms: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE outbox SET status = ?1, last_error_code = 'sending_recovered'
                 WHERE status = ?2
                   AND last_attempt_at_ms IS NOT NULL
                   AND last_attempt_at_ms < ?3",
                params![
                    DeliveryStatus::Pending as u8,
                    DeliveryStatus::Sending as u8,
                    (now_ms as i64).saturating_sub(60_000), // default stale age: 60s
                ],
            )
            .std_context("recover stale Sending deliveries")?;
        if changed > 0 {
            crate::chat_core::DIAGNOSTICS.record(
                None,
                crate::diagnostics::DiagnosticEventKind::SendingRecovered { count: changed },
            );
        }
        Ok(changed)
    }
    /// Atomically claim the oldest due row addressed to one peer.
    pub fn claim_due_outbox_for_peer(
        &self,
        now_ms: u64,
        recipient_device_id: iroh::PublicKey,
        lease_owner: &str,
        lease_duration_ms: u64,
    ) -> Result<Option<OutboxRow>> {
        let conn = self.conn.lock().unwrap();
        let tx = conn
            .unchecked_transaction()
            .std_context("begin peer outbox claim")?;
        let recipient = recipient_device_id.as_bytes();
        let candidate: Option<MessageId> = tx
            .query_row(
                "SELECT msg_id FROM outbox
             WHERE recipient_device_id = ?1 AND status != ?2 AND status != ?3
               AND next_attempt_at_ms <= ?4
               AND (locked_until_ms IS NULL OR locked_until_ms <= ?4)
             ORDER BY rowid LIMIT 1",
                params![
                    recipient,
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8,
                    now_ms as i64
                ],
                |row| row.get(0),
            )
            .optional()
            .std_context("select peer outbox claim candidate")?;
        let Some(msg_id) = candidate else {
            tx.commit().std_context("commit empty peer outbox claim")?;
            return Ok(None);
        };
        let locked_until = now_ms.saturating_add(lease_duration_ms);
        let changed = tx
            .execute(
                "UPDATE outbox SET lease_owner = ?1, locked_until_ms = ?2
             WHERE msg_id = ?3 AND recipient_device_id = ?4
               AND (locked_until_ms IS NULL OR locked_until_ms <= ?5)",
                params![
                    lease_owner,
                    locked_until as i64,
                    msg_id.as_slice(),
                    recipient,
                    now_ms as i64
                ],
            )
            .std_context("claim peer outbox row")?;
        if changed != 1 {
            tx.rollback()
                .std_context("rollback lost peer outbox claim")?;
            return Ok(None);
        }
        let mut stmt = tx
            .prepare(
                "SELECT msg_id, recipient_device_id, status, attempts,
                    next_attempt_at_ms, last_error_code, last_attempt_at_ms,
                    lease_owner, locked_until_ms, expires_at_ms
             FROM outbox WHERE msg_id = ?1 AND recipient_device_id = ?2",
            )
            .std_context("prepare claimed peer outbox row")?;
        let mut rows = stmt
            .query(params![msg_id.as_slice(), recipient])
            .std_context("query claimed peer outbox row")?;
        let row_ref = rows
            .next()
            .std_context("next claimed peer outbox row")?
            .ok_or_else(|| anyhow!("claimed peer outbox row disappeared"))?;
        let row = row_to_outbox(row_ref)?;
        drop(rows);
        drop(stmt);
        tx.commit().std_context("commit peer outbox claim")?;
        Ok(Some(row))
    }
    /// Finish a claimed attempt and release its lease.
    pub fn finish_outbox_attempt(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        lease_owner: &str,
        success: bool,
        next_attempt_at_ms: u64,
        error_code: Option<&str>,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let status = if success {
            DeliveryStatus::Sent
        } else {
            DeliveryStatus::Pending
        };
        let changed = conn
            .execute(
                "UPDATE outbox SET attempts = attempts + 1, last_attempt_at_ms = ?1,
                        next_attempt_at_ms = ?2, last_error_code = ?3, status = ?4,
                        lease_owner = NULL, locked_until_ms = NULL
                 WHERE msg_id = ?5 AND recipient_device_id = ?6
                   AND lease_owner = ?7 AND status != ?8",
                params![
                    now_ms() as i64,
                    next_attempt_at_ms as i64,
                    error_code,
                    status as u8,
                    msg_id.as_slice(),
                    recipient_device_id.as_bytes(),
                    lease_owner,
                    DeliveryStatus::Acked as u8
                ],
            )
            .std_context("finish outbox attempt")?;
        Ok(changed == 1)
    }
    /// Extend a lease without opening a transaction during network activity.
    ///
    /// The caller supplies the new absolute deadline. Only the current owner
    /// may extend a live lease; an expired lease cannot be resurrected by its
    /// former owner and must be reclaimed first.
    pub fn extend_outbox_lease(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        lease_owner: &str,
        now_ms: u64,
        locked_until_ms: u64,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE outbox SET locked_until_ms = ?1
                 WHERE msg_id = ?2 AND recipient_device_id = ?3
                   AND lease_owner = ?4 AND locked_until_ms > ?5",
                params![
                    locked_until_ms as i64,
                    msg_id.as_slice(),
                    recipient_device_id.as_bytes(),
                    lease_owner,
                    now_ms as i64
                ],
            )
            .std_context("extend outbox lease")?;
        Ok(changed == 1)
    }
    /// Release a lease without recording an attempt (for cancellation).
    pub fn release_outbox_lease(
        &self,
        msg_id: &MessageId,
        recipient_device_id: iroh::PublicKey,
        lease_owner: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE outbox SET lease_owner = NULL, locked_until_ms = NULL
             WHERE msg_id = ?1 AND recipient_device_id = ?2 AND lease_owner = ?3",
                params![
                    msg_id.as_slice(),
                    recipient_device_id.as_bytes(),
                    lease_owner
                ],
            )
            .std_context("release outbox lease")?;
        Ok(changed == 1)
    }
    /// Expire leases whose deadlines have passed, making them immediately due.
    pub fn recover_stale_outbox_leases(&self, now_ms: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let changed = conn
            .execute(
                "UPDATE outbox SET lease_owner = NULL, locked_until_ms = NULL
             WHERE locked_until_ms IS NOT NULL AND locked_until_ms <= ?1
               AND status != ?2 AND status != ?3",
                params![
                    now_ms as i64,
                    DeliveryStatus::Acked as u8,
                    DeliveryStatus::Expired as u8
                ],
            )
            .std_context("recover stale outbox leases")?;
        Ok(changed)
    }
    /// Expire outbox messages past their message expiry.
    pub fn expire_outbox(&self, now_ms: u64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE outbox SET status = ?1
             WHERE status != ?2 AND status != ?1 AND msg_id IN (
                 SELECT msg_id FROM inbox WHERE expires_at_ms <= ?3
             )",
            params![
                DeliveryStatus::Expired as u8,
                DeliveryStatus::Acked as u8,
                now_ms as i64
            ],
        )
        .std_context("expire outbox")?;
        Ok(0) // rusqlite::Connection::execute returns changed rows on some
              // builds; we don't need the exact count here.
    }
    /// Atomically remove chat-owned records for a conversation.
    ///
    /// Attachment rows are keyed by the local chat-history event ids. File
    /// objects are retained because they may also be referenced by shared-file
    /// offers or another conversation. Repeating this operation is safe.
    pub fn delete_chat_history(
        &self,
        conversation_id: &[u8; 32],
        event_ids: &[u64],
    ) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .std_context("begin chat history deletion")?;

        let mut removed = 0usize;
        for event_id in event_ids {
            removed += tx
                .execute(
                    "DELETE FROM message_attachments WHERE event_id = ?1",
                    params![*event_id as i64],
                )
                .std_context("delete message attachment")?;
        }

        tx.execute(
            "DELETE FROM dm_acknowledgements WHERE message_id IN
             (SELECT message_id FROM dm_messages WHERE conversation_id = ?1)",
            params![conversation_id.as_slice()],
        )
        .std_context("delete dm acknowledgements")?;
        tx.execute(
            "DELETE FROM dm_outbox WHERE message_id IN
             (SELECT message_id FROM dm_messages WHERE conversation_id = ?1)",
            params![conversation_id.as_slice()],
        )
        .std_context("delete dm outbox")?;
        tx.execute(
            "DELETE FROM dm_messages WHERE conversation_id = ?1",
            params![conversation_id.as_slice()],
        )
        .std_context("delete dm messages")?;
        tx.execute(
            "DELETE FROM dm_sender_sequences WHERE conversation_id = ?1",
            params![conversation_id.as_slice()],
        )
        .std_context("delete dm sender sequences")?;
        tx.execute(
            "DELETE FROM dm_conversations WHERE conversation_id = ?1",
            params![conversation_id.as_slice()],
        )
        .std_context("delete dm conversation")?;
        tx.commit().std_context("commit chat history deletion")?;
        Ok(removed)
    }
    /// Attach a file object to a chat message.
    pub fn attach_file_to_message(
        &self,
        event_id: u64,
        content_hash: &str,
        display_filename: &str,
        position: u32,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO message_attachments
                (event_id, content_hash, display_filename, position)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                event_id as i64,
                content_hash,
                display_filename,
                position as i64
            ],
        )
        .std_context("insert message_attachment")?;
        let id = conn.last_insert_rowid();
        Ok(id)
    }
    /// List all attachments for a message.
    pub fn get_message_attachments(&self, event_id: u64) -> Result<Vec<MessageAttachment>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, event_id, content_hash, display_filename, position
                 FROM message_attachments
                 WHERE event_id = ?1
                 ORDER BY position",
            )
            .std_context("prepare get_message_attachments")?;
        let mut rows = stmt
            .query(params![event_id as i64])
            .std_context("query attachments")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(MessageAttachment {
                id: row.get(0).std_context("get id")?,
                event_id: row.get::<_, i64>(1).std_context("get event_id")? as u64,
                content_hash: row.get(2).std_context("get hash")?,
                display_filename: row.get(3).std_context("get filename")?,
                position: row.get::<_, i64>(4).std_context("get position")? as u32,
            });
        }
        Ok(results)
    }
    /// Remove an attachment by its id.
    pub fn remove_message_attachment(&self, id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn
            .execute("DELETE FROM message_attachments WHERE id = ?1", params![id])
            .std_context("remove message_attachment")?;
        Ok(n > 0)
    }
    /// Find all messages that reference a given file object.
    pub fn find_messages_for_file(&self, content_hash: &str) -> Result<Vec<u64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT event_id FROM message_attachments WHERE content_hash = ?1")
            .std_context("prepare find_messages_for_file")?;
        let mut rows = stmt
            .query(params![content_hash])
            .std_context("query find_messages")?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().std_context("next row")? {
            results.push(row.get::<_, i64>(0).std_context("get event_id")? as u64);
        }
        Ok(results)
    }
    /// Insert a new outgoing message entry (delivery_state starts as "queued").
    ///
    /// Returns an error if the event_id already exists in the table
    /// (e.g. because `ChatHistoryStore::next_event_id` fell out of sync
    /// with SQLite after a crash prevented JSON from being saved).
    pub fn insert_outgoing_message(
        &self,
        event_id: u64,
        topic: &TopicId,
        hash: &str,
        signed_bytes: &[u8],
    ) -> Result<()> {
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "INSERT INTO outgoing_messages
                (event_id, topic_blob, hash, signed_bytes, delivery_state, retry_count, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, 'queued', 0, ?5, ?5)",
                params![event_id as i64, topic.as_bytes(), hash, signed_bytes, now as i64],
            )
            .std_context("insert outgoing message")?;
        if rows == 0 {
            return Err(anyhow!(
                "insert_outgoing_message: 0 rows affected for event_id={event_id} — \
                 possible event_id collision with existing SQLite row"
            )
            .into());
        }
        Ok(())
    }
    /// Update the delivery state for an outgoing message.
    /// Returns an error if the event_id does not exist.
    pub fn update_outgoing_delivery_state(&self, event_id: u64, state: &str) -> Result<()> {
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "UPDATE outgoing_messages SET delivery_state = ?1, updated_at_ms = ?2 WHERE event_id = ?3",
                params![state, now as i64, event_id as i64],
            )
            .std_context("update outgoing delivery state")?;
        if rows == 0 {
            return Err(anyhow!("no outgoing message with event_id {event_id}").into());
        }
        Ok(())
    }
    /// Increment the retry count for an outgoing message.
    pub fn increment_outgoing_retry(&self, event_id: u64) -> Result<()> {
        let now = now_ms();
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "UPDATE outgoing_messages SET retry_count = retry_count + 1, updated_at_ms = ?1 WHERE event_id = ?2",
                params![now as i64, event_id as i64],
            )
            .std_context("increment outgoing retry")?;
        if rows == 0 {
            return Err(anyhow!("no outgoing message with event_id {event_id}").into());
        }
        Ok(())
    }
    /// Return all outgoing messages whose delivery_state is "queued" (ready for retry).
    pub fn list_pending_outgoing(&self) -> Result<Vec<OutgoingMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_id, topic_blob, hash, signed_bytes, delivery_state, retry_count, created_at_ms, updated_at_ms
                 FROM outgoing_messages
                 WHERE delivery_state IN ('queued')
                 ORDER BY created_at_ms ASC",
            )
            .std_context("prepare list_pending_outgoing")?;
        let rows = stmt
            .query_map([], Self::row_to_outgoing)
            .std_context("query list_pending_outgoing")?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.std_context("read pending outgoing row")?);
        }
        Ok(results)
    }
    /// Return the most recently updated outgoing messages across ALL topics
    /// (any delivery state), newest activity first.  Used by the MCP
    /// `boru_get_outbox_status` diagnostic tool.  Limit is clamped to
    /// `[1, 500]` rows.
    pub fn list_recent_outgoing(&self, limit: usize) -> Result<Vec<OutgoingMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 500) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT event_id, topic_blob, hash, signed_bytes, delivery_state, retry_count, created_at_ms, updated_at_ms
                 FROM outgoing_messages
                 ORDER BY updated_at_ms DESC
                 LIMIT ?1",
            )
            .std_context("prepare list_recent_outgoing")?;
        let rows = stmt
            .query_map([limit], Self::row_to_outgoing)
            .std_context("query list_recent_outgoing")?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.std_context("read recent outgoing row")?);
        }
        Ok(results)
    }
    /// Return outgoing messages for a specific topic whose delivery_state is "queued".
    pub fn list_pending_outgoing_for_topic(
        &self,
        topic: &TopicId,
    ) -> Result<Vec<OutgoingMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_id, topic_blob, hash, signed_bytes, delivery_state, retry_count, created_at_ms, updated_at_ms
                 FROM outgoing_messages
                 WHERE topic_blob = ?1 AND delivery_state IN ('queued')
                 ORDER BY created_at_ms ASC",
            )
            .std_context("prepare list_pending_outgoing_for_topic")?;
        let rows = stmt
            .query_map(params![topic.as_bytes()], Self::row_to_outgoing)
            .std_context("query list_pending_outgoing_for_topic")?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.std_context("read pending outgoing row")?);
        }
        Ok(results)
    }
    /// Return ALL outgoing messages for a topic (any delivery state).
    /// Used by the GUI to reconstruct delivery state from SQLite on restart.
    pub fn list_outgoing_for_topic(&self, topic: &TopicId) -> Result<Vec<OutgoingMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT event_id, topic_blob, hash, signed_bytes, delivery_state, retry_count, created_at_ms, updated_at_ms
                 FROM outgoing_messages
                 WHERE topic_blob = ?1
                 ORDER BY created_at_ms ASC",
            )
            .std_context("prepare list_outgoing_for_topic")?;
        let rows = stmt
            .query_map(params![topic.as_bytes()], Self::row_to_outgoing)
            .std_context("query list_outgoing_for_topic")?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row.std_context("read outgoing row")?);
        }
        Ok(results)
    }
    /// Get a single outgoing message by event_id.
    pub fn get_outgoing_message(&self, event_id: u64) -> Result<Option<OutgoingMessageRow>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT event_id, topic_blob, hash, signed_bytes, delivery_state, retry_count, created_at_ms, updated_at_ms
             FROM outgoing_messages WHERE event_id = ?1",
            params![event_id as i64],
            Self::row_to_outgoing,
        )
        .optional()
        .std_context("get outgoing message")
    }
    /// Delete all outgoing messages for a given topic (used during room cleanup).
    pub fn delete_outgoing_for_topic(&self, topic: &TopicId) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = conn
            .execute(
                "DELETE FROM outgoing_messages WHERE topic_blob = ?1",
                params![topic.as_bytes()],
            )
            .std_context("delete outgoing for topic")?;
        Ok(count)
    }
    /// Return the maximum event_id in the outgoing_messages table, or 0 if empty.
    /// Used to seed ChatHistoryStore::next_event_id so it never reuses an id
    /// already in SQLite (prevents INSERT collision after JSON/save desync).
    pub fn max_outgoing_event_id(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let max_id: Option<i64> = conn
            .query_row(
                "SELECT COALESCE(MAX(event_id), 0) FROM outgoing_messages",
                [],
                |row| row.get(0),
            )
            .std_context("max outgoing event_id")?;
        Ok(max_id.unwrap_or(0) as u64)
    }
    /// Helper to parse an outgoing_messages row from a rusqlite statement.
    fn row_to_outgoing(row: &rusqlite::Row) -> rusqlite::Result<OutgoingMessageRow> {
        let topic_bytes: Vec<u8> = row.get(1)?;
        let topic_arr: [u8; 32] = topic_bytes
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        Ok(OutgoingMessageRow {
            event_id: row.get::<_, i64>(0)? as u64,
            topic: TopicId::from_bytes(topic_arr),
            hash: row.get(2)?,
            signed_bytes: row.get(3)?,
            delivery_state: row.get(4)?,
            retry_count: row.get::<_, i64>(5)? as u32,
            created_at_ms: row.get::<_, i64>(6)? as u64,
            updated_at_ms: row.get::<_, i64>(7)? as u64,
        })
    }
    /// Insert a chat message into the history table for backfill.
    ///
    /// Returns `true` if a new row was inserted, `false` if a duplicate
    /// (same msg_hash) was silently ignored.
    pub fn insert_chat_message(
        &self,
        msg_hash: &[u8; 32],
        topic: &TopicId,
        sender: &[u8; 32],
        timestamp_ms: u64,
        signed_bytes: &[u8],
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .execute(
                "INSERT OR IGNORE INTO chat_messages
                 (msg_hash, topic, sender, timestamp_ms, signed_bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    msg_hash.as_slice(),
                    topic.as_bytes(),
                    sender,
                    timestamp_ms as i64,
                    signed_bytes,
                ],
            )
            .std_context("insert chat message")?;
        Ok(rows > 0)
    }
    /// Return up to `count` of the most recent chat messages across all topics,
    /// sorted oldest-first.  Each entry is `(timestamp_ms, signed_bytes)`.
    pub fn get_recent_chat_messages(&self, count: usize) -> Result<Vec<(u64, Vec<u8>)>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT timestamp_ms, signed_bytes FROM chat_messages
                 ORDER BY id DESC
                 LIMIT ?1",
            )
            .std_context("prepare get_recent_chat_messages")?;
        let rows = stmt
            .query_map([count as i64], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, Vec<u8>>(1)?))
            })
            .std_context("query get_recent_chat_messages")?;
        let mut results: Vec<(u64, Vec<u8>)> = Vec::new();
        for row in rows {
            results.push(row.std_context("read chat message row")?);
        }
        results.reverse(); // restore chronological order
        Ok(results)
    }
    /// Return up to `count` of the most recent chat messages for a specific
    /// topic, sorted oldest-first.  Each entry is `(timestamp_ms, signed_bytes)`.
    pub fn get_recent_chat_messages_for_topic(
        &self,
        topic: &TopicId,
        count: usize,
    ) -> Result<Vec<(u64, Vec<u8>)>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let topic_bytes = topic.as_bytes();
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT timestamp_ms, signed_bytes FROM chat_messages
                 WHERE topic = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .std_context("prepare get_recent_chat_messages_for_topic")?;
        let rows = stmt
            .query_map(params![topic_bytes, count as i64], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, Vec<u8>>(1)?))
            })
            .std_context("query get_recent_chat_messages_for_topic")?;
        let mut results: Vec<(u64, Vec<u8>)> = Vec::new();
        for row in rows {
            results.push(row.std_context("read chat message row")?);
        }
        results.reverse(); // restore chronological order
        Ok(results)
    }
    /// Count the total number of chat messages across all topics.
    pub fn total_chat_message_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages", [], |row| row.get(0))
            .std_context("count chat messages")?;
        Ok(count as usize)
    }
    /// Count the number of chat messages for a specific topic.
    pub fn count_chat_messages_for_topic(&self, topic: &TopicId) -> Result<usize> {
        let topic_bytes = topic.as_bytes();
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_messages WHERE topic = ?1",
                params![topic_bytes],
                |row| row.get(0),
            )
            .std_context("count chat messages for topic")?;
        Ok(count as usize)
    }
}
