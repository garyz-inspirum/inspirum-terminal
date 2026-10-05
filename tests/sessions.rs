use inspirum_terminal::{Session, SshOptions, load_sessions, save_sessions};

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
        local_forwards: vec![
            "127.0.0.1:8080:internal.example:80".into(),
            "[::1]:8443:internal.example:443".into(),
        ],
        remote_forwards: vec!["127.0.0.1:9000:127.0.0.1:3000".into()],
        dynamic_forwards: vec!["127.0.0.1:1080".into()],
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
