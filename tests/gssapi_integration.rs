use egui_term::{BackendCommand, PtyEvent, TerminalBackend};
use inspirum_terminal::{Session, SshOptions, terminal::connect};
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn fixture() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("INSPIRUM_GSSAPI_FIXTURE")
            .expect("run scripts/test-gssapi-integration.sh; missing fixture is not a skip"),
    );
    assert!(path.join("config").is_file());
    path
}

fn grid(backend: &mut TerminalBackend) -> String {
    backend
        .sync()
        .grid
        .display_iter()
        .map(|cell| cell.c)
        .collect()
}

fn wait_text(backend: &mut TerminalBackend, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let text = grid(backend);
        if text.contains(needle) {
            return;
        }
        assert!(Instant::now() < deadline, "missing {needle:?}: {text}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_exit(receiver: &mpsc::Receiver<(u64, PtyEvent)>, id: u64) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Ok((actual, PtyEvent::Exit)) = receiver.recv_timeout(Duration::from_millis(100)) {
            assert_eq!(actual, id);
            return;
        }
    }
    panic!("PTY {id} never emitted Exit");
}

fn open(fixture: &Path, id: u64) -> (TerminalBackend, mpsc::Receiver<(u64, PtyEvent)>) {
    let (sender, receiver) = mpsc::channel();
    let session = Session {
        name: "Disposable GSSAPI".into(),
        host: "gssapi-fixture".into(),
        strict: true,
        ssh: SshOptions {
            public_key_auth: Some(false),
            password_auth: Some(false),
            keyboard_interactive_auth: Some(false),
            gssapi_auth: Some(true),
            gssapi_delegate_credentials: Some(false),
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let backend = connect(
        id,
        eframe::egui::Context::default(),
        sender,
        &session,
        Some(&fixture.join("config")),
    )
    .expect("real terminal::connect through GSSAPI OpenSSH path");
    (backend, receiver)
}

#[test]
#[ignore = "requires scripts/test-gssapi-integration.sh"]
fn gssapi_authenticates_with_disposable_ticket_and_no_delegation() {
    let fixture = fixture();
    let (mut backend, receiver) = open(&fixture, 821);
    wait_text(&mut backend, "FIXTURE_GSSAPI_AUTHENTICATED");
    wait_text(&mut backend, "REMOTE_GSSAPI_DELEGATED:no");
    backend.process_command(BackendCommand::Write(b"exit\n".to_vec()));
    wait_exit(&receiver, 821);
    println!("PASS GSSAPI application authentication with credential delegation disabled");
}

#[test]
#[ignore = "requires scripts/test-gssapi-integration.sh with an empty credential cache"]
fn gssapi_missing_ticket_fails_without_auth_fallback() {
    let fixture = fixture();
    let (mut backend, receiver) = open(&fixture, 822);
    wait_exit(&receiver, 822);
    let text = grid(&mut backend);
    assert!(
        !text.contains("FIXTURE_GSSAPI_AUTHENTICATED"),
        "GSSAPI connection authenticated without a Kerberos ticket"
    );
    assert!(
        text.contains("Permission denied")
            || text.contains("No Kerberos credentials")
            || text.contains("credentials cache")
            || text.contains("No credentials"),
        "missing-ticket failure was not actionable: {text}"
    );
    println!("PASS missing GSSAPI ticket failed without password/key fallback");
}
