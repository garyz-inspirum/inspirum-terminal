use inspirum_terminal::{
    Session, SessionImportMode, SshOptions, delete_session, duplicate_session_draft,
    export_sessions, import_sessions, load_sessions, save_session_edit, save_sessions,
    session_matches_query,
};

fn session() -> Session {
    Session {
        name: "Work laptop".into(),
        host: "work-alias".into(),
        user: String::new(),
        port: None,
        strict: false,
        ssh: SshOptions::default(),
    }
}

#[test]
fn defaults_preserve_config_and_ask_for_host_key() {
    assert_eq!(
        session().ssh_args().unwrap(),
        ["-tt", "-o", "StrictHostKeyChecking=ask", "--", "work-alias"]
    );
}

#[test]
fn explicit_fields_are_separate_arguments() {
    let mut s = session();
    s.user = "alice".into();
    s.port = Some(2222);
    s.strict = true;
    s.host = "2001:db8::1".into();
    assert_eq!(
        s.ssh_args().unwrap(),
        [
            "-tt",
            "-o",
            "StrictHostKeyChecking=yes",
            "-l",
            "alice",
            "-p",
            "2222",
            "--",
            "2001:db8::1"
        ]
    );
}

#[test]
fn advanced_ssh_fields_map_to_discrete_openssh_arguments() {
    let mut s = session();
    s.user = "alice".into();
    s.port = Some(2222);
    s.ssh = SshOptions {
        identity_file: "/keys/work key".into(),
        proxy_jump: "jump-user@bastion:2200,second-hop".into(),
        agent_forwarding: Some(true),
        x11_forwarding: Some(true),
        compression: Some(true),
        connect_timeout_seconds: Some(12),
        server_alive_interval_seconds: Some(30),
        remote_command: "tmux attach || tmux new".into(),
        local_forwards: vec![
            "127.0.0.1:8080:internal.example:80".into(),
            "[::1]:8443:internal.example:443".into(),
        ],
        remote_forwards: vec!["127.0.0.1:9000:127.0.0.1:3000".into()],
        dynamic_forwards: vec!["127.0.0.1:1080".into()],
        ..SshOptions::default()
    };
    assert_eq!(
        s.ssh_args().unwrap(),
        [
            "-tt",
            "-o",
            "StrictHostKeyChecking=ask",
            "-i",
            "/keys/work key",
            "-J",
            "jump-user@bastion:2200,second-hop",
            "-A",
            "-X",
            "-C",
            "-o",
            "ConnectTimeout=12",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ExitOnForwardFailure=yes",
            "-L",
            "127.0.0.1:8080:internal.example:80",
            "-L",
            "[::1]:8443:internal.example:443",
            "-R",
            "127.0.0.1:9000:127.0.0.1:3000",
            "-D",
            "127.0.0.1:1080",
            "-l",
            "alice",
            "-p",
            "2222",
            "--",
            "work-alias",
            "tmux attach || tmux new"
        ]
    );
}

#[test]
fn authentication_policies_map_to_explicit_openssh_options() {
    let mut s = session();
    s.ssh.public_key_auth = Some(true);
    s.ssh.password_auth = Some(false);
    s.ssh.keyboard_interactive_auth = Some(true);
    s.ssh.gssapi_auth = Some(false);
    s.ssh.gssapi_delegate_credentials = Some(false);
    s.ssh.identities_only = Some(true);
    assert_eq!(
        s.ssh_args().unwrap(),
        [
            "-tt",
            "-o",
            "StrictHostKeyChecking=ask",
            "-o",
            "PubkeyAuthentication=yes",
            "-o",
            "PasswordAuthentication=no",
            "-o",
            "KbdInteractiveAuthentication=yes",
            "-o",
            "GSSAPIAuthentication=no",
            "-o",
            "GSSAPIDelegateCredentials=no",
            "-o",
            "IdentitiesOnly=yes",
            "--",
            "work-alias"
        ]
    );
}

