//! Cross-platform native SSH acceptance using scripts/test-native-ssh-smoke.py.
use inspirum_terminal::{Session, terminal::connect};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use terminal_core::{BackendCommand, PtyEvent, TerminalBackend};

fn fixture() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("INSPIRUM_NATIVE_SSH_FIXTURE")
            .expect("run scripts/test-native-ssh-smoke.py; missing fixture is not a skip"),
    );
    assert!(path.join("config").is_file());
    assert!(path.join("changed-config").is_file());
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
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let text = grid(backend);
        if text.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "missing {needle:?} from native SSH terminal: {text}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_exit(receiver: &mpsc::Receiver<(u64, PtyEvent)>, expected_id: u64) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        if let Ok((id, PtyEvent::Exit)) = receiver.recv_timeout(Duration::from_millis(50)) {
            assert_eq!(id, expected_id);
            return;
        }
    }
    panic!("native SSH PTY {expected_id} never emitted Exit");
}

fn open(
    fixture: &Path,
    id: u64,
    config_name: &str,
) -> (TerminalBackend, mpsc::Receiver<(u64, PtyEvent)>) {
    let (sender, receiver) = mpsc::channel();
    let session = Session {
        name: "Native SSH acceptance".into(),
        host: "native-smoke".into(),
        strict: true,
        ..Session::default()
    };
    let backend = connect(id, sender, &session, Some(&fixture.join(config_name)))
        .expect("launch real system OpenSSH through Inspirum");
    (backend, receiver)
}

fn write(backend: &mut TerminalBackend, value: &str) {
    backend.process_command(BackendCommand::Write(value.as_bytes().to_vec()));
}

fn event_count(path: &Path, prefix: &str) -> usize {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with(prefix))
        .count()
}

fn wait_event_count(path: &Path, prefix: &str, expected_at_least: usize) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let count = event_count(path, prefix);
        if count >= expected_at_least {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected at least {expected_at_least} {prefix:?} events, found {count}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
#[ignore = "requires disposable AsyncSSH fixture: scripts/test-native-ssh-smoke.py"]
fn native_authenticated_terminal_trust_resize_reconnect_and_cleanup() {
    let fixture = fixture();
    let events = fixture.join("server.log");

    let (mut backend, receiver) = open(&fixture, 9801, "config");
    wait_text(&mut backend, "NATIVE_SMOKE_READY");
    write(&mut backend, "echo:native-roundtrip\n");
    wait_text(&mut backend, "NATIVE_ECHO:native-roundtrip");

    backend.process_command(BackendCommand::Resize(
        terminal_core::Size::new(970.0, 310.0),
        terminal_core::Size::new(10.0, 10.0),
    ));
    let resize_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        write(&mut backend, "size\n");
        thread::sleep(Duration::from_millis(100));
        if grid(&mut backend).contains("NATIVE_SIZE:31 97") {
            break;
        }
        assert!(
            Instant::now() < resize_deadline,
            "native SSH resize did not converge: {}",
            grid(&mut backend)
        );
    }
    assert_eq!(backend.sync().grid.display_iter().count(), 31 * 97);

    write(&mut backend, "exit\n");
    wait_exit(&receiver, 9801);
    drop(backend);
    wait_event_count(&events, "CONNECTION_CLOSED:", 1);
    wait_event_count(&events, "SESSION_CLOSED:", 1);

    let (mut reconnected, reconnect_receiver) = open(&fixture, 9802, "config");
    wait_text(&mut reconnected, "NATIVE_SMOKE_READY");
    write(&mut reconnected, "echo:reconnect-ok\n");
    wait_text(&mut reconnected, "NATIVE_ECHO:reconnect-ok");
    write(&mut reconnected, "exit\n");
    wait_exit(&reconnect_receiver, 9802);
    drop(reconnected);
    wait_event_count(&events, "CONNECTION_CLOSED:", 2);
    wait_event_count(&events, "SESSION_CLOSED:", 2);

    let auth_before_changed_key = event_count(&events, "AUTH_ACCEPT:");
    let (mut rejected, rejected_receiver) = open(&fixture, 9803, "changed-config");
    wait_exit(&rejected_receiver, 9803);
    let rejection = grid(&mut rejected).to_ascii_lowercase();
    assert!(
        rejection.contains("host identification has changed")
            || rejection.contains("host key verification failed")
            || rejection.contains("@@@@@@@@@@"),
        "changed host key was not surfaced by system OpenSSH: {rejection}"
    );
    drop(rejected);

    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        event_count(&events, "AUTH_ACCEPT:"),
        auth_before_changed_key,
        "changed-host-key connection reached user authentication"
    );
    wait_event_count(&events, "CONNECTION_CLOSED:", 3);

    let log = fs::read_to_string(&events).unwrap();
    assert!(log.contains("AUTH_ACCEPT:native-smoke"));
    assert!(log.contains("RESIZE:97x31"));
    assert_eq!(
        event_count(&events, "SESSION_OPEN"),
        event_count(&events, "SESSION_CLOSED:"),
        "native SSH smoke left a server-side session open"
    );

    println!(
        "PASS native OpenSSH authentication, terminal I/O, PTY resize, reconnect, changed-host-key rejection and cleanup"
    );
}
