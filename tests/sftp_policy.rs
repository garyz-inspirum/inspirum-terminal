use inspirum_terminal::{Session, SshOptions, terminal::sftp_launch_args};
use std::path::Path;

const AUTH_OPTIONS: [&str; 6] = [
    "PubkeyAuthentication",
    "PasswordAuthentication",
    "KbdInteractiveAuthentication",
    "GSSAPIAuthentication",
    "GSSAPIDelegateCredentials",
    "IdentitiesOnly",
];

fn profile() -> Session {
    Session {
        host: "example.test".into(),
        ..Session::default()
    }
}

fn options(args: &[String], name: &str) -> Vec<String> {
    let prefix = format!("{name}=");
    args.windows(2)
        .filter(|pair| pair[0] == "-o" && pair[1].starts_with(&prefix))
        .map(|pair| pair[1].clone())
        .collect()
}

#[test]
fn inherited_authentication_options_are_not_overridden() {
    let args = sftp_launch_args(&profile(), None).unwrap();
    for name in AUTH_OPTIONS {
        assert!(
            options(&args, name).is_empty(),
            "unexpected {name} override"
        );
    }
}

#[test]
fn all_six_authentication_policies_match_ssh_for_enable_and_disable() {
    for enabled in [false, true] {
        let mut session = profile();
        session.ssh = SshOptions {
            public_key_auth: Some(enabled),
            password_auth: Some(enabled),
            keyboard_interactive_auth: Some(enabled),
            gssapi_auth: Some(enabled),
            gssapi_delegate_credentials: Some(enabled),
            identities_only: Some(enabled),
            ..SshOptions::default()
        };
        let ssh = session.ssh_args().unwrap();
        let sftp = sftp_launch_args(&session, None).unwrap();
        for name in AUTH_OPTIONS {
            let expected = format!("{name}={}", if enabled { "yes" } else { "no" });
            assert_eq!(options(&sftp, name), vec![expected]);
            assert_eq!(options(&sftp, name), options(&ssh, name));
        }
    }
}

#[test]
fn mixed_authentication_policy_keeps_inherited_fields_absent() {
    let mut session = profile();
    session.ssh.public_key_auth = Some(true);
    session.ssh.password_auth = Some(false);
    session.ssh.gssapi_delegate_credentials = Some(false);
    let args = sftp_launch_args(&session, None).unwrap();
    assert_eq!(
        options(&args, "PubkeyAuthentication"),
        ["PubkeyAuthentication=yes"]
    );
    assert_eq!(
        options(&args, "PasswordAuthentication"),
        ["PasswordAuthentication=no"]
    );
    assert_eq!(
        options(&args, "GSSAPIDelegateCredentials"),
        ["GSSAPIDelegateCredentials=no"]
    );
    assert!(options(&args, "KbdInteractiveAuthentication").is_empty());
    assert!(options(&args, "GSSAPIAuthentication").is_empty());
    assert!(options(&args, "IdentitiesOnly").is_empty());
}

#[test]
fn sftp_ipv6_destination_is_bracketed_without_changing_ssh() {
    for host in ["::1", "2001:db8::9", "fe80::1%eth0", "fe80::1%12"] {
        let mut session = profile();
        session.host = host.into();
        let args = sftp_launch_args(&session, None).unwrap();
        assert_eq!(args.last().unwrap(), &format!("[{host}]"));
        assert_eq!(session.ssh_args().unwrap().last().unwrap(), host);
    }
}

#[test]
fn sftp_hostname_alias_and_ipv4_are_not_rewritten() {
    for host in ["work-alias", "example.test", "127.0.0.1"] {
        let mut session = profile();
        session.host = host.into();
        assert_eq!(
            sftp_launch_args(&session, None).unwrap().last().unwrap(),
            host
        );
    }
}

#[test]
fn auth_overrides_preserve_paths_trust_and_terminal_only_exclusions() {
    let mut session = profile();
    session.strict = true;
    session.ssh.identity_file = "C:/keys/work key".into();
    session.ssh.public_key_auth = Some(true);
    session.ssh.agent_forwarding = Some(true);
    session.ssh.x11_forwarding = Some(true);
    session.ssh.remote_command = "must-not-run".into();
    session.ssh.local_forwards = vec!["127.0.0.1:8080:example.test:80".into()];
    let args = sftp_launch_args(&session, Some(Path::new("config with spaces"))).unwrap();
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-i", "C:/keys/work key"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-F", "config with spaces"])
    );
    assert_eq!(
        options(&args, "StrictHostKeyChecking"),
        ["StrictHostKeyChecking=yes"]
    );
    for unwanted in ["-A", "-a", "-X", "-L", "-R", "-D", "must-not-run"] {
        assert!(
            !args.iter().any(|arg| arg == unwanted),
            "unexpected {unwanted}"
        );
    }
}

