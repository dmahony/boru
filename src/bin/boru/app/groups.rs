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
                    self.create_group_error = Some(crate::i18n::t("groups.name_required"));
                    self.push_system(crate::i18n::t("groups.name_required"));
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
                let authorization_bootstrap = match boru_core::authorization::AuthorizationBootstrap::sign(&self.secret_key, topic) {
                    Ok(bootstrap) => bootstrap,
                    Err(error) => {
                        self.create_group_submitting = false;
                        self.create_group_error = Some(error.to_string());
                        return iced::Task::none();
                    }
                };
                // Bump the room generation so a stale group-creation
                // completion cannot navigate the UI away from a room the
                // user opened while creation was in flight.
                self.room_generation = self.room_generation.wrapping_add(1);
                let creation_generation = self.room_generation;
                let gossip = self.gossip.clone();
                let storage = self.storage.clone();
                let net_tx = self.net_tx.clone();
                let sk = self.secret_key.clone();
                let label = self.local_label.clone();
                let forward_handle_slot = self.forward_handle_slot.clone();
                let data_dir = self.data_dir.clone();
                let endpoint = self.endpoint.clone();
                let share_direct_addresses = self.settings_state.share_direct_addresses;
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

                        // Persist creator-owned root and initial owner policy
                        // before publishing the room as active.
                        let storage = storage.ok_or_else(|| "SQLite storage unavailable; cannot create managed room".to_string())?;
                        let authorization_state = boru_core::authorization::AuthorizationState::new(topic, sk.public());
                        storage.initialize_room_authorization_root(
                            &topic,
                            &authorization_bootstrap,
                            &authorization_state,
                        ).map_err(|e| format!("persist managed-room authorization root: {e}"))?;

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
                    self.push_system(crate::i18n::t("groups.select_friend_required"));
                    return iced::Task::none();
                }

                let topic = self.topic;
                let room_history = &self.room_history;
                let room_entry = room_history.find(&topic);

                let group_id_bytes = match room_entry {
                    Some(_) => {
                        // Derive group ID bytes from topic
                        let topic_str = topic.to_string();
                        let mut bytes = [0u8; 32];
                        let topic_bytes = topic_str.as_bytes();
                        let len = topic_bytes.len().min(32);
                        bytes[..len].copy_from_slice(&topic_bytes[..len]);
                        bytes
                    }
                    None => {
                        self.push_system(crate::i18n::t("groups.not_found"));
                        return iced::Task::none();
                    }
                };

                let group_name = room_entry
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| crate::i18n::t("groups.group"));
                let inviter_pk = self.secret_key.public();
                let whisper_handle = self.whisper_handle.clone();
                let storage = self.storage.clone();
                let endpoint = self.endpoint.clone();
                let share_direct_addresses = self.settings_state.share_direct_addresses;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let expire_ms = now_ms + 7 * 24 * 60 * 60 * 1000; // 7 days

                let room_ticket = Ticket {
                    topic,
                    peers: vec![invitation_endpoint_addr(endpoint.watch_addr().get(), share_direct_addresses)],
                    discovery_secret: None,
                };
                let local_key = self.secret_key.clone();
                let mut recipient_tickets = HashMap::new();
                let is_managed = match self.storage.as_ref().expect("storage checked above").room_authorization_status(&topic) {
                    Ok(status) => status.is_some(),
                    Err(error) => {
                        self.push_system(format!("Unable to verify room authorization before inviting: {error}"));
                        return iced::Task::none();
                    }
                };
                for recipient in &selected {
                    let encoded = if is_managed {
                        let st = self.storage.as_ref().expect("storage checked above");
                        let bootstrap = match st.load_room_authorization_bootstrap(&topic) {
                            Ok(Some(root)) => root,
                            Ok(None) => {
                                self.push_system("Managed room has no valid owner bootstrap; invitation was not sent");
                                return iced::Task::none();
                            }
                            Err(error) => {
                                self.push_system(format!("Managed room root could not be loaded: {error}"));
                                return iced::Task::none();
                            }
                        };
                        let (state, _) = match st.load_room_authorization(&topic) {
                            Ok(Some(loaded)) => loaded,
                            Ok(None) => {
                                self.push_system("Managed room policy is missing; invitation was not sent");
                                return iced::Task::none();
                            }
                            Err(error) => {
                                self.push_system(format!("Managed room policy could not be loaded: {error}"));
                                return iced::Task::none();
                            }
                        };
                        if state.owner() != local_key.public() || bootstrap.owner != local_key.public() {
                            self.push_system("Only the pinned managed-room owner can issue invitations");
                            return iced::Task::none();
                        }
                        let event = match boru_core::authorization::AuthorizationEvent::sign(
                            &local_key, topic, state.last_sequence().saturating_add(1), *recipient,
                            boru_core::authorization::AuthorizationAction::Admit {
                                role: boru_core::authorization::Role::Member,
                            },
                        ) {
                            Ok(event) => event,
                            Err(error) => {
                                self.push_system(format!("Unable to sign managed-room admission: {error}"));
                                return iced::Task::none();
                            }
                        };
                        let (updated, events) = match st.issue_room_invitation_admission(&topic, &state, &event) {
                            Ok(result) => result,
                            Err(error) => {
                                self.push_system(format!("Unable to persist managed-room admission: {error}"));
                                return iced::Task::none();
                            }
                        };
                        self.room_authorization.insert(topic, updated);
                        match (boru_core::chat_core::protocol::ManagedRoomInvitationV2 {
                            version: 2,
                            ticket: room_ticket.clone(),
                            bootstrap,
                            recipient: *recipient,
                            events,
                        }).encode() {
                            Ok(ticket) => ticket,
                            Err(error) => {
                                self.push_system(format!("Unable to encode managed-room invitation: {error}"));
                                return iced::Task::none();
                            }
                        }
                    } else {
                        room_ticket.to_string()
                    };
                    recipient_tickets.insert(*recipient, encoded);
                }

                iced::Task::perform(
                    async move {
                        let inviter_pk_bytes = inviter_pk.to_vec();
                        let mut sent = 0usize;
                        let mut failures = Vec::new();

                        for peer_key in &selected {
                            let Some(ticket_str) = recipient_tickets.get(peer_key) else {
                                failures.push(format!("{peer_key}: missing invitation envelope"));
                                continue;
                            };
                            let invite_id: [u8; 32] = rand::random();
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
                                if let Err(error) = st.create_group_invite(&invite_row) {
                                    failures.push(format!("{peer_key}: invitation persistence failed: {error}"));
                                    continue;
                                }
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
                                    sent += 1;
                                    tracing::info!(
                                        ?peer_key,
                                        invite_len = invite_text.len(),
                                        "whisper DM sent for group invite"
                                    );
                                }
                                Err(e) => {
                                    failures.push(format!("{peer_key}: delivery failed: {e}"));
                                    tracing::warn!(
                                        ?peer_key,
                                        error = %e,
                                        "whisper DM failed for group invite"
                                    );
                                }
                            }
                        }

                        AppMessage::SystemMsg(if failures.is_empty() {
                            crate::i18n::t_args("groups.invite_sent", &[("count", &sent.to_string()), ("group_name", &group_name)])
                        } else {
                            format!("Invited {sent} of {} selected members to {group_name}; {} failed: {}", selected.len(), failures.len(), failures.join("; "))
                        })
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
                            crate::i18n::t("groups.group")
                        } else {
                            inv.group_name.clone()
                        };
                        let managed_invitation_v2 = boru_core::chat_core::protocol::ManagedRoomInvitationV2::parse(&inv.ticket).ok();
                        let managed_invitation = boru_core::chat_core::protocol::ManagedRoomInvitation::parse(&inv.ticket).ok();
                        let invitation_topic = managed_invitation_v2.as_ref().map(|i| i.ticket.topic)
                            .or_else(|| managed_invitation.as_ref().map(|i| i.ticket.topic))
                            .or_else(|| RoomInvitation::parse(&inv.ticket).ok().map(|ticket| ticket.topic()));
                        if let Some(topic) = invitation_topic {
                            if let Some(managed) = managed_invitation_v2 {
                                let Some(st) = self.storage.as_ref() else {
                                    self.push_system("SQLite storage unavailable; cannot accept managed invitation");
                                    return iced::Task::none();
                                };
                                let recipient = self.local_public;
                                if managed.recipient != recipient {
                                    self.push_system("Recipient-bound invitation was issued to a different identity");
                                    return iced::Task::none();
                                }
                                if let Err(error) = st.accept_room_authorization_invitation_v2(&managed) {
                                    self.push_system(format!("Could not accept recipient-bound invitation: {error}"));
                                    return iced::Task::none();
                                }
                                match st.load_room_authorization(&topic) {
                                    Ok(Some((state, _))) => { self.room_authorization.insert(topic, state); }
                                    Ok(None) => {
                                        self.push_system("Managed invitation policy is missing; room remains denied");
                                        return iced::Task::none();
                                    }
                                    Err(error) => {
                                        self.push_system(format!("Managed invitation policy failed validation: {error}"));
                                        return iced::Task::none();
                                    }
                                }
                            } else if let Some(managed) = managed_invitation {
                                let inviter: [u8; 32] = match inv.inviter_public_key.as_slice().try_into() {
                                    Ok(bytes) => bytes,
                                    Err(_) => {
                                        self.push_system("Owner-bound invitation has an invalid inviter key");
                                        return iced::Task::none();
                                    }
                                };
                                let inviter = match PublicKey::from_bytes(&inviter) {
                                    Ok(key) => key,
                                    Err(_) => {
                                        self.push_system("Owner-bound invitation has an invalid inviter key");
                                        return iced::Task::none();
                                    }
                                };
                                if let Err(error) = managed.bootstrap.verify(topic, inviter) {
                                    self.push_system(format!("Rejected invalid owner-bound invitation: {error}"));
                                    return iced::Task::none();
                                }
                                let Some(st) = self.storage.as_ref() else {
                                    self.push_system("SQLite storage unavailable; cannot accept managed invitation");
                                    return iced::Task::none();
                                };
                                if let Err(error) = st.accept_room_authorization_bootstrap(&topic, &managed.bootstrap, inviter) {
                                    self.push_system(format!("Could not pin managed-room owner root: {error}"));
                                    return iced::Task::none();
                                }
                                match st.load_room_authorization(&topic) {
                                    Ok(Some((state, _))) => { self.room_authorization.insert(topic, state); }
                                    Ok(None) => {
                                        self.push_system("Managed invitation policy is missing; room remains denied");
                                        return iced::Task::none();
                                    }
                                    Err(error) => {
                                        self.push_system(format!("Managed invitation policy failed validation: {error}"));
                                        return iced::Task::none();
                                    }
                                }
                            }
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
                    self.push_system(crate::i18n::t("groups.invite_no_ticket"));
                } else {
                    self.push_system(crate::i18n::t("groups.invite_not_found"));
                }
                iced::Task::none()
            }

            AppMessage::GroupCreated {
                sender,
                topic,
                ticket: ticket_str,
                entry,
                group_id,
                name: display_name,
                description: _,
                members: friend_keys,
                generation,
            } => {
                // The async creation completed: clear the in-flight flag and
                // close the dialog so navigation to the new group proceeds.
                self.create_group_submitting = false;
                self.show_create_group_dialog = false;
                self.create_group_error = None;
                // State-safety: if the user opened another room while the
                // group-creation task was in flight, this completion is stale
                // and must not navigate the UI away from that room. Detect in
                // debug builds.
                debug_assert_eq!(
                    self.room_generation, generation,
                    "stale GroupCreated: completion generation {generation} \
                     != current room generation {}",
                    self.room_generation,
                );
                info!(
                    ?group_id,
                    name = %display_name,
                    member_count = friend_keys.len(),
                    ?topic,
                    "group created",
                );
                // Save the current room state to the conversation cache so the
                // user can switch back to it without a slow re-subscribe.
                self.save_room_to_history();
                self.leave_current_room();
                // The new group appears in the Groups screen's group list.
                self.invalidate_prewarm(&[Screen::Groups]);

                // Apply the gossip sender for the new group room
                self.sender = Some(*sender);
                self.room_loading = false;

                // Persist the conversation entry
                self.conversation_store.upsert(*entry);
                self.chats_sidebar_revision = self.chats_sidebar_revision.wrapping_add(1);

                // Set member count on room history entry (includes self + selected members)
                let total_members = friend_keys.len() as u32 + 1; // +1 for self (creator)
                self.room_history.upsert(topic, display_name.clone(), true);
                self.room_history.update_member_count(&topic, total_members);
                self.room_history_dirty = true;

                // Navigate to the group room - emulate RoomOpened
                self.topic = topic;
                self.ticket_str = ticket_str.clone();
                self.screen = Screen::Chat { topic };
                // A new active conversation is being applied — bump the
                // conversation ownership token so in-flight image downloads
                // started for the previous conversation are detected.
                self.conversation_generation = self.conversation_generation.wrapping_add(1);

                // Build task list: clipboard write + invite DMs if members selected
                let mut tasks: Vec<iced::Task<AppMessage>> =
                    vec![iced::clipboard::write(ticket_str.clone())];

                if !friend_keys.is_empty() {
                    let mut group_id_bytes = [0u8; 32];
                    let topic_str = topic.to_string();
                    let topic_bytes = topic_str.as_bytes();
                    let len = topic_bytes.len().min(32);
                    group_id_bytes[..len].copy_from_slice(&topic_bytes[..len]);

                    let whisper_handle = self.whisper_handle.clone();
                    let storage = self.storage.clone();
                    let sk = self.secret_key.clone();

                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let expire_ms = now_ms + 7 * 24 * 60 * 60 * 1000; // 7 days
                    let group_name = display_name.clone();
                    let member_count = friend_keys.len();

                    let invite_task = iced::Task::perform(
                        async move {
                            let invite_id: [u8; 32] = rand::random();
                            let inviter_pk_bytes = sk.public().to_vec();
                            use data_encoding::HEXLOWER;
                            let invite_id_hex = HEXLOWER.encode(&invite_id);
                            let group_id_hex = HEXLOWER.encode(&group_id_bytes);
                            let invite_text = format!(
                                "INVITE:{invite_id_hex}:{}:{group_id_hex}:{group_name}:{ticket_str}",
                                sk.public(),
                            );

                            for peer_key in &friend_keys {
                                // Persist GroupInviteRow
                                if let Some(ref st) = storage {
                                    let invite_row = boru_core::storage::GroupInviteRow {
                                        invite_id,
                                        group_id: group_id_bytes,
                                        inviter_public_key: inviter_pk_bytes.clone(),
                                        recipient_public_key: peer_key.to_vec(),
                                        epoch: 1,
                                        status: "Pending".into(),
                                        created_at_ms: now_ms,
                                        expires_at_ms: expire_ms,
                                        ticket: ticket_str.clone(),
                                        group_name: group_name.clone(),
                                    };
                                    let _ = st.create_group_invite(&invite_row);
                                }

                                // Send whisper DM
                                match whisper_handle.send_dm(*peer_key, invite_text.clone()).await {
                                    Ok(()) => {
                                        tracing::info!(
                                            ?peer_key,
                                            "whisper DM sent for group invite (auto on create)"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            ?peer_key,
                                            error = %e,
                                            "whisper DM failed for group invite (auto on create)"
                                        );
                                    }
                                }
                            }

                            AppMessage::SystemMsg(crate::i18n::t_args(
                                "groups.created_with_members",
                                &[
                                    ("group_name", &group_name),
                                    ("member_count", &member_count.to_string()),
                                ],
                            ))
                        },
                        |msg| msg,
                    );
                    tasks.push(invite_task);
                } else {
                    tasks.push(iced::Task::done(AppMessage::SystemMsg(
                        crate::i18n::t_args(
                            "groups.created_ticket_copied",
                            &[("display_name", &display_name)],
                        ),
                    )));
                }

                return iced::Task::batch(tasks);
            }
            AppMessage::OpenGroups => {
                if !matches!(self.screen, Screen::Groups) {
                    self.groups_return_to = Some(self.screen.clone());
                }
                self.screen = Screen::Groups;
                iced::Task::none()
            }
            AppMessage::CloseGroups => {
                self.screen = self.groups_return_to.take().unwrap_or(Screen::ChatList);
                iced::Task::none()
            }
            // update() only dispatches the groups variants here; other
            // variants can never reach this method (defensive catch-all).
            _ => iced::Task::none(),
        }
    }
}
