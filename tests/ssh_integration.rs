//! Opt-in Linux loopback verification: scripts/test-ssh-integration.sh.
use inspirum_terminal::{
    ControlMasterMode, ProxyKind, Session, SshOptions,
    remote_edit::{RemoteEdit, SaveOutcome},
    scp, sftp,
    terminal::{
        connect, connect_sftp, control_master_operation, launch_args,
        launch_args_with_proxy_helper, start_tunnels,
    },
    tmux,
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
use terminal_core::{BackendCommand, PtyEvent, TerminalBackend};
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
        connect(id, tx, &s, Some(&p.join(cfg))).expect("real terminal::connect"),
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
        connect_sftp(id, tx, &session, Some(&p.join("config")))
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
        terminal_core::Size::new(970.0, 310.0),
        terminal_core::Size::new(10.0, 10.0),
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
fn explicit_cipher_policy_matches_restricted_disposable_sshd() {
    let p = fixture();
    let ssh = SshOptions {
        ciphers: "aes256-ctr".into(),
        ..SshOptions::default()
    };
    let (mut backend, rx) = open_with_ssh(&p, 715, true, "config", ssh);
    wait_text(&mut backend, "FIXTURE_AUTHENTICATED");
    write(&mut backend, "exit\n");
    wait_exit(&rx, 715);

    let mismatched = SshOptions {
        ciphers: "aes128-gcm@openssh.com".into(),
        ..SshOptions::default()
    };
    let (mut rejected, rejected_rx) = open_with_ssh(&p, 716, true, "config", mismatched);
    wait_exit(&rejected_rx, 716);
    let text = grid(&mut rejected).to_lowercase();
    assert!(
        text.contains("no matching cipher") || text.contains("no matching cipher found"),
        "restricted-cipher mismatch was not surfaced: {text}"
    );
    assert!(!text.contains("fixture_authenticated"));
    println!("PASS explicit cipher policy connects only when it matches restricted sshd policy");
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
fn agent_forwarding_respects_explicit_profile_consent() {
    let fixture = fixture();
    let (mut enabled, enabled_events) = open_with_ssh(
        &fixture,
        717,
        true,
        "agent-forward-config",
        SshOptions {
            agent_forwarding: Some(true),
            ..SshOptions::default()
        },
    );
    wait_text(&mut enabled, "FIXTURE_AUTHENTICATED");
    write(&mut enabled, "agent-probe\n");
    wait_text(&mut enabled, "AGENT_FORWARDED");
    write(&mut enabled, "exit\n");
    wait_exit(&enabled_events, 717);

    // The server is still agent-forwarding capable. The application's
    // explicit disable must prevent exposing the local agent socket there.
    let (mut disabled, disabled_events) = open_with_ssh(
        &fixture,
        718,
        true,
        "agent-forward-config",
        SshOptions {
            agent_forwarding: Some(false),
            ..SshOptions::default()
        },
    );
    wait_text(&mut disabled, "FIXTURE_AUTHENTICATED");
    write(&mut disabled, "agent-probe\n");
    wait_text(&mut disabled, "AGENT_NO_SOCKET");
    assert!(!grid(&mut disabled).contains("AGENT_FORWARDED"));
    write(&mut disabled, "exit\n");
    wait_exit(&disabled_events, 718);
    println!("PASS agent socket forwarding requires explicit per-session consent");
}

#[test]
#[ignore = "requires disposable Xvfb+xauth+sshd: scripts/test-ssh-integration.sh"]
fn x11_forwarding_respects_explicit_profile_consent() {
    let fixture = fixture();
    let (mut enabled, enabled_events) = open_with_ssh(
        &fixture,
        719,
        true,
        "config",
        SshOptions {
            x11_forwarding: Some(true),
            ..SshOptions::default()
        },
    );
    wait_text(&mut enabled, "FIXTURE_AUTHENTICATED");
    write(&mut enabled, "x11-probe\n");
    wait_text(&mut enabled, "X11_FORWARDED");
    write(&mut enabled, "exit\n");
    wait_exit(&enabled_events, 719);

    // Explicitly opting out must not expose a DISPLAY from the remote
    // session, even though the sshd is X11-forwarding capable.
    let (mut disabled, disabled_events) = open_with_ssh(
        &fixture,
        720,
        true,
        "config",
        SshOptions {
            x11_forwarding: Some(false),
            ..SshOptions::default()
        },
    );
    wait_text(&mut disabled, "FIXTURE_AUTHENTICATED");
    write(&mut disabled, "x11-probe\n");
    wait_text(&mut disabled, "X11_NO_DISPLAY");
    assert!(!grid(&mut disabled).contains("X11_FORWARDED"));
    write(&mut disabled, "exit\n");
    wait_exit(&disabled_events, 720);
    println!("PASS native SSH X11 forwarding requires an explicit client policy");
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
    let mut backend =
        connect(717, tx, &session, Some(&p.join("config"))).expect("start stalled OpenSSH client");
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
    // A rejected OpenSSH client can exit almost immediately. Observe its
    // diagnostic through the live PTY before relying on the Exit event;
    // checking only after Exit races the terminal reader under CI load.
    wait_text(&mut b, "REMOTE HOST IDENTIFICATION HAS CHANGED");
    wait_text(&mut b, "Host key verification failed");
    wait_exit(&rx, 702);
    let text = grid(&mut b);
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
    let socket = PathBuf::from(format!("/tmp/inspirum-cm-{}-%C", std::process::id()));
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

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn tunnel_manager_reports_listener_failure_and_stop_closes_listener() {
    let p = fixture();
    let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let occupied_port = occupied.local_addr().unwrap().port();
    let mut blocked = Session {
        name: "Blocked tunnel fixture".into(),
        host: "127.0.0.1".into(),
        strict: true,
        ssh: SshOptions {
            local_forwards: vec![format!("127.0.0.1:{occupied_port}:127.0.0.1:1")],
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let error = match start_tunnels(&blocked, Some(&p.join("config"))) {
        Ok(_) => panic!("occupied requested listener unexpectedly succeeded"),
        Err(error) => error.to_string(),
    };
    assert!(
        error.contains("tunnel setup failed"),
        "listener failure was not surfaced: {error}"
    );
    drop(occupied);

    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    blocked.ssh.local_forwards = vec![format!("127.0.0.1:{port}:127.0.0.1:1")];
    let mut tunnels = start_tunnels(&blocked, Some(&p.join("config")))
        .expect("forwarding-only manager should stay running");
    TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("managed listener should be ready");
    tunnels.stop().expect("explicit tunnel stop");
    wait_listener_closed(port);
    println!("PASS tunnel manager surfaced listener failure and explicit stop closed listener");
}

fn fixture_sftp_session() -> Session {
    Session {
        name: "Graphical SFTP fixture".into(),
        host: "fixture-sftp".into(),
        strict: true,
        ..Session::default()
    }
}

fn wait_managed_transfer(transfer: &mut sftp::Transfer) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match transfer.poll() {
            Ok(Some(())) => return Ok(()),
            Ok(None) => {
                assert!(Instant::now() < deadline, "managed SFTP transfer timed out");
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn graphical_sftp_operations_are_verified_conflict_safe_and_cancellable() {
    let p = fixture();
    let session = fixture_sftp_session();
    let config = p.join("config");
    let local = p.join("graphical-sftp-local");
    fs::create_dir_all(&local).unwrap();

    let source = local.join("binary source.bin");
    let payload: Vec<u8> = (0..65536).map(|index| (index % 251) as u8).collect();
    fs::write(&source, &payload).unwrap();
    let remote = "browser binary.bin";

    let mut upload = sftp::start_upload(&session, Some(&config), &source, remote)
        .expect("start managed SFTP upload");
    wait_managed_transfer(&mut upload).expect("verified managed SFTP upload");

    let listed = sftp::list_remote(&session, Some(&config), ".").unwrap();
    let uploaded = listed
        .iter()
        .find(|entry| entry.name == remote)
        .expect("uploaded file in graphical listing");
    assert_eq!(uploaded.size, Some(payload.len() as u64));

    let destination = local.join("downloaded binary.bin");
    let mut download = sftp::start_download(
        &session,
        Some(&config),
        remote,
        &destination,
        false,
        Some(payload.len() as u64),
    )
    .expect("start managed SFTP download");
    wait_managed_transfer(&mut download).expect("verified managed SFTP download");
    assert_eq!(fs::read(&destination).unwrap(), payload);

    fs::write(&destination, b"KEEP-EXISTING").unwrap();
    let conflict = sftp::start_download(
        &session,
        Some(&config),
        remote,
        &destination,
        false,
        Some(payload.len() as u64),
    );
    assert!(conflict.is_err(), "unsafe local overwrite was not rejected");
    assert_eq!(fs::read(&destination).unwrap(), b"KEEP-EXISTING");

    let mut bad_integrity = sftp::start_download(
        &session,
        Some(&config),
        remote,
        &destination,
        true,
        Some(payload.len() as u64 + 1),
    )
    .unwrap();
    assert!(
        wait_managed_transfer(&mut bad_integrity).is_err(),
        "wrong expected size unexpectedly passed integrity verification"
    );
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"KEEP-EXISTING",
        "failed verified overwrite modified the original file"
    );

    let mut overwrite = sftp::start_download(
        &session,
        Some(&config),
        remote,
        &destination,
        true,
        Some(payload.len() as u64),
    )
    .unwrap();
    wait_managed_transfer(&mut overwrite).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), payload);

    let cancel_destination = local.join("cancelled.bin");
    let mut cancelled = sftp::start_download(
        &session,
        Some(&config),
        remote,
        &cancel_destination,
        false,
        Some(payload.len() as u64),
    )
    .unwrap();
    let cancelled_partial = cancelled
        .cancel()
        .unwrap()
        .expect("cancelled download should preserve its staging file");
    drop(cancelled);
    assert!(!cancel_destination.exists());
    assert!(cancelled_partial.exists());
    fs::remove_file(cancelled_partial).unwrap();

    let resume_destination = local.join("resumed download.bin");
    let resume_partial = local.join(".resume-download-partial");
    fs::write(&resume_partial, &payload[..8192]).unwrap();
    let mut resumed_download = sftp::start_download_resume(
        &session,
        Some(&config),
        remote,
        &resume_destination,
        &resume_partial,
        false,
        Some(payload.len() as u64),
    )
    .expect("start resumable SFTP download");
    wait_managed_transfer(&mut resumed_download).expect("complete resumable SFTP download");
    assert_eq!(fs::read(&resume_destination).unwrap(), payload);
    assert!(
        !resume_partial.exists(),
        "successful resume left its partial file"
    );

    let resume_remote = "browser resume upload.bin";
    fs::write(p.join("sftp-root").join(resume_remote), &payload[..8192]).unwrap();
    let mut resumed_upload =
        sftp::start_upload_resume(&session, Some(&config), &source, resume_remote)
            .expect("start resumable SFTP upload");
    wait_managed_transfer(&mut resumed_upload).expect("complete resumable SFTP upload");
    assert_eq!(
        fs::read(p.join("sftp-root").join(resume_remote)).unwrap(),
        payload
    );
    sftp::delete_remote(&session, Some(&config), resume_remote, false).unwrap();

    sftp::mkdir_remote(&session, Some(&config), "browser-dir").unwrap();
    sftp::rename_remote(&session, Some(&config), "browser-dir", "browser-renamed").unwrap();
    let listed = sftp::list_remote(&session, Some(&config), ".").unwrap();
    assert!(
        listed
            .iter()
            .any(|entry| entry.name == "browser-renamed" && entry.is_dir)
    );
    sftp::delete_remote(&session, Some(&config), "browser-renamed", true).unwrap();
    sftp::delete_remote(&session, Some(&config), remote, false).unwrap();
    assert!(
        sftp::delete_remote(&session, Some(&config), "missing-entry", false).is_err(),
        "negative remote delete unexpectedly succeeded"
    );
    println!(
        "PASS graphical SFTP browse/mutate, binary transfer, conflict safety, verification, cancellation and upload/download resume"
    );
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn safe_remote_editor_round_trip_conflict_and_binary_policy() {
    let p = fixture();
    let session = fixture_sftp_session();
    let config = p.join("config");
    let root = p.join("sftp-root");
    let remote = "remote-edit.txt";
    let remote_path = root.join(remote);
    fs::write(&remote_path, b"one\ntwo\n").unwrap();

    let mut edit = RemoteEdit::open(&session, Some(&config), remote, Some(8))
        .expect("open remote text editor");
    assert_eq!(edit.text(), "one\ntwo\n");
    edit.text_mut().push_str("three\n");
    assert_eq!(
        edit.save(&session, Some(&config), false).unwrap(),
        SaveOutcome::Saved
    );
    assert_eq!(
        fs::read_to_string(&remote_path).unwrap(),
        "one\ntwo\nthree\n"
    );

    let mut conflicted = RemoteEdit::open(
        &session,
        Some(&config),
        remote,
        Some(fs::metadata(&remote_path).unwrap().len()),
    )
    .unwrap();
    conflicted.text_mut().push_str("editor-change\n");
    fs::write(&remote_path, b"changed-by-someone-else\n").unwrap();
    assert_eq!(
        conflicted.save(&session, Some(&config), false).unwrap(),
        SaveOutcome::Conflict
    );
    assert_eq!(
        fs::read_to_string(&remote_path).unwrap(),
        "changed-by-someone-else\n"
    );
    assert_eq!(
        conflicted.save(&session, Some(&config), true).unwrap(),
        SaveOutcome::Saved
    );
    assert!(
        fs::read_to_string(&remote_path)
            .unwrap()
            .contains("editor-change")
    );

    let binary = "remote-edit.bin";
    let binary_path = root.join(binary);
    fs::write(&binary_path, [0_u8, 1, 2, 0xff, 0x7f]).unwrap();
    assert!(
        RemoteEdit::open(
            &session,
            Some(&config),
            binary,
            Some(fs::metadata(&binary_path).unwrap().len()),
        )
        .is_err(),
        "binary remote file unexpectedly opened for text editing"
    );

    sftp::delete_remote(&session, Some(&config), remote, false).unwrap();
    sftp::delete_remote(&session, Some(&config), binary, false).unwrap();
    println!(
        "PASS safe remote editor text round-trip, conflict guard, confirmed overwrite and binary rejection"
    );
}

fn sha256(path: &std::path::Path) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum must be available in Linux CI");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

fn wait_scp_transfer(transfer: &mut scp::Transfer) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match transfer.poll() {
            Ok(Some(())) => return Ok(()),
            Ok(None) => {
                assert!(Instant::now() < deadline, "managed SCP transfer timed out");
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error),
        }
    }
}

#[test]
#[ignore = "requires disposable sshd: scripts/test-ssh-integration.sh"]
fn scp_binary_round_trip_checksums_match_and_failures_leave_no_success_file() {
    let p = fixture();
    let session = fixture_sftp_session();
    let config = p.join("config");
    let local = p.join("scp-local");
    fs::create_dir_all(&local).unwrap();

    let source = local.join("source binary.bin");
    let payload: Vec<u8> = (0..131072)
        .map(|index| ((index * 31) % 251) as u8)
        .collect();
    fs::write(&source, &payload).unwrap();
    let remote = "scp binary;literal.bin";

    let mut upload = scp::start_upload(&session, Some(&config), &source, remote, false)
        .expect("start managed SCP upload");
    wait_scp_transfer(&mut upload).expect("verified SCP upload");
    let remote_path = p.join("sftp-root").join(remote);
    assert_eq!(sha256(&source), sha256(&remote_path));

    let download = local.join("downloaded binary.bin");
    let mut transfer = scp::start_download(&session, Some(&config), remote, &download, false)
        .expect("start managed SCP download");
    wait_scp_transfer(&mut transfer).expect("verified SCP download");
    assert_eq!(sha256(&source), sha256(&download));

    let overwrite_rejected = scp::start_download(&session, Some(&config), remote, &download, false);
    assert!(
        overwrite_rejected.is_err(),
        "SCP download overwrote an existing local destination without confirmation"
    );

    let original = b"KEEP-EXISTING";
    fs::write(&download, original).unwrap();
    let missing = scp::start_download(
        &session,
        Some(&config),
        "missing-remote.bin",
        &download,
        true,
    );
    assert!(
        missing.is_err(),
        "missing remote SCP source unexpectedly started"
    );
    assert_eq!(fs::read(&download).unwrap(), original);

    let existing_upload = scp::start_upload(&session, Some(&config), &source, remote, false);
    assert!(
        existing_upload.is_err(),
        "SCP upload overwrote an existing remote destination without confirmation"
    );
    assert_eq!(sha256(&source), sha256(&remote_path));

    let bad_remote = "missing-dir/final.bin";
    let mut failed_upload = scp::start_upload(&session, Some(&config), &source, bad_remote, true)
        .expect("SCP child should start before remote path failure");
    assert!(
        wait_scp_transfer(&mut failed_upload).is_err(),
        "SCP upload to missing remote directory unexpectedly succeeded"
    );
    assert!(!p.join("sftp-root/missing-dir/final.bin").exists());
    assert!(
        fs::read_dir(p.join("sftp-root"))
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .contains(".inspirum-scp-")),
        "failed SCP upload left a remote staging file"
    );

    let mut overwrite = scp::start_upload(&session, Some(&config), &source, remote, true)
        .expect("start confirmed SCP overwrite");
    wait_scp_transfer(&mut overwrite).expect("verified confirmed SCP overwrite");
    assert_eq!(sha256(&source), sha256(&remote_path));

    println!(
        "PASS SCP binary SHA-256 upload/download, overwrite guards, staging and negative cleanup"
    );
}

fn fixture_tmux_session() -> Session {
    Session {
        name: "tmux fixture".into(),
        host: "fixture-tmux".into(),
        strict: true,
        ..Session::default()
    }
}

fn wait_tmux_attached(
    session: &Session,
    config: &std::path::Path,
    name: &str,
    expected_attached: bool,
) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let sessions = tmux::list_sessions(session, Some(config)).unwrap();
        if let Some(item) = sessions.iter().find(|item| item.name == name)
            && (item.attached_clients > 0) == expected_attached
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "tmux session {name:?} did not reach attached={expected_attached}; sessions={sessions:?}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn cleanup_tmux_session(session: &Session, config: &std::path::Path, name: &str) {
    let mut cleanup = session.clone();
    cleanup.ssh.remote_command = format!("tmux kill-session -t {name}");
    let output = Command::new("ssh")
        .args(launch_args(&cleanup, Some(config)).unwrap())
        .stdin(Stdio::null())
        .output()
        .expect("run tmux fixture cleanup");
    assert!(
        output.status.success(),
        "tmux cleanup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires disposable sshd and tmux: scripts/test-ssh-integration.sh"]
fn tmux_create_attach_disconnect_and_reconnect_preserve_server_session() {
    let p = fixture();
    let session = fixture_tmux_session();
    let config = p.join("config");
    let name = format!("inspirum{}", std::process::id());

    let before = tmux::list_sessions(&session, Some(&config)).unwrap();
    assert!(
        before.iter().all(|item| item.name != name),
        "unique fixture tmux session already exists"
    );

    let create = tmux::create_session(&session, &name).unwrap();
    let (tx, _rx) = mpsc::channel();
    let mut created =
        connect(719, tx, &create, Some(&config)).expect("create and attach tmux session");
    wait_tmux_attached(&session, &config, &name, true);

    created.process_command(BackendCommand::Write(
        b"printf 'TMUX_CREATE_OK\\n'\n".to_vec(),
    ));
    thread::sleep(Duration::from_millis(100));
    drop(created);
    wait_tmux_attached(&session, &config, &name, false);

    let listed = tmux::list_sessions(&session, Some(&config)).unwrap();
    assert!(
        listed
            .iter()
            .any(|item| item.name == name && item.attached_clients == 0),
        "disconnect did not preserve detached tmux session"
    );

    let attach = tmux::attach_session(&session, &name).unwrap();
    let (tx, _rx) = mpsc::channel();
    let attached = connect(720, tx, &attach, Some(&config)).expect("attach existing tmux session");
    wait_tmux_attached(&session, &config, &name, true);
    drop(attached);
    wait_tmux_attached(&session, &config, &name, false);

    let (tx, _rx) = mpsc::channel();
    let reattached = connect(721, tx, &attach, Some(&config))
        .expect("reconnect by attaching existing tmux session");
    wait_tmux_attached(&session, &config, &name, true);
    drop(reattached);
    wait_tmux_attached(&session, &config, &name, false);

    cleanup_tmux_session(&session, &config, &name);
    assert!(
        tmux::list_sessions(&session, Some(&config))
            .unwrap()
            .iter()
            .all(|item| item.name != name),
        "fixture tmux session survived explicit test cleanup"
    );
    println!(
        "PASS tmux create, selectable discovery, attach, disconnect-detach and attach-only reconnect"
    );
}