#[test]
fn advanced_ssh_policies_can_explicitly_disable_configured_features() {
    let mut s = session();
    s.ssh.agent_forwarding = Some(false);
    s.ssh.x11_forwarding = Some(false);
    s.ssh.compression = Some(false);
    assert_eq!(
        s.ssh_args().unwrap(),
        [
            "-tt",
            "-o",
            "StrictHostKeyChecking=ask",
            "-a",
            "-x",
            "-o",
            "Compression=no",
            "--",
            "work-alias"
        ]
    );
}

#[test]
fn rejects_option_shell_and_control_injection() {
    for bad in [
        "",
        "-oProxyCommand=touch /tmp/pwn",
        "user@host",
        "ssh://host",
        "a b",
        "a\nb",
        "$(id)",
        "a;id",
        "a`id`",
        "a\0b",
        "host/command",
    ] {
        let mut s = session();
        s.host = bad.into();
        assert!(s.ssh_args().is_err(), "accepted {bad:?}");
    }
    for bad in ["-root", "a b", "a\nb", "a;id", "x@y"] {
        let mut s = session();
        s.user = bad.into();
        assert!(s.ssh_args().is_err());
    }
    let mut s = session();
    s.port = Some(0);
    assert!(s.ssh_args().is_err());
}

#[test]
fn rejects_invalid_advanced_ssh_arguments() {
    let mut s = session();
    s.ssh.identity_file = "bad\npath".into();
    assert!(s.ssh_args().is_err());

    let mut s = session();
    s.ssh.proxy_jump = "-oProxyCommand=bad".into();
    assert!(s.ssh_args().is_err());

    let mut s = session();
    s.ssh.proxy_jump = "jump host".into();
    assert!(s.ssh_args().is_err());

    let mut s = session();
    s.ssh.connect_timeout_seconds = Some(0);
    assert!(s.ssh_args().is_err());

    let mut s = session();
    s.ssh.server_alive_interval_seconds = Some(0);
    assert!(s.ssh_args().is_err());

    let mut s = session();
    s.ssh.local_forwards = vec!["127.0.0.1:8080:host:80\n-R9000:host:90".into()];
    assert!(s.ssh_args().is_err());

    let mut s = session();
    s.ssh.dynamic_forwards = vec![String::new()];
    assert!(s.ssh_args().is_err());

    let mut s = session();
    s.ssh.remote_command = "echo ok\nwhoami".into();
    assert!(s.ssh_args().is_err());
}

#[test]
fn saves_nonsecret_sessions_atomically_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.json");
    let first = session();
    save_sessions(&path, std::slice::from_ref(&first)).unwrap();
    assert_eq!(load_sessions(&path).unwrap(), vec![first]);

    let mut next = session();
    next.name = "Unicode: 東京".into();
    next.strict = true;
    next.ssh.identity_file = "/keys/東京".into();
    next.ssh.proxy_jump = "bastion".into();
    next.ssh.local_forwards = vec!["127.0.0.1:8080:internal:80".into()];
    save_sessions(&path, &[next.clone()]).unwrap();
    assert_eq!(load_sessions(&path).unwrap(), vec![next]);

    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value[0].as_object().unwrap().len(), 6);
}

#[test]
fn loads_legacy_profile_without_ssh_options() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.json");
    std::fs::write(
        &path,
        r#"[{"name":"legacy","host":"legacy-host","user":"","port":null,"strict":false}]"#,
    )
    .unwrap();
    let loaded = load_sessions(&path).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].name, "legacy");
    assert_eq!(loaded[0].ssh, SshOptions::default());
}

#[test]
fn rejects_corrupt_unknown_and_invalid_stored_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.json");
    assert_eq!(load_sessions(&path).unwrap(), Vec::new());
    for data in [
        "{bad",
        r#"[{"name":"a","host":"-bad","user":"","port":null,"strict":false}]"#,
        r#"[{"name":"a","host":"good","user":"","port":null,"strict":false,"password":"secret"}]"#,
        r#"[{"name":"a","host":"good","user":"","port":null,"strict":false,"ssh":{"unknown":true}}]"#,
    ] {
        std::fs::write(&path, data).unwrap();
        assert!(load_sessions(&path).is_err());
    }
}

