use egui_term::{BackendCommand, PtyEvent, TerminalBackend};
use inspirum_terminal::{Session, terminal::connect};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn fixture() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("INSPIRUM_NATIVE_SSH_FIXTURE")
            .expect("run scripts/test-native-ssh-smoke.py; missing fixture is not a skip"),
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
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let text = grid(backend);
        if text.contains(needle) {
            return;
        }
        assert!(Instant::now() < deadline, "missing {needle:?}: {text}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn write_line(backend: &mut TerminalBackend, text: &str) {
    let ending = if cfg!(windows) { "\r" } else { "\n" };
    backend.process_command(BackendCommand::Write(
        format!("{text}{ending}").into_bytes(),
    ));
}

fn wait_exit(receiver: &mpsc::Receiver<(u64, PtyEvent)>, id: u64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok((actual, PtyEvent::Exit)) = receiver.recv_timeout(Duration::from_millis(100)) {
            assert_eq!(actual, id);
            return;
        }
    }
    panic!("PTY {id} never emitted Exit");
}

fn wait_cleanup(marker: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "remote process cleanup marker was not written"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn open(
    fixture: &Path,
    id: u64,
    config: &str,
) -> (TerminalBackend, mpsc::Receiver<(u64, PtyEvent)>) {
    let (sender, receiver) = mpsc::channel();
    let session = Session {
        name: "Native disposable SSH".into(),
        host: "native-fixture".into(),
        strict: true,
        ..Session::default()
    };
    let backend = connect(
        id,
        eframe::egui::Context::default(),
        sender,
        &session,
        Some(&fixture.join(config)),
    )
    .expect("real terminal::connect through native OpenSSH");
    (backend, receiver)
}

#[test]
#[ignore = "requires scripts/test-native-ssh-smoke.py"]
fn native_auth_io_resize_trust_reconnect_and_cleanup() {
    let fixture = fixture();
    let cleanup = fixture.join("cleanup.marker");
    let _ = fs::remove_file(&cleanup);

    let (mut backend, receiver) = open(&fixture, 801, "config");
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
    write_line(&mut backend, "echo:native-roundtrip-801");
    wait_text(&mut backend, "REMOTE_ECHO:native-roundtrip-801");

    backend.process_command(BackendCommand::Resize(
        eframe::egui::vec2(970.0, 310.0).into(),
        eframe::egui::vec2(10.0, 10.0).into(),
    ));
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        write_line(&mut backend, "size");
        thread::sleep(Duration::from_millis(100));
        let text = grid(&mut backend);
        if text.contains("REMOTE_SIZE:31 97") {
            break;
        }
        assert!(Instant::now() < deadline, "remote resize failed: {text}");
    }
    assert_eq!(backend.sync().grid.display_iter().count(), 31 * 97);

    write_line(&mut backend, "exit");
    wait_exit(&receiver, 801);
    wait_cleanup(&cleanup);
    println!("PASS native authenticated terminal I/O, remote PTY resize and clean exit");

    let (mut changed, changed_receiver) = open(&fixture, 802, "changed-config");
    wait_exit(&changed_receiver, 802);
    let changed_text = grid(&mut changed);
    assert!(
        changed_text.contains("REMOTE HOST IDENTIFICATION HAS CHANGED"),
        "{changed_text}"
    );
    assert!(
        !changed_text.contains("FIXTURE_AUTHENTICATED"),
        "changed host key reached authenticated remote command"
    );
    println!("PASS native changed host key rejection before authentication");

    for id in 803..805 {
        let _ = fs::remove_file(&cleanup);
        let (mut reconnected, _receiver) = open(&fixture, id, "config");
        wait_text(&mut reconnected, "FIXTURE_AUTHENTICATED");
        write_line(&mut reconnected, &format!("echo:reconnect-{id}"));
        wait_text(&mut reconnected, &format!("REMOTE_ECHO:reconnect-{id}"));
        drop(reconnected);
        wait_cleanup(&cleanup);
    }
    println!("PASS native reconnect and backend-drop remote process cleanup");
}
