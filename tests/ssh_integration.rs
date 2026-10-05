//! Opt-in Linux loopback verification: scripts/test-ssh-integration.sh.
use egui_term::{BackendCommand, PtyEvent, TerminalBackend};
use inspirum_terminal::{Session, terminal::connect};
use std::{
    fs,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
fn grid(b: &mut TerminalBackend) -> String {
    b.sync().grid.display_iter().map(|c| c.c).collect()
}
fn wait_text(b: &mut TerminalBackend, needle: &str) {
    let end = Instant::now() + Duration::from_secs(12);
    loop {
        let text = grid(b);
        if text.contains(needle) {
            return;
        }
        assert!(Instant::now() < end, "missing {needle:?}: {text}");
        thread::sleep(Duration::from_millis(25));
    }
}
fn write(b: &mut TerminalBackend, s: &str) {
    b.process_command(BackendCommand::Write(s.as_bytes().to_vec()));
}
fn wait_exit(rx: &mpsc::Receiver<(u64, PtyEvent)>, id: u64) {
    let end = Instant::now() + Duration::from_secs(12);
    while Instant::now() < end {
        if let Ok((actual, PtyEvent::Exit)) = rx.recv_timeout(Duration::from_millis(50)) {
            assert_eq!(actual, id);
            return;
        }
    }
    panic!("PTY {id} never emitted Exit");
}
fn subscription_threads() -> usize {
    fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|task| fs::read_to_string(task.path().join("comm")).ok())
        .filter(|name| name.trim() == "pty_event_subsc")
        .count()
}
fn fixture() -> PathBuf {
    let p = PathBuf::from(
        std::env::var_os("INSPIRUM_SSH_FIXTURE")
            .expect("run scripts/test-ssh-integration.sh; missing fixture is not a skip"),
    );
    assert!(p.join("config").is_file());
    p
}
fn open(
    p: &std::path::Path,
    id: u64,
    strict: bool,
    cfg: &str,
) -> (TerminalBackend, mpsc::Receiver<(u64, PtyEvent)>) {
    let (tx, rx) = mpsc::channel();
    let s = Session {
        name: "Disposable loopback".into(),
        host: "127.0.0.1".into(),
        strict,
        ..Session::default()
    };
    (
        connect(
            id,
            eframe::egui::Context::default(),
            tx,
            &s,
            Some(&p.join(cfg)),
        )
        .expect("real terminal::connect"),
        rx,
    )
}
#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn authenticated_grid_input_resize_and_exit() {
    let p = fixture();
    let (mut b, rx) = open(&p, 701, true, "config");
    wait_text(&mut b, "FIXTURE_AUTHENTICATED");
    write(&mut b, "echo:astra-roundtrip-701\n");
    wait_text(&mut b, "REMOTE_ECHO:astra-roundtrip-701");
    b.process_command(BackendCommand::Resize(
        eframe::egui::vec2(970.0, 310.0).into(),
        eframe::egui::vec2(10.0, 10.0).into(),
    ));
    let end = Instant::now() + Duration::from_secs(8);
    loop {
        write(&mut b, "size\n");
        thread::sleep(Duration::from_millis(100));
        if grid(&mut b).contains("REMOTE_SIZE:31 97") {
            break;
        }
        assert!(Instant::now() < end, "resize failed: {}", grid(&mut b));
    }
    assert_eq!(b.sync().grid.display_iter().count(), 31 * 97);
    println!("PASS authenticated grid, input roundtrip, remote PTY and grid resize 31x97");
    write(&mut b, "exit\n");
    wait_exit(&rx, 701);
    println!("PASS remote exit emitted PtyEvent::Exit");
}
#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn changed_host_key_rejected_even_in_ask_mode() {
    let p = fixture();
    let (mut b, rx) = open(&p, 702, false, "changed-config");
    wait_exit(&rx, 702);
    let text = grid(&mut b);
    assert!(
        text.contains("REMOTE HOST IDENTIFICATION HAS CHANGED"),
        "{text}"
    );
    assert!(text.contains("Host key verification failed"), "{text}");
    assert!(!text.contains("FIXTURE_AUTHENTICATED"), "{text}");
    println!("PASS changed pinned host key rejected with StrictHostKeyChecking=ask");
}
#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn dropping_backend_disconnects_remote_process_and_joins_subscription_thread() {
    let p = fixture();
    let baseline_threads = subscription_threads();
    for id in 703..706 {
        let _ = fs::remove_file(p.join("remote.pid"));
        let (mut backend, _rx) = open(&p, id, true, "config");
        wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
        let pid: u32 = fs::read_to_string(p.join("remote.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let process = PathBuf::from(format!("/proc/{pid}"));
        assert!(process.exists());
        drop(backend);
        let end = Instant::now() + Duration::from_secs(8);
        while process.exists() {
            assert!(
                Instant::now() < end,
                "remote process {pid} survived backend drop"
            );
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(
            subscription_threads(),
            baseline_threads,
            "PTY subscription thread leaked after closing backend {id}"
        );
        println!(
            "PASS backend {id} drop terminates remote process {pid} and joins subscription thread"
        );
    }
}