#[test]
fn invalid_save_does_not_damage_previous_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.json");
    save_sessions(&path, &[session()]).unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut bad = session();
    bad.host = "-bad".into();
    assert!(save_sessions(&path, &[bad]).is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn oversized_save_does_not_replace_previous_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.json");
    save_sessions(&path, &[session()]).unwrap();
    let before = std::fs::read(&path).unwrap();
    let large = Session {
        name: "\"".repeat(256),
        host: "h".repeat(253),
        user: "u".repeat(253),
        port: Some(65535),
        strict: true,
        ssh: SshOptions {
            identity_file: "i".repeat(4096),
            ..SshOptions::default()
        },
    };
    let oversized = vec![large; 1000];
    let serialized = serde_json::to_vec_pretty(&oversized).unwrap();
    assert!(serialized.len() + 1 > 1_048_576);
    let error = save_sessions(&path, &oversized).unwrap_err().to_string();
    assert!(error.contains("1 MiB"), "{error}");
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn saved_profile_search_matches_name_host_and_user_case_insensitively() {
    let mut s = session();
    s.user = "Alice".into();
    assert!(session_matches_query(&s, ""));
    assert!(session_matches_query(&s, "WORK LAP"));
    assert!(session_matches_query(&s, "work-ALIAS"));
    assert!(session_matches_query(&s, "alice"));
    assert!(!session_matches_query(&s, "bastion"));
}

#[test]
fn selected_profile_can_be_renamed_without_leaving_the_old_entry() {
    let first = session();
    let mut second = session();
    second.name = "Backup".into();
    second.host = "backup-host".into();

    let mut edited = first.clone();
    edited.name = "Primary".into();
    edited.host = "primary-host".into();

    let next = save_session_edit(
        &[first, second.clone()],
        Some("Work laptop"),
        edited.clone(),
    )
    .unwrap();
    assert_eq!(next, vec![edited, second]);
    assert!(!next.iter().any(|profile| profile.name == "Work laptop"));
}

#[test]
fn profile_rename_rejects_collision_without_mutating_the_source_list() {
    let first = session();
    let mut second = session();
    second.name = "Backup".into();
    second.host = "backup-host".into();
    let original = vec![first.clone(), second.clone()];

    let mut edited = first;
    edited.name = "Backup".into();
    assert!(save_session_edit(&original, Some("Work laptop"), edited).is_err());
    assert_eq!(original, vec![session(), second]);
}

#[test]
fn new_profile_save_rejects_existing_name() {
    let existing = session();
    let mut duplicate_name = existing.clone();
    duplicate_name.host = "different-host".into();
    assert!(save_session_edit(&[existing], None, duplicate_name).is_err());
}

#[test]
fn duplicate_draft_uses_first_available_copy_name_without_persisting() {
    let first = session();
    let mut copy = first.clone();
    copy.name = "Work laptop copy".into();
    let mut copy2 = first.clone();
    copy2.name = "Work laptop copy 2".into();

    let draft = duplicate_session_draft(&[first.clone(), copy, copy2], &first);
    assert_eq!(draft.name, "Work laptop copy 3");
    assert_eq!(draft.host, first.host);
}

#[test]
fn duplicate_draft_keeps_generated_name_within_profile_limit() {
    let mut source = session();
    source.name = "x".repeat(256);
    let draft = duplicate_session_draft(std::slice::from_ref(&source), &source);
    assert!(draft.name.len() <= 256);
    assert!(draft.name.ends_with(" copy"));
    draft.ssh_args().unwrap();
}

#[test]
fn delete_session_removes_only_the_selected_profile() {
    let first = session();
    let mut second = session();
    second.name = "Backup".into();
    second.host = "backup-host".into();

    let next = delete_session(&[first, second.clone()], "Work laptop").unwrap();
    assert_eq!(next, vec![second]);
    assert!(delete_session(&next, "missing").is_err());
}

#[test]
fn profile_export_round_trips_the_validated_nonsecret_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.json");
    let mut exported = session();
    exported.ssh.identity_file = "/keys/machine-specific".into();
    exported.ssh.proxy_jump = "bastion".into();

    export_sessions(&path, std::slice::from_ref(&exported)).unwrap();
    assert_eq!(load_sessions(&path).unwrap(), vec![exported]);

    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(value[0].get("password").is_none());
    assert!(value[0].get("passphrase").is_none());
}

