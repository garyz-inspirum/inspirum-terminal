use inspirum_terminal::{Session, load_sessions, save_sessions};
fn session() -> Session {
    Session {
        name: "Work laptop".into(),
        host: "work-alias".into(),
        user: String::new(),
        port: None,
        strict: false,
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
fn rejects_option_shell_and_control_injection() {
    for bad in [
        "",
        "-oProxyCommand=touch /tmp/pwn",
        "user@host",
        "ssh://host",
        "a b",
        "a
b",
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
    for bad in [
        "-root", "a b", "a
b", "a;id", "x@y",
    ] {
        let mut s = session();
        s.user = bad.into();
        assert!(s.ssh_args().is_err());
    }
    let mut s = session();
    s.port = Some(0);
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
    save_sessions(&path, &[next.clone()]).unwrap();
    assert_eq!(load_sessions(&path).unwrap(), vec![next]);
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value[0].as_object().unwrap().len(), 5);
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
    };
    let oversized = vec![large; 1000];
    let serialized = serde_json::to_vec_pretty(&oversized).unwrap();
    assert!(serialized.len() + 1 > 1_048_576);
    let error = save_sessions(&path, &oversized).unwrap_err().to_string();
    assert!(error.contains("1 MiB"), "{error}");
    assert_eq!(std::fs::read(path).unwrap(), before);
}
