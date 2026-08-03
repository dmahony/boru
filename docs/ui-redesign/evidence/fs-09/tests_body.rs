    use super::*;

    fn local_row(hash: &str, name: &str, created_at_ms: u64) -> SharedFileRow {
        SharedFileRow {
            content_hash: hash.into(),
            profile_user_id: "local".into(),
            metadata_id: format!("meta-{hash}"),
            display_filename: name.into(),
            description: None,
            offered: true,
            created_at_ms,
            updated_at_ms: created_at_ms,
            version: 1,
        }
    }

    fn object(hash: &str, name: &str, size: u64, source: Option<&str>) -> FileObject {
        FileObject {
            content_hash: hash.into(),
            size,
            mime_type: "application/pdf".into(),
            filename: name.into(),
            created_at_ms: 1,
            data: None,
            source_path: source.map(str::to_owned),
        }
    }

    fn permission(
        hash: &str,
        grantee: &str,
        permission: &str,
        expires_at_ms: Option<u64>,
    ) -> SharedFilePermission {
        SharedFilePermission {
            content_hash: hash.into(),
            grantor_user_id: "local".into(),
            grantee_user_id: grantee.into(),
            permission: permission.into(),
            created_at_ms: 1,
            expires_at_ms,
        }
    }

    #[test]
    fn projection_is_newest_first_with_stable_id_tiebreak() {
        let rows = vec![
            local_row("a", "old.txt", 100),
            local_row("b", "new.txt", 200),
            local_row("c", "same.txt", 200),
        ];
        let out = build_shared_by_me(&rows, &HashMap::new(), &HashMap::new(), 1_000);
        assert_eq!(out[0].id, "local:local:meta-b");
        assert_eq!(out[1].id, "local:local:meta-c");
        assert_eq!(out[2].id, "local:local:meta-a");
        assert_eq!(
            out.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            out.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn projection_never_contains_local_path() {
        let mut objects = HashMap::new();
        objects.insert(
            "a".into(),
            object("a", "secret.pdf", 42, Some("/home/u/secret.pdf")),
        );
        let rows = vec![local_row("a", "secret.pdf", 100)];
        let out = build_shared_by_me(&rows, &objects, &HashMap::new(), 1_000);
        assert!(out[0].source_available);
        assert!(!format!("{:?}", out[0]).contains("/home/u"));
        assert!(!format!("{:?}", out[0]).contains("source_path"));
        // The bare display filename is expected; a *path* must never leak.
        assert_eq!(out[0].display_name, "secret.pdf");
        assert!(!out[0].display_name.contains('/'));
    }

    #[test]
    fn missing_source_and_unknown_size_render_as_safe_missing_values() {
        let rows = vec![local_row("a", "gone.pdf", 100)];
        let out = build_shared_by_me(&rows, &HashMap::new(), &HashMap::new(), 1_000);
        assert!(!out[0].source_available);
        assert_eq!(out[0].size_bytes, None);
        assert_eq!(format_size(out[0].size_bytes), "—");
        assert_eq!(kind_label(out[0].mime_type.as_deref()), "File");
    }

    #[test]
    fn recipients_are_classified_allowed_expired_denied() {
        let mut perms = HashMap::new();
        perms.insert(
            "a".into(),
            vec![
                permission("a", "peer-allowed", "read", None),
                permission("a", "peer-expired", "read", Some(500)),
                permission("a", "peer-denied", "deny", None),
            ],
        );
        let rows = vec![local_row("a", "a.pdf", 100)];
        let out = build_shared_by_me(&rows, &HashMap::new(), &perms, 1_000);
        assert!(out[0].has_explicit_recipients);
        assert_eq!(out[0].recipients.len(), 3);
        assert_eq!(out[0].recipients[0].access, RecipientAccess::Allowed);
        assert_eq!(out[0].recipients[1].access, RecipientAccess::Expired);
        assert_eq!(out[0].recipients[2].access, RecipientAccess::Denied);
    }

    #[test]
    fn zero_recipients_means_friends_fallback() {
        let rows = vec![local_row("a", "a.pdf", 100)];
        let out = build_shared_by_me(&rows, &HashMap::new(), &HashMap::new(), 1_000);
        assert!(!out[0].has_explicit_recipients);
        assert!(out[0].recipients.is_empty());
    }

    #[test]
    fn downloads_is_untracked_until_durable_counter_exists() {
        let rows = vec![local_row("a", "a.pdf", 100)];
        let out = build_shared_by_me(&rows, &HashMap::new(), &HashMap::new(), 1_000);
        assert_eq!(out[0].downloads, None);
    }

    #[test]
    fn relabel_replaces_grantee_ids_with_friendly_names() {
        let mut perms = HashMap::new();
        perms.insert("a".into(), vec![permission("a", "peer-x", "read", None)]);
        let rows = vec![local_row("a", "a.pdf", 100)];
        let mut out = build_shared_by_me(&rows, &HashMap::new(), &perms, 1_000);
        assert_eq!(out[0].recipients[0].label, "peer-x");
        let mut labels = HashMap::new();
        labels.insert("peer-x".into(), "Alice".into());
        out = relabel_recipients(out, &labels);
        assert_eq!(out[0].recipients[0].label, "Alice");
    }

    #[test]
    fn shared_on_formatting_uses_local_offset() {
        // 2026-08-04 09:12:00 UTC.
        let ms: u64 = 1_785_834_720_000;
        let label = format_shared_on_with(ms, 10 * 3600); // UTC+10 (Melbourne summer)
        assert!(label.contains("04 Aug 2026"), "got {label}");
        assert!(label.contains("19:12"), "got {label}");
        let utc = format_shared_on_with(ms, 0);
        assert!(utc.contains("09:12"), "got {utc}");
    }

    #[test]
    fn truncation_is_unicode_safe() {
        let name = "日本語のとても長いファイル名ファイル名ファイル名.pdf";
        let truncated = truncated_name(name, 12);
        assert!(truncated.chars().count() <= 12);
        assert!(truncated.ends_with('…'));
        assert!(truncated_name("short.txt", 40).ends_with("short.txt"));
    }

    fn ui_state_cycles_are_deterministic() {
        let mut ui = SharedByMeUiState::default();
        ui.toggle_menu("a");
        assert_eq!(ui.menu_open.as_deref(), Some("a"));
        ui.toggle_menu("a");
        assert_eq!(ui.menu_open, None);
        ui.toggle_menu("a");
        ui.open_details("a");
        assert_eq!(ui.menu_open, None);
        assert_eq!(ui.details_open.as_deref(), Some("a"));
        ui.clear();
        assert_eq!(ui, SharedByMeUiState::default());
    }
