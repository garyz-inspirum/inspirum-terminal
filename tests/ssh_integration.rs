//! Opt-in Linux loopback verification: scripts/test-ssh-integration.sh.
use egui_term::{BackendCommand, PtyEvent, TerminalBackend};
use inspirum_terminal::{
    ControlMasterMode, ProxyKind, Session, SshOptions,
    terminal::{
        connect, connect_sftp, control_master_operation, launch_args, launch_args_with_proxy_helper,
    },
};
use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
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

fn wait_text_case_insensitive_secret_safe(b: &mut TerminalBackend, needle: &str) {
    let needle = needle.to_lowercase();
    let end = Instant::now() + Duration::from_secs(12);
    loop {
        if grid(b).to_lowercase().contains(&needle) {
            return;
        }
        assert!(
            Instant::now() < end,
            "expected authentication prompt was not observed"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn privileged_auth_fixture_available() -> bool {
    matches!(
        std::env::var("INSPIRUM_PRIV_AUTH_FIXTURE").as_deref(),
        Ok("1")
    )
}

fn fixture_password() -> String {
    std::env::var("INSPIRUM_FIXTURE_PASSWORD")
        .expect("scripts/test-ssh-integration.sh must supply fixture password")
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

fn start_stalled_ssh_once() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(5));
        drop(stream);
    });
    (port, handle)
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
fn open_sftp(p: &std::path::Path, id: u64) -> (TerminalBackend, mpsc::Receiver<(u64, PtyEvent)>) {
    let (tx, rx) = mpsc::channel();
    let session = Session {
        name: "Disposable SFTP".into(),
        host: "fixture-sftp".into(),
        strict: true,
        ..Session::default()
    };
    (
        connect_sftp(
            id,
            eframe::egui::Context::default(),
            tx,
            &session,
            Some(&p.join("config")),
        )
        .expect("real terminal::connect_sftp"),
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
fn encrypted_private_key_authenticates_through_pty_prompt() {
    let p = fixture();
    let ssh = SshOptions {
        public_key_auth: Some(true),
        password_auth: Some(false),
        keyboard_interactive_auth: Some(false),
        gssapi_auth: Some(false),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 710, true, "encrypted-config", ssh);
    wait_text(&mut backend, "Enter passphrase for key");
    write(&mut backend, "fixture-passphrase\n");
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
    assert!(
        !grid(&mut backend).contains("fixture-passphrase"),
        "private-key passphrase was echoed into the terminal grid"
    );
    write(&mut backend, "exit\n");
    wait_exit(&rx, 710);
    println!("PASS encrypted private key authenticated through PTY prompt without echo");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn ssh_agent_authenticates_without_identity_file_secret_storage() {
    let p = fixture();
    let ssh = SshOptions {
        public_key_auth: Some(true),
        password_auth: Some(false),
        keyboard_interactive_auth: Some(false),
        gssapi_auth: Some(false),
        identities_only: Some(false),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 711, true, "agent-config", ssh);
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
    assert!(
        !grid(&mut backend).contains("Enter passphrase"),
        "agent-backed authentication unexpectedly prompted for a private-key passphrase"
    );
    write(&mut backend, "exit\n");
    wait_exit(&rx, 711);
    println!("PASS SSH agent authenticated through the application PTY path");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn password_authenticates_through_pty_prompt_without_echo_or_log_disclosure() {
    if !privileged_auth_fixture_available() {
        eprintln!("SKIP password acceptance: passwordless sudo fixture unavailable");
        return;
    }
    let p = fixture();
    let secret = fixture_password();
    let ssh = SshOptions {
        public_key_auth: Some(false),
        password_auth: Some(true),
        keyboard_interactive_auth: Some(false),
        gssapi_auth: Some(false),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 712, true, "password-config", ssh);
    wait_text_case_insensitive_secret_safe(&mut backend, "password:");
    write(&mut backend, &format!("{secret}\n"));
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
    assert!(
        !grid(&mut backend).contains(&secret),
        "password was echoed into the terminal grid"
    );
    write(&mut backend, "exit\n");
    wait_exit(&rx, 712);
    wait_file_contains(&p.join("password_sshd.log"), "Accepted password");
    assert!(
        !fs::read_to_string(p.join("password_sshd.log"))
            .unwrap()
            .contains(&secret),
        "password was written to the sshd log"
    );
    println!("PASS password authentication completed through PTY prompt without secret disclosure");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn public_key_plus_keyboard_interactive_pam_mfa_authenticates() {
    if !privileged_auth_fixture_available() {
        eprintln!("SKIP MFA acceptance: passwordless sudo fixture unavailable");
        return;
    }
    let p = fixture();
    let secret = fixture_password();
    let ssh = SshOptions {
        public_key_auth: Some(true),
        password_auth: Some(false),
        keyboard_interactive_auth: Some(true),
        gssapi_auth: Some(false),
        identities_only: Some(true),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 713, true, "mfa-config", ssh);
    wait_text_case_insensitive_secret_safe(&mut backend, "password:");
    write(&mut backend, &format!("{secret}\n"));
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
    assert!(
        !grid(&mut backend).contains(&secret),
        "keyboard-interactive response was echoed into the terminal grid"
    );
    write(&mut backend, "exit\n");
    wait_exit(&rx, 713);

    wait_file_contains(&p.join("mfa_sshd.log"), "publickey");
    wait_file_contains(&p.join("mfa_sshd.log"), "keyboard-interactive");
    let log = fs::read_to_string(p.join("mfa_sshd.log")).unwrap();
    assert!(
        !log.contains(&secret),
        "keyboard-interactive response was written to the sshd log"
    );
    println!(
        "PASS public-key plus keyboard-interactive PAM MFA authenticated through application PTY"
    );
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn disabled_password_policy_does_not_prompt_or_authenticate() {
    if !privileged_auth_fixture_available() {
        eprintln!("SKIP password policy negative case: passwordless sudo fixture unavailable");
        return;
    }
    let p = fixture();
    let ssh = SshOptions {
        public_key_auth: Some(false),
        password_auth: Some(false),
        keyboard_interactive_auth: Some(false),
        gssapi_auth: Some(false),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 714, true, "password-config", ssh);
    wait_exit(&rx, 714);
    let text = grid(&mut backend).to_lowercase();
    assert!(!text.contains("fixture_authenticated"));
    assert!(!text.contains("password:"));
    println!("PASS disabled password policy prevented prompt and authentication");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn disabled_keyboard_interactive_policy_blocks_second_mfa_factor() {
    if !privileged_auth_fixture_available() {
        eprintln!("SKIP MFA policy negative case: passwordless sudo fixture unavailable");
        return;
    }
    let p = fixture();
    let ssh = SshOptions {
        public_key_auth: Some(true),
        password_auth: Some(false),
        keyboard_interactive_auth: Some(false),
        gssapi_auth: Some(false),
        identities_only: Some(true),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 715, true, "mfa-config", ssh);
    wait_exit(&rx, 715);
    let text = grid(&mut backend).to_lowercase();
    assert!(!text.contains("fixture_authenticated"));
    assert!(!text.contains("password:"));
    println!("PASS disabled keyboard-interactive policy blocked required MFA factor");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn password_prompt_can_be_cancelled_without_authentication() {
    if !privileged_auth_fixture_available() {
        eprintln!("SKIP password cancellation: passwordless sudo fixture unavailable");
        return;
    }
    let p = fixture();
    let ssh = SshOptions {
        public_key_auth: Some(false),
        password_auth: Some(true),
        keyboard_interactive_auth: Some(false),
        gssapi_auth: Some(false),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 716, true, "password-config", ssh);
    wait_text_case_insensitive_secret_safe(&mut backend, "password:");
    write(&mut backend, "\u{3}");
    wait_exit(&rx, 716);
    assert!(!grid(&mut backend).contains("FIXTURE_AUTHENTICATED"));
    println!("PASS password prompt cancellation exited without authentication");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn connect_timeout_terminates_stalled_ssh_handshake() {
    let p = fixture();
    let (port, stalled_server) = start_stalled_ssh_once();
    let (tx, rx) = mpsc::channel();
    let session = Session {
        name: "Stalled handshake".into(),
        host: "127.0.0.1".into(),
        port: Some(port),
        strict: true,
        ssh: SshOptions {
            connect_timeout_seconds: Some(1),
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let started = Instant::now();
    let mut backend = connect(
        717,
        eframe::egui::Context::default(),
        tx,
        &session,
        Some(&p.join("config")),
    )
    .expect("start stalled OpenSSH client");
    wait_exit(&rx, 717);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "ConnectTimeout=1 did not bound the stalled SSH handshake"
    );
    assert!(!grid(&mut backend).contains("FIXTURE_AUTHENTICATED"));
    stalled_server.join().unwrap();
    println!("PASS ConnectTimeout bounded a stalled SSH handshake through application PTY");
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
fn sftp_upload_download_round_trip_and_exit() {
    let p = fixture();
    let source = p.join("sftp-source.bin");
    let downloaded = p.join("sftp-downloaded.bin");
    let payload = b"inspirum-sftp-round-trip-709\0with-binary\xff";
    fs::write(&source, payload).unwrap();

    let (mut backend, rx) = open_sftp(&p, 709);
    wait_text(&mut backend, "sftp>");

    write(
        &mut backend,
        &format!("put \"{}\" upload.bin\n", source.display()),
    );
    write(
        &mut backend,
        &format!("get upload.bin \"{}\"\n", downloaded.display()),
    );
    write(&mut backend, "quit\n");
    wait_exit(&rx, 709);

    assert_eq!(fs::read(&downloaded).unwrap(), payload);
    assert_eq!(fs::read(p.join("sftp-root/upload.bin")).unwrap(), payload);
    println!("PASS SFTP upload/download preserved bytes and exited cleanly");
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

fn start_http_connect_proxy(deny: bool) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut client, _) = listener.accept().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(8)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 16 * 1024, "oversized CONNECT request");
            client.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let text = String::from_utf8(request).unwrap();
        let authority = text
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("CONNECT "))
            .and_then(|rest| rest.strip_suffix(" HTTP/1.1"))
            .expect("valid CONNECT request");
        if deny {
            client
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            return;
        }
        let (host, port) = authority.rsplit_once(':').expect("host:port");
        let host = host.trim_matches(['[', ']']);
        let port: u16 = port.parse().unwrap();
        let mut target = TcpStream::connect((host, port)).unwrap();
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .unwrap();
        client.set_read_timeout(None).unwrap();

        let mut client_read = client.try_clone().unwrap();
        let mut target_write = target.try_clone().unwrap();
        let up = thread::spawn(move || {
            let _ = std::io::copy(&mut client_read, &mut target_write);
            let _ = target_write.shutdown(std::net::Shutdown::Write);
        });
        let _ = std::io::copy(&mut target, &mut client);
        let _ = client.shutdown(std::net::Shutdown::Both);
        let _ = target.shutdown(std::net::Shutdown::Both);
        let _ = up.join();
    });
    (port, handle)
}

fn structured_proxy_ssh_output(p: &std::path::Path, proxy_port: u16) -> std::process::Output {
    let session = Session {
        name: "Disposable proxy route".into(),
        host: "127.0.0.1".into(),
        strict: true,
        ssh: SshOptions {
            proxy_kind: ProxyKind::HttpConnect,
            proxy_host: "127.0.0.1".into(),
            proxy_port: Some(proxy_port),
            remote_command: "exit".into(),
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_inspirum-terminal"));
    let args = launch_args_with_proxy_helper(&session, Some(&p.join("config")), &helper).unwrap();
    let mut child = Command::new("ssh")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run real OpenSSH through structured proxy helper");
    child
        .stdin
        .take()
        .expect("proxy SSH stdin")
        .write_all(b"exit\n")
        .expect("request disposable remote exit");
    child.wait_with_output().expect("wait for proxy SSH")
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn structured_http_proxy_routes_authenticated_ssh_through_connect_tunnel() {
    let p = fixture();
    let (proxy_port, _proxy) = start_http_connect_proxy(false);
    let output = structured_proxy_ssh_output(&p, proxy_port);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("FIXTURE_AUTHENTICATED"),
        "real OpenSSH did not authenticate through proxy; status={:?}, stdout={stdout:?}, stderr={stderr:?}",
        output.status.code()
    );
    println!("PASS structured HTTP CONNECT proxy carried the authenticated SSH session");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn structured_proxy_denial_never_falls_back_to_direct_ssh() {
    let p = fixture();
    let (proxy_port, _proxy) = start_http_connect_proxy(true);
    let output = structured_proxy_ssh_output(&p, proxy_port);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "SSH unexpectedly succeeded after proxy denial"
    );
    assert!(
        !stdout.contains("FIXTURE_AUTHENTICATED"),
        "SSH reached directly reachable target after proxy denial: {stdout}"
    );
    println!("PASS proxy denial terminated SSH without direct-transport fallback");
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn controlmaster_lifecycle_check_and_explicit_close() {
    let p = fixture();
    let socket = p.join("inspirum-control-%C.sock");
    let session = Session {
        name: "ControlMaster fixture".into(),
        host: "127.0.0.1".into(),
        strict: true,
        ssh: SshOptions {
            control_master: ControlMasterMode::Auto,
            control_path: socket.to_string_lossy().into_owned(),
            control_persist_seconds: Some(30),
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let args = launch_args(&session, Some(&p.join("config"))).unwrap();
    let mut child = Command::new("ssh")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"exit\n").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    control_master_operation(&session, Some(&p.join("config")), "check")
        .expect("persisted master should be active");
    control_master_operation(&session, Some(&p.join("config")), "exit")
        .expect("explicit master close should succeed");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if control_master_operation(&session, Some(&p.join("config")), "check").is_err() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "ControlMaster survived explicit close"
        );
        thread::sleep(Duration::from_millis(50));
    }
}
