//! Opt-in Linux loopback verification: scripts/test-ssh-integration.sh.
use egui_term::{BackendCommand, PtyEvent, TerminalBackend};
use inspirum_terminal::{Session, SshOptions, terminal::connect};
use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
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

fn start_echo_once() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut buffer = [0_u8; 128];
        let bytes = stream.read(&mut buffer).unwrap();
        assert!(bytes > 0, "echo target received no data");
        stream.write_all(&buffer[..bytes]).unwrap();
    });
    (port, handle)
}

fn reserve_forward_ports() -> [u16; 3] {
    let listeners: Vec<TcpListener> = (0..3)
        .map(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap())
        .collect();
    let ports = [
        listeners[0].local_addr().unwrap().port(),
        listeners[1].local_addr().unwrap().port(),
        listeners[2].local_addr().unwrap().port(),
    ];
    drop(listeners);
    ports
}

fn connect_loopback(port: u16) -> TcpStream {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect_timeout(&address, Duration::from_millis(200)) {
            Ok(stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                return stream;
            }
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "forward listener {address} did not become ready: {error}"
                );
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn assert_echo(port: u16, payload: &[u8]) {
    let mut stream = connect_loopback(port);
    stream.write_all(payload).unwrap();
    let mut echoed = vec![0_u8; payload.len()];
    stream.read_exact(&mut echoed).unwrap();
    assert_eq!(echoed, payload);
}

fn assert_socks5_echo(socks_port: u16, target_port: u16, payload: &[u8]) {
    let mut stream = connect_loopback(socks_port);
    stream.write_all(&[0x05, 0x01, 0x00]).unwrap();
    let mut greeting = [0_u8; 2];
    stream.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [0x05, 0x00], "SOCKS5 no-auth negotiation failed");

    let [high, low] = target_port.to_be_bytes();
    stream
        .write_all(&[0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, high, low])
        .unwrap();
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).unwrap();
    assert_eq!(header[0], 0x05, "unexpected SOCKS version");
    assert_eq!(header[1], 0x00, "SOCKS CONNECT failed with {}", header[1]);
    let address_len = match header[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len = [0_u8; 1];
            stream.read_exact(&mut len).unwrap();
            usize::from(len[0])
        }
        other => panic!("unexpected SOCKS address type {other:#x}"),
    };
    let mut bound_address_and_port = vec![0_u8; address_len + 2];
    stream.read_exact(&mut bound_address_and_port).unwrap();

    stream.write_all(payload).unwrap();
    let mut echoed = vec![0_u8; payload.len()];
    stream.read_exact(&mut echoed).unwrap();
    assert_eq!(echoed, payload);
}

fn wait_listener_closed(port: u16) {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            Err(_) => return,
            Ok(stream) => drop(stream),
        }
        assert!(
            Instant::now() < deadline,
            "forward listener {address} survived terminal shutdown"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_file_contains(path: &std::path::Path, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text = fs::read_to_string(path).unwrap_or_default();
        if text.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "missing {needle:?} in {}: {text}",
            path.display()
        );
        thread::sleep(Duration::from_millis(50));
    }
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
    open_with_ssh(p, id, strict, cfg, SshOptions::default())
}

fn open_with_ssh(
    p: &std::path::Path,
    id: u64,
    strict: bool,
    cfg: &str,
    ssh: SshOptions,
) -> (TerminalBackend, mpsc::Receiver<(u64, PtyEvent)>) {
    let (tx, rx) = mpsc::channel();
    let s = Session {
        name: "Disposable loopback".into(),
        host: "127.0.0.1".into(),
        strict,
        ssh,
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
fn remote_command_is_sent_after_authentication() {
    let p = fixture();
    let command = "printf canary && whoami";
    let ssh = SshOptions {
        remote_command: command.into(),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 708, true, "config", ssh);
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
    wait_text(&mut backend, &format!("REMOTE_COMMAND:{command}"));
    wait_exit(&rx, 708);
    println!("PASS remote command is delivered after SSH authentication");
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
fn proxy_jump_authenticates_through_disposable_bastion() {
    let p = fixture();
    let ssh = SshOptions {
        proxy_jump: "fixture-jump".into(),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 707, true, "config", ssh);
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
    write(&mut backend, "echo:proxy-jump-707\n");
    wait_text(&mut backend, "REMOTE_ECHO:proxy-jump-707");
    write(&mut backend, "exit\n");
    wait_exit(&rx, 707);
    drop(backend);

    wait_file_contains(&p.join("jump_sshd.log"), "Accepted publickey");
    println!("PASS ProxyJump authenticated through disposable bastion and reached target");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn local_remote_and_dynamic_forwarding_round_trip_and_teardown() {
    let p = fixture();
    let (local_target, local_echo) = start_echo_once();
    let (remote_target, remote_echo) = start_echo_once();
    let (dynamic_target, dynamic_echo) = start_echo_once();
    let [local_port, remote_port, socks_port] = reserve_forward_ports();

    let ssh = SshOptions {
        local_forwards: vec![format!("127.0.0.1:{local_port}:127.0.0.1:{local_target}")],
        remote_forwards: vec![format!("127.0.0.1:{remote_port}:127.0.0.1:{remote_target}")],
        dynamic_forwards: vec![format!("127.0.0.1:{socks_port}")],
        ..SshOptions::default()
    };
    let (mut backend, _rx) = open_with_ssh(&p, 706, true, "config", ssh);
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");

    assert_echo(local_port, b"local-forward-706");
    assert_echo(remote_port, b"remote-forward-706");
    assert_socks5_echo(socks_port, dynamic_target, b"dynamic-forward-706");

    local_echo.join().unwrap();
    remote_echo.join().unwrap();
    dynamic_echo.join().unwrap();

    drop(backend);
    wait_listener_closed(local_port);
    wait_listener_closed(remote_port);
    wait_listener_closed(socks_port);
    println!(
        "PASS local, remote and SOCKS5 dynamic forwarding round trips and listeners close on disconnect"
    );
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
