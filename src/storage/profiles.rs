//! Persistence for privacy-safe public profiles received from gossip peers.

use iroh::PublicKey;
use n0_error::{Result, StdResultExt};
use rusqlite::params;

use crate::user_profile::PublicUserProfile;

use super::{now_ms, Storage};

/// A profile loaded from the received-peer profile table.
#[derive(Debug, Clone)]
pub struct ReceivedProfileRow {
    /// Public key authenticated by the gossip envelope.
    pub peer: PublicKey,
    /// Privacy-safe payload received from the peer.
    pub profile: PublicUserProfile,
    /// Time the payload was received, in Unix milliseconds.
    pub received_at_ms: u64,
    /// Time the row was last updated, in Unix milliseconds.
    pub updated_at_ms: u64,
}

/// Profiles are kept for one hour without a refresh, matching the GUI cache policy.
pub const RECEIVED_PROFILE_TTL_MS: u64 = 60 * 60 * 1000;

impl Storage {
    /// Insert or replace a peer's validated public profile atomically.
    pub fn upsert_received_profile(
        &self,
        peer: &PublicKey,
        profile: &PublicUserProfile,
        received_at_ms: u64,
    ) -> Result<()> {
        profile.validate()?;
        let payload = postcard::to_stdvec(profile).std_context("encode received public profile")?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO received_peer_profiles
                    (peer_public_key, payload, received_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(peer_public_key) DO UPDATE SET
                    payload=excluded.payload,
                    received_at_ms=excluded.received_at_ms,
                    updated_at_ms=excluded.updated_at_ms",
                params![
                    peer.as_bytes().as_slice(),
                    payload,
                    received_at_ms as i64,
                    now_ms() as i64
                ],
            )
            .std_context("upsert received public profile")?;
            Ok(())
        })
    }

    /// Load non-expired peer profiles. Malformed rows are skipped and removed so
    /// a corrupt payload cannot prevent the application from starting.
    pub fn load_received_profiles(&self, now_ms: u64) -> Result<Vec<ReceivedProfileRow>> {
        let cutoff = now_ms.saturating_sub(RECEIVED_PROFILE_TTL_MS) as i64;
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT peer_public_key, payload, received_at_ms, updated_at_ms
                     FROM received_peer_profiles WHERE updated_at_ms >= ?1",
                )
                .std_context("prepare received public profile load")?;
            let rows = stmt
                .query_map(params![cutoff], |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .std_context("query received public profiles")?;
            let mut result = Vec::new();
            let mut malformed = Vec::new();
            for row in rows {
                let (peer_bytes, payload, received_at_ms, updated_at_ms) =
                    row.std_context("read received public profile")?;
                let Ok(peer_bytes_array) = <[u8; 32]>::try_from(peer_bytes.as_slice()) else {
                    malformed.push(peer_bytes);
                    continue;
                };
                let Ok(peer) = PublicKey::from_bytes(&peer_bytes_array) else {
                    malformed.push(peer_bytes);
                    continue;
                };
                let Ok(profile) = postcard::from_bytes::<PublicUserProfile>(&payload) else {
                    malformed.push(peer.as_bytes().to_vec());
                    continue;
                };
                if profile.validate().is_err() {
                    malformed.push(peer.as_bytes().to_vec());
                    continue;
                }
                result.push(ReceivedProfileRow {
                    peer,
                    profile,
                    received_at_ms: received_at_ms.max(0) as u64,
                    updated_at_ms: updated_at_ms.max(0) as u64,
                });
            }
            for peer in malformed {
                conn.execute(
                    "DELETE FROM received_peer_profiles WHERE peer_public_key = ?1",
                    params![peer],
                )
                .std_context("remove malformed received public profile")?;
            }
            Ok(result)
        })
    }

    /// Delete profiles that have not been refreshed within the cache TTL.
    pub fn expire_received_profiles(&self, now_ms: u64) -> Result<usize> {
        let cutoff = now_ms.saturating_sub(RECEIVED_PROFILE_TTL_MS) as i64;
        self.with_conn(|conn| {
            let count = conn
                .execute(
                    "DELETE FROM received_peer_profiles WHERE updated_at_ms < ?1",
                    params![cutoff],
                )
                .std_context("expire received public profiles")?;
            Ok(count)
        })
    }
}
