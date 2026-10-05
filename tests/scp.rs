use inspirum_terminal::{
    ControlMasterMode, ProxyKind, Session, SshOptions,
    scp::{download_args_with_proxy_helper, upload_args_with_proxy_helper},
};
use std::path::Path;

fn session() -> Session {
    Session {
        name: "SCP policy".into(),
        host: "example.internal".into(),
        user: "alice".into(),
        port: Some(2222),
        strict: true,
        ssh: SshOptions {
            identity_file: "/keys/id test".into(),
            proxy_jump: "bastion".into(),
            control_master: ControlMasterMode::Auto,
            control_path: "/tmp/inspirum-scp-%C".into(),
            control_persist_seconds: Some(20),
            public_key_auth: Some(true),
            password_auth: Some(false),
            keyboard_interactive_auth: Some(false),
            identities_only: Some(true),
            compression: Some(true),
            connect_timeout_seconds: Some(7),
            server_alive_interval_seconds: Some(15),
            ..SshOptions::default()
        },
    }
}

#[test]
fn scp_upload_is_discrete_argv_and_reuses_profile_policy() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("local file;literal.bin");
    std::fs::write(&local, b"binary").unwrap();
    let args = upload_args_with_proxy_helper(
        &session(),
        Some(Path::new("/tmp/ssh config")),
        Path::new("/unused"),
        &local,
        "remote file;literal.bin",
    )
    .unwrap();

    assert!(args.windows(2).any(|v| v == ["-F", "/tmp/ssh config"]));
    assert!(args.windows(2).any(|v| v == ["-i", "/keys/id test"]));
    assert!(args.windows(2).any(|v| v == ["-J", "bastion"]));
    assert!(args.windows(2).any(|v| v == ["-P", "2222"]));
    assert!(args.windows(2).any(|v| v == ["-o", "User=alice"]));
    assert!(args.windows(2).any(|v| v == ["-o", "ControlMaster=auto"]));
    assert!(
        args.windows(2)
            .any(|v| v == ["-o", "PasswordAuthentication=no"])
    );
    assert!(args.iter().any(|v| v == "-C"));
    assert_eq!(args[args.len() - 3], "--");
    assert_eq!(args[args.len() - 2], local.to_string_lossy());
    assert_eq!(
        args.last().unwrap(),
        "example.internal:remote file;literal.bin"
    );
    assert!(!args.iter().any(|arg| arg == "sh" || arg == "-c"));
}

#[test]
fn scp_download_keeps_remote_and_local_paths_as_separate_argv() {
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("download file.bin");
    let args = download_args_with_proxy_helper(
        &session(),
        None,
        Path::new("/unused"),
        "dir/remote file.bin",
        &local,
    )
    .unwrap();
    assert_eq!(args[args.len() - 3], "--");
    assert_eq!(args[args.len() - 2], "example.internal:dir/remote file.bin");
    assert_eq!(args.last().unwrap(), &local.to_string_lossy());
}

#[test]
fn structured_proxy_is_rejected_when_combined_with_proxy_jump() {
    let mut session = session();
    session.ssh.proxy_kind = ProxyKind::HttpConnect;
    session.ssh.proxy_host = "proxy.example".into();
    session.ssh.proxy_port = Some(8080);
    let temp = tempfile::tempdir().unwrap();
    let local = temp.path().join("source");
    std::fs::write(&local, b"x").unwrap();
    assert!(
        upload_args_with_proxy_helper(&session, None, Path::new("/helper"), &local, "remote",)
            .is_err()
    );
}