#[test]
fn profile_export_refuses_to_overwrite_existing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("export.json");
    std::fs::write(&path, b"keep-me").unwrap();

    let error = export_sessions(&path, &[session()]).unwrap_err().to_string();
    assert!(error.contains("already exists"), "{error}");
    assert_eq!(std::fs::read(path).unwrap(), b"keep-me");
}

#[test]
fn profile_import_merge_appends_only_nonconflicting_valid_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let import_path = dir.path().join("import.json");
    let existing = session();
    let mut incoming = session();
    incoming.name = "Imported".into();
    incoming.host = "imported-host".into();
    export_sessions(&import_path, std::slice::from_ref(&incoming)).unwrap();

    let merged =
        import_sessions(&import_path, std::slice::from_ref(&existing), SessionImportMode::Merge)
            .unwrap();
    assert_eq!(merged, vec![existing, incoming]);
}

#[test]
fn profile_import_merge_rejects_name_collision_without_touching_active_store() {
    let dir = tempfile::tempdir().unwrap();
    let active_path = dir.path().join("active.json");
    let import_path = dir.path().join("import.json");
    let existing = session();
    save_sessions(&active_path, std::slice::from_ref(&existing)).unwrap();
    let before = std::fs::read(&active_path).unwrap();

    let mut conflicting = existing.clone();
    conflicting.host = "different-host".into();
    export_sessions(&import_path, &[conflicting]).unwrap();

    assert!(
        import_sessions(
            &import_path,
            std::slice::from_ref(&existing),
            SessionImportMode::Merge
        )
        .is_err()
    );
    assert_eq!(std::fs::read(active_path).unwrap(), before);
}

#[test]
fn profile_import_rejects_malformed_or_duplicate_names_before_state_change() {
    let dir = tempfile::tempdir().unwrap();
    let malformed = dir.path().join("malformed.json");
    std::fs::write(&malformed, "{bad").unwrap();
    assert!(
        import_sessions(&malformed, &[], SessionImportMode::Replace).is_err()
    );

    let duplicate = dir.path().join("duplicate.json");
    let one = session();
    let mut two = one.clone();
    two.host = "other-host".into();
    save_sessions(&duplicate, &[one, two]).unwrap();
    let error = import_sessions(&duplicate, &[], SessionImportMode::Replace)
        .unwrap_err()
        .to_string();
    assert!(error.contains("duplicate profile name"), "{error}");
}

#[test]
fn replace_import_returns_only_validated_import_without_mutating_source_file() {
    let dir = tempfile::tempdir().unwrap();
    let import_path = dir.path().join("import.json");
    let existing = session();
    let mut replacement = session();
    replacement.name = "Replacement".into();
    replacement.host = "replacement-host".into();
    export_sessions(&import_path, std::slice::from_ref(&replacement)).unwrap();
    let import_before = std::fs::read(&import_path).unwrap();

    let next = import_sessions(
        &import_path,
        std::slice::from_ref(&existing),
        SessionImportMode::Replace,
    )
    .unwrap();
    assert_eq!(next, vec![replacement]);
    assert_eq!(std::fs::read(import_path).unwrap(), import_before);
}