#[cfg(target_os = "linux")]
mod fixture {
    use super::*;
    use inspirum_terminal::terminal::connect_sftp;
    use std::{
        fs,
        path::PathBuf,
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    use terminal_core::{BackendCommand, PtyEvent, TerminalBackend};

    fn fixture_dir() -> PathBuf {
        let path = PathBuf::from(
            std::env::var_os("INSPIRUM_SSH_FIXTURE")
                .expect("run scripts/test-ssh-integration.sh; missing fixture is not a skip"),
        );
        assert!(path.join("config").is_file());
        path
    }

    fn open(config: &Path, ssh: SshOptions) -> (TerminalBackend, mpsc::Receiver<(u64, PtyEvent)>) {
        let (tx, rx) = mpsc::channel();
        let session = Session {
            host: "fixture-sftp".into(),
            strict: true,
            ssh,
            ..Session::default()
        };
        let backend = connect_sftp(931, tx, &session, Some(config)).unwrap();
        (backend, rx)
    }

    fn text(backend: &mut TerminalBackend) -> String {
        backend
            .sync()
            .grid
            .display_iter()
            .map(|cell| cell.c)
            .collect()
    }

    fn wait_exit(rx: &mpsc::Receiver<(u64, PtyEvent)>) {
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            if let Ok((931, PtyEvent::Exit)) = rx.recv_timeout(Duration::from_millis(50)) {
                return;
            }
        }
        panic!("SFTP policy fixture did not exit within its deadline");
    }

    #[test]
    #[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
    fn disabled_public_key_policy_blocks_sftp_even_with_valid_config_identity() {
        let dir = fixture_dir();
        let ssh = SshOptions {
            public_key_auth: Some(false),
            password_auth: Some(false),
            keyboard_interactive_auth: Some(false),
            gssapi_auth: Some(false),
            ..SshOptions::default()
        };
        let (mut backend, rx) = open(&dir.join("config"), ssh);
        wait_exit(&rx);
        let output = text(&mut backend);
        assert!(output.contains("Permission denied"), "{output}");
        assert!(
            !output.contains("sftp>"),
            "SFTP authenticated despite disabled methods"
        );
        println!("PASS disabled public-key policy blocks SFTP with valid configured key");
    }

    #[test]
    #[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
    fn explicit_public_key_policy_authenticates_and_exits() {
        let dir = fixture_dir();
        let ssh = SshOptions {
            public_key_auth: Some(true),
            password_auth: Some(false),
            keyboard_interactive_auth: Some(false),
            gssapi_auth: Some(false),
            identities_only: Some(true),
            ..SshOptions::default()
        };
        let (mut backend, rx) = open(&dir.join("config"), ssh);
        let deadline = Instant::now() + Duration::from_secs(12);
        while !text(&mut backend).contains("sftp>") {
            assert!(Instant::now() < deadline, "SFTP did not authenticate");
            thread::sleep(Duration::from_millis(25));
        }
        backend.process_command(BackendCommand::Write(b"quit\n".to_vec()));
        wait_exit(&rx);
        println!("PASS explicit SFTP public-key authentication and clean exit");
    }

    #[test]
    #[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
    fn changed_host_key_blocks_sftp_with_authentication_enabled() {
        let dir = fixture_dir();
        let wrong_key = fs::read_to_string(dir.join("wrong-host.pub")).unwrap();
        let known = dir.join("sftp-policy-wrong-known-hosts");
        fs::write(&known, format!("sftp-policy-target {wrong_key}")).unwrap();
        let config = dir.join("sftp-policy-changed-config");
        // First value wins: only these trust directives override the isolated base config.
        fs::write(
            &config,
            format!(
                "Host *\n HostKeyAlias sftp-policy-target\n UserKnownHostsFile {}\n{}",
                known.display(),
                fs::read_to_string(dir.join("config")).unwrap()
            ),
        )
        .unwrap();
        let ssh = SshOptions {
            public_key_auth: Some(true),
            ..SshOptions::default()
        };
        let (mut backend, rx) = open(&config, ssh);
        wait_exit(&rx);
        let output = text(&mut backend);
        assert!(
            output.contains("REMOTE HOST IDENTIFICATION HAS CHANGED"),
            "{output}"
        );
        assert!(
            !output.contains("sftp>"),
            "SFTP authenticated despite changed host key"
        );
        println!("PASS changed host key blocks SFTP before authentication");
    }
}
