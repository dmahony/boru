//! Group chat state (create group dialog).
//!
//! Extracted from app.rs (BORU-AUDIT-22). Owns the group-creation update
//! arms: show/hide dialog, name/description/member/search changes, and the
//! async confirm-create flow (subscribe, metadata + roster docs, friend
//! notifications). Reads app state via `use super::*`; app.rs re-exports
//! the pub(crate) items it still references with `use groups::*`.

use super::*;

impl IcedChat {

    /// State-layer update for group creation (BORU-AUDIT-22 spec step 5).
    ///
    /// Handles the create-group dialog (show/hide, name/description/member/
    /// search changes) and the async confirm-create flow. The root
    /// `update()` dispatches these variants here via combined match arms.
    pub(crate) fn update_groups(&mut self, message: AppMessage) -> iced::Task<AppMessage> {
        match message {
            AppMessage::ShowCreateGroupDialog => {
                self.show_create_group_dialog = true;
                self.create_group_name = String::new();
                self.create_group_description = String::new();
                self.create_group_selected_members.clear();
                self.create_group_search = String::new();
                self.create_group_submitting = false;
                self.create_group_error = None;
                // Auto-focus the group name field.
                iced::widget::operation::focus(CREATE_GROUP_NAME_INPUT)
            }
            AppMessage::HideCreateGroupDialog => {
                // Mid-submit: the dialog cannot be dismissed until the async
                // group creation completes.
                if self.create_group_submitting {
                    return iced::Task::none();
                }
                self.show_create_group_dialog = false;
                self.create_group_error = None;
                iced::Task::none()
            }
            AppMessage::CreateGroupNameChanged(name) => {
                self.create_group_name = name;
                iced::Task::none()
            }
            AppMessage::CreateGroupDescriptionChanged(desc) => {
                self.create_group_description = desc;
                iced::Task::none()
            }
            AppMessage::CreateGroupMemberToggled(peer) => {
                if self.create_group_selected_members.contains(&peer) {
                    self.create_group_selected_members.remove(&peer);
                } else {
                    self.create_group_selected_members.insert(peer);
                }
                iced::Task::none()
            }
            AppMessage::CreateGroupSearchChanged(query) => {
                self.create_group_search = query;
                iced::Task::none()
            }

            AppMessage::ConfirmCreateGroup => {
                // Guard: never re-enter while a submit is in flight.
                if self.create_group_submitting {
                    return iced::Task::none();
                }
                let group_name = std::mem::take(&mut self.create_group_name);
                let group_description = std::mem::take(&mut self.create_group_description);
                let selected_members: Vec<PublicKey> =
                    self.create_group_selected_members.drain().collect();

                if group_name.trim().is_empty() {
                    // Keep the dialog open and show the error inline under
                    // the name field instead of only logging/toasting it.
                    self.create_group_name = group_name;
                    self.create_group_error = Some("Group name is required.".to_string());
                    self.push_system("Group name is required.".to_string());
                    return iced::Task::none();
                }
                self.create_group_error = None;

                // Keep the dialog open while the async gossip subscription +
                // metadata/roster creation runs; the primary button shows a
                // loading state and Escape/backdrop/Cancel are disabled.
                self.create_group_submitting = true;

                // ── Generate group identifiers ────────────────────────
                let group_id = GroupId::generate();
                let epoch = 1u64;
                let topic = TopicId::from_bytes(rand::random());
                // Bump the room generation so a stale group-creation
                // completion cannot navigate the UI away from a room the
                // user opened while creation was in flight.
                self.room_generation = self.room_generation.wrapping_add(1);
                let creation_generation = self.room_generation;
                let gossip = self.gossip.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let data_dir = self.data_dir.clone();
                let endpoint = self.endpoint.clone();
                let share_direct_addresses = self.share_direct_addresses;
                let friend_keys: Vec<PublicKey> = selected_members;
                let display_name = group_name.trim().to_string();
                let description = group_description.trim().to_string();

                // Show a loading spinner while the gossip subscription is in flight.
                self.room_loading = true;

                iced::Task::perform(
                    async move {
                        // Step 1: Subscribe to the new group topic
                        let sub = gossip
                            .subscribe(topic, vec![])
                            .await
                            .map_err(|e| e.to_string())?;
                        let (sender, receiver) = sub.split();
                        let neighbor_ids: Vec<PublicKey> = receiver.neighbors().collect();
                        let neighbor_count = neighbor_ids.len();
                        let local_peer_addr = invitation_endpoint_addr(
                            endpoint.watch_addr().get(),
                            share_direct_addresses,
                        );

                        // Step 2: Create room metadata (name + description)
                        let meta_name = if display_name.is_empty() {
                            None
                        } else {
                            Some(display_name.clone())
                        };
                        let meta_desc = if description.is_empty() {
                            None
                        } else {
                            Some(description.clone())
                        };
                        let metadata_doc = room_docs::create_metadata_doc(
                            topic,
                            &sender,
                            RoomMetadata {
                                name: meta_name,
                                description: meta_desc,
                                rules: None,
                            },
                        )
                        .await
                        .map_err(|e| e.to_string())?;

                        // Step 3: Create roster doc with creator as member
                        let roster_doc = room_docs::create_roster_doc(
                            topic,
                            &sender,
                            sk.public().to_string(),
                            label.clone(),
                        )
                        .await
                        .map_err(|e| e.to_string())?;

                        // Step 4: Start conversation forwarder
                        let forward_handle = spawn_conversation_forwarder(
                            topic,
                            metadata_doc,
                            roster_doc,
                            receiver,
                            net_tx,
                            None,
                        );
                        *forward_handle_slot.lock().unwrap() = Some(forward_handle);

                        // Step 5: Broadcast creator presence
                        let msg = SignedMessage::sign_and_encode(
                            &sk,
                            &crate::Message::AboutMe {
                                name: label,
                                profile_image_ticket: None,
                            },
                        )
                        .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(msg).await;
                        let presence =
                            SignedMessage::sign_and_encode(&sk, &crate::Message::Presence)
                                .map_err(|e| e.to_string())?;
                        let _ = sender.broadcast(presence).await;

                        // Step 6: Persist room store
                        let mut room =
                            RoomStore::with_peers(&data_dir, topic, vec![local_peer_addr]);
                        room.discovery_secret = None;

                        // Step 7: Create conversation entry for the group
                        let entry = ConversationEntry::new_group_epoch(
                            group_id,
                            epoch,
                            topic,
                            display_name.clone(),
                        );

                        // Step 8: Build the ticket string for invitation
                        let ticket_str = Ticket {
                            topic,
                            peers: vec![invitation_endpoint_addr(
                                endpoint.watch_addr().get(),
                                share_direct_addresses,
                            )],
                            discovery_secret: None,
                        }
                        .to_string();

                        Ok::<_, String>((
                            sender,
                            topic,
                            ticket_str,
                            entry,
                            group_id,
                            display_name,
                            description,
                            friend_keys,
                        ))
                    },
                    move |result| match result {
                        Ok((
                            sender,
                            topic_id,
                            ticket_str,
                            entry,
                            group_id,
                            display_name,
                            description,
                            friend_keys,
                        )) => {
                            // These are applied in the next update cycle
                            AppMessage::GroupCreated {
                                sender: Box::new(sender),
                                topic: topic_id,
                                ticket: ticket_str,
                                entry: Box::new(entry),
                                group_id,
                                name: display_name,
                                description,
                                members: friend_keys,
                                generation: creation_generation,
                            }
                        }
                        Err(e) => AppMessage::RoomJoinFailed {
                            error: e,
                            generation: creation_generation,
                        },
                    },
                )
            }

            AppMessage::ShowInviteMemberDialog => {
                self.show_invite_member_dialog = true;
                self.invite_member_selected.clear();
                iced::Task::none()
            }
            AppMessage::HideInviteMemberDialog => {
                self.show_invite_member_dialog = false;
                self.invite_member_selected.clear();
                iced::Task::none()
            }
            AppMessage::InviteMemberToggled(peer) => {
                if self.invite_member_selected.contains(&peer) {
                    self.invite_member_selected.remove(&peer);
                } else {
                    self.invite_member_selected.insert(peer);
                }
                iced::Task::none()
            }
            AppMessage::ConfirmInviteMember => {
                let selected: Vec<PublicKey> = self.invite_member_selected.drain().collect();
                self.show_invite_member_dialog = false;

                if selected.is_empty() {
                    self.push_system("Select at least one friend to invite.".to_string());
                    return iced::Task::none();
                }

                let topic = self.topic;
                let room_history = &self.room_history;
                let room_entry = room_history.find(&topic);

                let group_id_bytes = match room_entry {
                    Some(entry) => {
                        // Derive group ID bytes from topic
                        let topic_str = topic.to_string();
                        let mut bytes = [0u8; 32];
                        let topic_bytes = topic_str.as_bytes();
                        let len = topic_bytes.len().min(32);
                        bytes[..len].copy_from_slice(&topic_bytes[..len]);
                        bytes
                    }
                    None => {
                        self.push_system("Group not found. Cannot send invite.".to_string());
                        return iced::Task::none();
                    }
                };

                let group_name = room_entry
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| "Group".to_string());
                let inviter_pk = self.secret_key.public();
                let inviter_name = self.local_label.clone();
                let whisper_handle = self.whisper_handle.clone();
                let storage = self.storage.clone();
                let data_dir = self.data_dir.clone();
                let sk = self.secret_key.clone();
                let endpoint = self.endpoint.clone();
                let share_direct_addresses = self.share_direct_addresses;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let expire_ms = now_ms + 7 * 24 * 60 * 60 * 1000; // 7 days

                // Construct a ticket so the invitee can join the room
                let ticket_str = Ticket {
                    topic,
                    peers: vec![invitation_endpoint_addr(
                        endpoint.watch_addr().get(),
                        share_direct_addresses,
                    )],
                    discovery_secret: None,
                }
                .to_string();

                iced::Task::perform(
                    async move {
                        let invite_id: [u8; 32] = rand::random();
                        let inviter_pk_bytes = inviter_pk.to_vec();

                        for peer_key in &selected {
                            // 1. Persist GroupInviteRow in storage
                            let recipient_pk_bytes = peer_key.to_vec();
                            if let Some(ref st) = storage {
                                let invite_row = boru_core::storage::GroupInviteRow {
                                    invite_id,
                                    group_id: group_id_bytes,
                                    inviter_public_key: inviter_pk_bytes.clone(),
                                    recipient_public_key: recipient_pk_bytes,
                                    epoch: 1,
                                    status: "Pending".into(),
                                    created_at_ms: now_ms,
                                    expires_at_ms: expire_ms,
                                    ticket: ticket_str.clone(),
                                    group_name: group_name.clone(),
                                };
                                let _ = st.create_group_invite(&invite_row);
                            }

                            // 2. Send invite as DM via whisper - using INVITE prefix + base64
                            // invite_id + group_id (each 32 bytes) encoded as hex via data-encoding
                            // Final field is the room ticket so the invitee can join directly.
                            use data_encoding::HEXLOWER;
                            let invite_id_hex = HEXLOWER.encode(&invite_id);
                            let group_id_hex = HEXLOWER.encode(&group_id_bytes);
                            let invite_text = format!(
                                "INVITE:{invite_id_hex}:{inviter_pk}:{group_id_hex}:{group_name}:{ticket_str}"
                            );
                            match whisper_handle.send_dm(*peer_key, invite_text.clone()).await {
                                Ok(()) => {
                                    tracing::info!(
                                        ?peer_key,
                                        invite_len = invite_text.len(),
                                        "whisper DM sent for group invite"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        ?peer_key,
                                        error = %e,
                                        "whisper DM failed for group invite"
                                    );
                                }
                            }
                        }

                        let count = selected.len();
                        AppMessage::SystemMsg(format!(
                            "Group invite sent to {count} friend(s) for \"{group_name}\"."
                        ))
                    },
                    |msg| msg,
                )
            }
            AppMessage::AcceptGroupInvite(invite_id) => {
                // Look up the invite ticket and auto-join the room
                let invite = self.storage.as_ref().and_then(|st| {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    st.get_pending_group_invites(&self.local_public.to_vec(), now_ms)
                        .ok()?
                        .into_iter()
                        .find(|inv| inv.invite_id.as_slice() == invite_id.as_slice())
                });

                if let Some(inv) = invite {
                    if !inv.ticket.is_empty() {
                        // Create the group ConversationEntry NOW so RoomOpened
                        // can find and unarchive it after JoinFromTicket succeeds.
                        // RoomOpened only unarchives — it never creates entries.
                        let group_name = if inv.group_name.is_empty() {
                            "Group".to_string()
                        } else {
                            inv.group_name.clone()
                        };
                        if let Ok(ticket) = RoomInvitation::parse(&inv.ticket) {
                            let topic = ticket.topic();
                            let group_id = GroupId::from_bytes(inv.group_id);
                            let entry = ConversationEntry::new_group_epoch(
                                group_id,
                                inv.epoch,
                                topic,
                                &group_name,
                            );
                            self.conversation_store.upsert(entry);
                            self.chats_sidebar_revision =
                                self.chats_sidebar_revision.wrapping_add(1);
                        }

                        self.join_ticket_input = inv.ticket.clone();
                        // Mark the invite as accepted
                        let mut id = [0u8; 32];
                        let len = inv.invite_id.len().min(32);
                        id[..len].copy_from_slice(&inv.invite_id[..len]);
                        if let Some(ref st) = self.storage {
                            let _ = st.update_group_invite_state(&id, "Accepted");
                        }
                        // Bump revision to remove the invite from sidebar
                        self.requests_sidebar_revision =
                            self.requests_sidebar_revision.wrapping_add(1);
                        self.refresh_sidebar_counts();
                        return iced::Task::done(AppMessage::JoinFromTicket);
                    }
                    self.push_system(
                        "Cannot accept group invite: no ticket available. Ask the sender to re-invite.".to_string(),
                    );
                } else {
                    self.push_system("Group invite not found or expired.".to_string());
                }
                iced::Task::none()
            }

            // update() only dispatches the groups variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
