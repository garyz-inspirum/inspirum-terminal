//! Real GSSAPI acceptance with an isolated MIT Kerberos realm and sshd.
//! Run only under scripts/test-native-gssapi.py; no ambient credentials.
use inspirum_terminal::{Session, SshOptions, terminal::connect};
use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use terminal_core::{PtyEvent, TerminalBackend};

fn content(backend: &mut TerminalBackend) -> String {
    backend
        .sync()
        .grid
        .display_iter()
        .map(|cell| cell.c)
        .collect()
}

fn wait_for(backend: &mut TerminalBackend, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let current = content(backend);
        if current.contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "GSSAPI expected {expected:?}; got {current:?}"
        );
        thread::sleep(Duration::from_millis(35));
    }
}

#[test]
#[ignore = "requires disposable MIT KDC + sshd: scripts/test-native-gssapi.py"]
fn native_gssapi_only_authentication_and_delegation() {
    let fixture = PathBuf::from(
        std::env::var_os("INSPIRUM_GSSAPI_FIXTURE")
            .expect("GSSAPI fixture path is required; never use ambient realm"),
    );
    let mode = std::env::var("INSPIRUM_GSSAPI_MODE").expect("GSSAPI fixture mode missing");
    assert!(
        [
            "no-delegation",
            "delegation",
            "no-ticket",
            "expired-ticket",
        ]
        .contains(&mode.as_str())
    );

    let remote_command = match mode.as_str() {
        "no-delegation" => {
            "if klist -s; then echo GSSAPI_UNEXPECTED_DELEGATION; else echo GSSAPI_NO_DELEGATION; fi"
        }
        "delegation" => {
            "if klist -s; then echo GSSAPI_DELEGATED; else echo GSSAPI_MISSING_DELEGATION; fi"
        }
        _ => "echo GSSAPI_UNEXPECTED_ACCESS",
    };
    let session = Session {
        name: "Isolated GSSAPI".into(),
        host: "krb-probe".into(),
        strict: true,
        ssh: SshOptions {
            gssapi_auth: Some(true),
            gssapi_delegate_credentials: Some(mode == "delegation"),
            public_key_auth: Some(false),
            password_auth: Some(false),
            keyboard_interactive_auth: Some(false),
            remote_command: remote_command.into(),
            connect_timeout_seconds: Some(5),
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let (sender, receiver) = mpsc::channel();
    let mut terminal = connect(9950, sender, &session, Some(&fixture.join("ssh_config")))
        .expect("the GSSAPI-only process must start through Inspirum");
    if matches!(mode.as_str(), "no-ticket" | "expired-ticket") {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok((_, PtyEvent::Exit)) = receiver.recv_timeout(Duration::from_millis(50)) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "missing-ticket GSSAPI SSH did not terminate"
            );
        }
        assert!(
            !content(&mut terminal).contains("GSSAPI_UNEXPECTED_ACCESS"),
            "missing ticket silently authenticated through another method"
        );
    } else if mode == "delegation" {
        wait_for(&mut terminal, "GSSAPI_DELEGATED");
        assert!(!content(&mut terminal).contains("GSSAPI_MISSING_DELEGATION"));
    } else {
        wait_for(&mut terminal, "GSSAPI_NO_DELEGATION");
        assert!(!content(&mut terminal).contains("GSSAPI_UNEXPECTED_DELEGATION"));
    }
    println!("PASS isolated GSSAPI fixture mode {mode}");
}
