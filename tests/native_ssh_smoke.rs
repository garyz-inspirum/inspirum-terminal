//! Cross-platform native SSH acceptance using scripts/test-native-ssh-smoke.py.
use inspirum_terminal::{
    Session, SshOptions, load_sessions, save_sessions, session_profile_key, terminal::connect,
    terminal_ux::source_first_synced_write, tunnels,
};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
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

fn open_jump(
    fixture: &Path,
    id: u64,
    config_name: &str,
) -> (TerminalBackend, mpsc::Receiver<(u64, PtyEvent)>) {
    let (sender, receiver) = mpsc::channel();
    let session = Session {
        name: "Disposable ProxyJump".into(),
        host: "native-smoke".into(),
        strict: true,
        ssh: SshOptions {
            proxy_jump: "native-hop".into(),
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let backend = connect(id, sender, &session, Some(&fixture.join(config_name)))
        .expect("launch real system OpenSSH through Inspirum with ProxyJump");
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
fn native_cursor_key_and_synchronized_input_reach_only_explicit_ssh_panes() {
    let fixture = fixture();
    let events = fixture.join("server.log");
    let (mut source, source_receiver) = open(&fixture, 9961, "config");
    let (mut mirror, mirror_receiver) = open(&fixture, 9962, "config");
    wait_text(&mut source, "NATIVE_SMOKE_READY");
    wait_text(&mut mirror, "NATIVE_SMOKE_READY");

    let left_before = event_count(&events, "REMOTE_CURSOR_LEFT");
    // The very same source-first fanout used by production Iced runs against
    // two independent authenticated native OpenSSH PTYs. Windows ConPTY can
    // consume ANSI cursor byte sequences as local console-key events instead
    // of forwarding the bytes unchanged. Its *native sync/focus isolation*
    // acceptance therefore uses an unambiguous printable sentinel; remote
    // cursor-byte encoding remains covered by Iced unit tests on Windows.
    // Unix PTYs additionally prove the raw cursor sequence remotely.
    let (left_input, left_reply): (&[u8], &str) = if cfg!(windows) {
        (b"echo:sync-left\n", "NATIVE_ECHO:sync-left")
    } else {
        (b"\x1b[D\n", "NATIVE_REMOTE_CURSOR_LEFT")
    };
    let forwarded = source_first_synced_write(9961, &[9961, 9962], |id| {
        let target = if id == 9961 { &mut source } else { &mut mirror };
        target.process_command(BackendCommand::Write(left_input.to_vec()));
        true
    });
    assert!(forwarded);
    wait_text(&mut source, left_reply);
    wait_text(&mut mirror, left_reply);
    if !cfg!(windows) {
        wait_event_count(&events, "REMOTE_CURSOR_LEFT", left_before + 2);
    }

    // After explicit disarm, one key/send goes only to the focused source.
    let right_before = event_count(&events, "REMOTE_CURSOR_RIGHT");
    let (right_input, right_reply) = if cfg!(windows) {
        ("echo:sync-right\n", "NATIVE_ECHO:sync-right")
    } else {
        ("\x1b[C\n", "NATIVE_REMOTE_CURSOR_RIGHT")
    };
    write(&mut source, right_input);
    wait_text(&mut source, right_reply);
    if !cfg!(windows) {
        wait_event_count(&events, "REMOTE_CURSOR_RIGHT", right_before + 1);
    }
    assert!(
        !grid(&mut mirror).contains(right_reply),
        "disarmed input leaked to the second SSH PTY"
    );

    // If the focused source cannot write, fanout must never reach mirrors.
    let mut observed = Vec::new();
    assert!(!source_first_synced_write(9961, &[9961, 9962], |id| {
        observed.push(id);
        false
    }));
    assert_eq!(observed, vec![9961]);

    write(&mut source, "exit\n");
    write(&mut mirror, "exit\n");
    wait_exit(&source_receiver, 9961);
    wait_exit(&mirror_receiver, 9962);
    drop(source);
    drop(mirror);
    wait_event_count(&events, "SESSION_CLOSED:", 2);
    println!(
        "PASS native real OpenSSH sync/disarm/failed-source isolation (Unix also verifies raw cursor bytes)"
    );
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

#[test]
#[ignore = "requires disposable AsyncSSH fixture: scripts/test-native-ssh-smoke.py"]
fn native_proxyjump_enforces_hop_trust_and_no_direct_fallback() {
    let fixture = fixture();
    let target_events = fixture.join("server.log");
    let jump_events = fixture.join("jump.log");

    let (mut connected, receiver) = open_jump(&fixture, 9901, "jump-config");
    wait_text(&mut connected, "NATIVE_SMOKE_READY");
    wait_event_count(&jump_events, "JUMP_FORWARD_ALLOW", 1);
    write(&mut connected, "echo:through-jump\n");
    wait_text(&mut connected, "NATIVE_ECHO:through-jump");
    write(&mut connected, "exit\n");
    wait_exit(&receiver, 9901);
    drop(connected);
    wait_event_count(&jump_events, "CONNECTION_CLOSED:", 1);

    let accepted = event_count(&target_events, "AUTH_ACCEPT:");

    // Target's trusted host key is intentionally wrong. The jump itself
    // succeeds, but target user authentication must never be attempted.
    let (mut rejected, rejected_events) = open_jump(&fixture, 9902, "jump-changed-config");
    wait_exit(&rejected_events, 9902);
    let message = grid(&mut rejected).to_ascii_lowercase();
    assert!(
        message.contains("host identification has changed")
            || message.contains("host key verification failed")
            || message.contains("@@@@@@@@@@"),
        "changed destination key did not cause a trust failure: {message}"
    );
    drop(rejected);
    assert_eq!(
        event_count(&target_events, "AUTH_ACCEPT:"),
        accepted,
        "changed key reached authentication via ProxyJump"
    );

    // The destination trust entry is valid, but the hop's stored host key
    // is deliberately wrong. OpenSSH must reject before authenticating to
    // the jump server, and no direct connection to the target is permitted.
    let hop_auth_before = event_count(&jump_events, "AUTH_ACCEPT:");
    let (mut bad_hop, bad_hop_events) = open_jump(&fixture, 9904, "jump-wrong-hop-config");
    wait_exit(&bad_hop_events, 9904);
    let message = grid(&mut bad_hop).to_ascii_lowercase();
    assert!(
        message.contains("host identification has changed")
            || message.contains("host key verification failed")
            || message.contains("@@@@@@@@@@"),
        "changed jump host key did not cause a trust failure: {message}"
    );
    drop(bad_hop);
    assert_eq!(
        event_count(&jump_events, "AUTH_ACCEPT:"),
        hop_auth_before,
        "changed jump key reached jump-host authentication"
    );
    assert_eq!(
        event_count(&target_events, "AUTH_ACCEPT:"),
        accepted,
        "changed jump key unexpectedly reached target authentication"
    );

    // The target is still listening, but the specified hop points to a
    // closed port. A direct fallback would authenticate anyway.
    let (mut broken, broken_events) = open_jump(&fixture, 9903, "jump-broken-config");
    wait_exit(&broken_events, 9903);
    assert!(
        !grid(&mut broken).contains("NATIVE_SMOKE_READY"),
        "configured ProxyJump failure silently connected directly"
    );
    drop(broken);
    assert_eq!(
        event_count(&target_events, "AUTH_ACCEPT:"),
        accepted,
        "failed jump reached the direct target"
    );
    println!("PASS native ProxyJump, target host trust and no-direct-fallback");
}

// macOS arm64 and Linux x64 execute this against a real locally spawned
// ssh-agent and a disposable AsyncSSH server with forwarding enabled.
// Windows named-pipe agent forwarding requires separate native evidence.
#[cfg(unix)]
#[test]
#[ignore = "requires native Unix agent fixture: scripts/test-native-ssh-smoke.py"]
fn native_forwarded_agent_identity_is_only_available_when_enabled() {
    let fixture = fixture();
    let events = fixture.join("server.log");
    // Distinct native OpenSSH sessions test both negative consent and a
    // fresh re-enable after teardown. An earlier allowed agent must not
    // leak into the disabled session, and re-enabling must still work.
    for (id, enabled, expected) in [
        (9971, true, "NATIVE_AGENT_FORWARDED"),
        (9972, false, "NATIVE_AGENT_DISABLED"),
        (9973, true, "NATIVE_AGENT_FORWARDED"),
    ] {
        let session = Session {
            name: "Disposable agent forwarding".into(),
            host: "native-smoke".into(),
            strict: true,
            ssh: SshOptions {
                agent_forwarding: Some(enabled),
                ..SshOptions::default()
            },
            ..Session::default()
        };
        let (sender, receiver) = mpsc::channel();
        let mut terminal = connect(id, sender, &session, Some(&fixture.join("agent-config")))
            .expect("native agent fixture terminal must start");
        wait_text(&mut terminal, "NATIVE_SMOKE_READY");
        write(&mut terminal, "agent-probe\n");
        wait_text(&mut terminal, expected);
        let visible = grid(&mut terminal);
        if enabled {
            assert!(!visible.contains("NATIVE_AGENT_DISABLED"));
        } else {
            assert!(!visible.contains("NATIVE_AGENT_FORWARDED"));
        }
        write(&mut terminal, "exit\n");
        wait_exit(&receiver, id);
        drop(terminal);
    }
    assert_eq!(
        event_count(&events, "AGENT_FORWARDED"),
        2,
        "fresh reconnect must regain forwarding only when explicitly enabled"
    );
    assert_eq!(
        event_count(&events, "AGENT_DISABLED"),
        1,
        "disabled session must not retain an earlier forwarded agent"
    );
    assert_eq!(
        event_count(&events, "SESSION_OPEN"),
        event_count(&events, "SESSION_CLOSED:"),
        "agent fixture left an SSH session open"
    );
    println!(
        "PASS native Unix SSH agent forwarding enable/disable/re-enable, identity listing and cleanup"
    );
}

fn fixture_port(fixture: &Path, name: &str) -> u16 {
    fs::read_to_string(fixture.join(name))
        .unwrap()
        .trim()
        .parse()
        .expect("fixture port must be numeric")
}

fn ephemeral_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn connect_loopback(port: u16) -> TcpStream {
    let address = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(9);
    loop {
        if let Ok(stream) = TcpStream::connect(&address) {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            return stream;
        }
        assert!(
            Instant::now() < deadline,
            "loopback SSH listener {port} did not appear"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn round_trip(stream: &mut TcpStream, message: &[u8]) {
    stream.write_all(message).unwrap();
    let mut response = vec![0u8; message.len()];
    stream.read_exact(&mut response).unwrap();
    assert_eq!(response, message, "forwarded loopback echo changed");
}

// An absent loopback listener must produce an actual refusal, not merely a
// timeout or a reset. Probe with the same blocking connect path as the echo
// check, rather than connect_timeout's separate nonblocking socket path.
fn wait_loopback_refused(port: u16, context: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = TcpStream::connect(("127.0.0.1", port));
        if matches!(&result, Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{context} did not refuse connections on {port}; last probe: {result:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn closed_loopback_listener_reports_connection_refused() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    wait_loopback_refused(port, "closed fixture listener");
}

/// Exercise the same saved-profile store and tunnel manager used by Iced.
#[test]
#[ignore = "requires disposable AsyncSSH fixture: scripts/test-native-ssh-smoke.py"]
fn saved_profile_tunnel_manager_releases_and_reuses_local_port() {
    let fixture = fixture();
    let local_port = ephemeral_port();
    let echo_port = fixture_port(&fixture, "echo-port");
    let path = fixture.join(format!("manager-profiles-{}.json", std::process::id()));
    let profile = Session {
        name: "Disposable manager profile".into(),
        host: "native-smoke".into(),
        strict: true,
        ssh: SshOptions {
            local_forwards: vec![format!("127.0.0.1:{local_port}:127.0.0.1:{echo_port}")],
            ..SshOptions::default()
        },
        ..Session::default()
    };
    save_sessions(&path, &[profile]).expect("save profile using Iced profile store");
    let loaded = load_sessions(&path).expect("load profile using Iced profile store");
    assert_eq!(loaded.len(), 1);
    let loaded = &loaded[0];
    let key = session_profile_key(loaded);
    let mut manager = tunnels::Manager::default();
    // On panic, Manager drops TunnelProcess, which kills and reaps the child.
    for attempt in 1..=2 {
        let status = manager
            .start(loaded, Some(&fixture.join("config")))
            .unwrap();
        assert!(status.is_running(), "manager start {attempt}: {status:?}");
        assert!(manager.status(&key).is_running());
        round_trip(
            &mut connect_loopback(local_port),
            b"MANAGER_SAVED_PROFILE_ECHO",
        );
        assert_eq!(manager.stop(&key).unwrap(), tunnels::Status::Stopped);
        assert_eq!(manager.status(&key), tunnels::Status::Stopped);
        wait_loopback_refused(local_port, &format!("manager stop {attempt}"));
    }
    println!(
        "PASS saved-profile Tunnel Manager echo, stop/refusal, same-port restart and stop/refusal"
    );
}

fn assert_forward_echo(port: u16, message: &[u8]) {
    round_trip(&mut connect_loopback(port), message);
}

/// Two saved `-L` rows of one reloaded profile run as independent OpenSSH
/// processes: stopping either leaves the other listener echoing.
#[test]
#[ignore = "requires disposable AsyncSSH fixture: scripts/test-native-ssh-smoke.py"]
fn saved_profile_individual_forwards_start_and_stop_independently() {
    let fixture = fixture();
    let config = fixture.join("config");
    let echo_port = fixture_port(&fixture, "echo-port");
    let port_a = ephemeral_port();
    let mut port_b = ephemeral_port();
    while port_b == port_a {
        port_b = ephemeral_port();
    }
    let spec_a = format!("127.0.0.1:{port_a}:127.0.0.1:{echo_port}");
    let spec_b = format!("127.0.0.1:{port_b}:127.0.0.1:{echo_port}");
    let path = fixture.join(format!(
        "manager-forward-profiles-{}.json",
        std::process::id()
    ));
    let profile = Session {
        name: "Disposable forward profile".into(),
        host: "native-smoke".into(),
        strict: true,
        ssh: SshOptions {
            local_forwards: vec![spec_a.clone(), spec_b.clone()],
            ..SshOptions::default()
        },
        ..Session::default()
    };
    // Same name in another folder: a distinct profile key, never started.
    let namesake = Session {
        folder: "Other folder".into(),
        ..profile.clone()
    };
    save_sessions(&path, &[profile, namesake]).expect("save profiles using Iced profile store");
    let loaded = load_sessions(&path).expect("load profiles using Iced profile store");
    assert_eq!(loaded.len(), 2);
    let (loaded, namesake) = (&loaded[0], &loaded[1]);
    assert!(loaded.folder.is_empty());
    assert_eq!(namesake.folder, "Other folder");
    assert_eq!(namesake.name, loaded.name);
    assert_eq!(
        loaded.ssh.local_forwards,
        vec![spec_a.clone(), spec_b.clone()]
    );
    let key = session_profile_key(loaded);
    let other_key = session_profile_key(namesake);
    assert_ne!(key, other_key, "same-name profiles must not share a key");
    let local = tunnels::ForwardKind::Local;
    // On panic, Manager drops every TunnelProcess, which kills and reaps it.
    let mut manager = tunnels::Manager::default();

    let a = manager
        .start_forward(loaded, local, &spec_a, Some(&config))
        .unwrap();
    assert!(a.is_running(), "start A: {a:?}");
    assert_eq!(
        manager.forward_status(&key, local, &spec_b),
        tunnels::Status::Stopped,
        "starting A must not start B"
    );
    let b = manager
        .start_forward(loaded, local, &spec_b, Some(&config))
        .unwrap();
    assert!(b.is_running(), "start B: {b:?}");
    assert_ne!(a, b, "A and B must be separate ssh processes");
    assert_eq!(manager.forward_status(&key, local, &spec_a), a);
    assert_eq!(manager.forward_status(&key, local, &spec_b), b);
    assert_eq!(manager.status(&key), tunnels::Status::Stopped);
    assert_eq!(manager.running_count(), 2);
    assert_forward_echo(port_a, b"FORWARD_A_ECHO");
    assert_forward_echo(port_b, b"FORWARD_B_ECHO");

    // The aggregate profile process is refused and kills nothing.
    let error = manager
        .start(loaded, Some(&config))
        .expect_err("aggregate start must be rejected while a forward runs");
    assert!(
        format!("{error:#}").contains("stop individual forwards"),
        "unexpected aggregate rejection: {error:#}"
    );
    assert_eq!(manager.status(&key), tunnels::Status::Stopped);
    assert_eq!(manager.forward_status(&key, local, &spec_a), a);
    assert_eq!(manager.forward_status(&key, local, &spec_b), b);
    assert_forward_echo(port_a, b"FORWARD_A_AFTER_AGGREGATE_REJECT");
    assert_forward_echo(port_b, b"FORWARD_B_AFTER_AGGREGATE_REJECT");

    // The namesake profile owns neither process; stopping it is a no-op.
    assert_eq!(
        manager.forward_status(&other_key, local, &spec_a),
        tunnels::Status::Stopped
    );
    assert_eq!(
        manager.stop_forward(&other_key, local, &spec_a).unwrap(),
        tunnels::Status::Stopped
    );
    assert_eq!(manager.stop(&other_key).unwrap(), tunnels::Status::Stopped);
    assert_eq!(manager.forward_status(&key, local, &spec_a), a);
    assert_eq!(manager.forward_status(&key, local, &spec_b), b);
    assert_forward_echo(port_a, b"FORWARD_A_AFTER_NAMESAKE_STOP");
    assert_forward_echo(port_b, b"FORWARD_B_AFTER_NAMESAKE_STOP");

    // Stop A: A refuses, B keeps echoing.
    assert_eq!(
        manager.stop_forward(&key, local, &spec_a).unwrap(),
        tunnels::Status::Stopped
    );
    assert_eq!(
        manager.forward_status(&key, local, &spec_a),
        tunnels::Status::Stopped
    );
    wait_loopback_refused(port_a, "stop forward A");
    assert_eq!(manager.forward_status(&key, local, &spec_b), b);
    assert_forward_echo(port_b, b"FORWARD_B_AFTER_A_STOP");

    // Restart A on the same port while B runs.
    let a = manager
        .start_forward(loaded, local, &spec_a, Some(&config))
        .unwrap();
    assert!(a.is_running(), "restart A: {a:?}");
    assert_ne!(a, b);
    assert_eq!(manager.running_count(), 2);
    assert_forward_echo(port_a, b"FORWARD_A_AFTER_RESTART");
    assert_forward_echo(port_b, b"FORWARD_B_AFTER_A_RESTART");

    // Stop B: B refuses, A keeps echoing.
    assert_eq!(
        manager.stop_forward(&key, local, &spec_b).unwrap(),
        tunnels::Status::Stopped
    );
    assert_eq!(
        manager.forward_status(&key, local, &spec_b),
        tunnels::Status::Stopped
    );
    wait_loopback_refused(port_b, "stop forward B");
    assert_eq!(manager.forward_status(&key, local, &spec_a), a);
    assert_forward_echo(port_a, b"FORWARD_A_AFTER_B_STOP");

    // Stop A: nothing is left listening.
    assert_eq!(
        manager.stop_forward(&key, local, &spec_a).unwrap(),
        tunnels::Status::Stopped
    );
    assert_eq!(
        manager.forward_status(&key, local, &spec_a),
        tunnels::Status::Stopped
    );
    wait_loopback_refused(port_a, "final stop forward A");
    wait_loopback_refused(port_b, "B after final stop of A");
    assert_eq!(manager.running_count(), 0);
    println!(
        "PASS saved-profile individual forwards: independent start/stop, refusal, same-port restart, aggregate rejection and namesake isolation"
    );
}

#[test]
#[ignore = "requires disposable AsyncSSH fixture: scripts/test-native-ssh-smoke.py"]
fn native_local_remote_and_dynamic_ssh_forwarding() {
    let fixture = fixture();
    let echo_port = fixture_port(&fixture, "echo-port");
    let remote_port = fixture_port(&fixture, "remote-port");
    let mut local_port = ephemeral_port();
    while local_port == remote_port {
        local_port = ephemeral_port();
    }
    let mut socks_port = ephemeral_port();
    while socks_port == local_port || socks_port == remote_port {
        socks_port = ephemeral_port();
    }

    // The destination for the reverse forward runs in this test, not in a
    // production service or on an externally reachable interface.
    let service = TcpListener::bind("127.0.0.1:0").unwrap();
    let service_port = service.local_addr().unwrap().port();
    service.set_nonblocking(true).unwrap();
    let responder = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(9);
        loop {
            match service.accept() {
                Ok((mut stream, _)) => {
                    // macOS can inherit O_NONBLOCK from the polling listener.
                    // The accepted stream needs blocking I/O for read_exact.
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut payload = [0; 12];
                    stream.read_exact(&mut payload).unwrap();
                    stream.write_all(&payload).unwrap();
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "reverse-forward service never used"
                    );
                    thread::sleep(Duration::from_millis(40));
                }
                Err(error) => panic!("reverse-forward accept failed: {error}"),
            }
        }
    });

    let session = Session {
        name: "Disposable forwarding".into(),
        host: "native-smoke".into(),
        strict: true,
        ssh: SshOptions {
            local_forwards: vec![format!("127.0.0.1:{local_port}:127.0.0.1:{echo_port}")],
            remote_forwards: vec![format!("127.0.0.1:{remote_port}:127.0.0.1:{service_port}")],
            dynamic_forwards: vec![format!("127.0.0.1:{socks_port}")],
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let (sender, receiver) = mpsc::channel();
    let mut terminal = connect(9960, sender, &session, Some(&fixture.join("config")))
        .expect("real Inspirum OpenSSH forwarding terminal must start");
    wait_text(&mut terminal, "NATIVE_SMOKE_READY");

    // OpenSSH local -L TCP relay through the disposable SSH server.
    round_trip(&mut connect_loopback(local_port), b"LOCAL_FORWARD");
    wait_event_count(&fixture.join("server.log"), "LOCAL_FORWARD_ALLOW", 1);

    // SOCKS5 -D relay, including the actual connect handshake and TCP data.
    let mut socks = connect_loopback(socks_port);
    socks.write_all(&[5, 1, 0]).unwrap();
    let mut greeting = [0; 2];
    socks.read_exact(&mut greeting).unwrap();
    assert_eq!(
        greeting,
        [5, 0],
        "SOCKS authentication was unexpectedly required"
    );
    let [hi, lo] = echo_port.to_be_bytes();
    socks
        .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, hi, lo])
        .unwrap();
    let mut reply = [0; 10];
    socks.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[0..2], &[5, 0], "SOCKS connect request failed");
    round_trip(&mut socks, b"SOCKS_FORWARD");

    // Remote -R listener is server-owned, forwarding back into a local
    // disposable echo service. The server logs the accepted request.
    wait_event_count(&fixture.join("server.log"), "REMOTE_FORWARD_ALLOW", 1);
    round_trip(&mut connect_loopback(remote_port), b"REVERSE_ECHO");
    responder.join().unwrap();

    // OpenSSH may keep its transport alive while an active SOCKS channel
    // exists, even after the remote shell exits. Close the last test channel
    // first so this check tests actual terminal shutdown, not a live tunnel.
    drop(socks);
    write(&mut terminal, "exit\n");
    wait_exit(&receiver, 9960);
    drop(terminal);
    // Forward listeners must go away when the SSH session closes.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let local_closed = TcpStream::connect(("127.0.0.1", local_port)).is_err();
        let socks_closed = TcpStream::connect(("127.0.0.1", socks_port)).is_err();
        let remote_closed = TcpStream::connect(("127.0.0.1", remote_port)).is_err();
        if local_closed && socks_closed && remote_closed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "SSH forwarding listener survived session exit"
        );
        thread::sleep(Duration::from_millis(50));
    }
    // Reconnect immediately with the very same -L/-R/-D specifications.
    // A stale OpenSSH listener from the first session must not prevent
    // ExitOnForwardFailure from binding the second session's ports.
    let remote_requests = event_count(&fixture.join("server.log"), "REMOTE_FORWARD_ALLOW");
    let (reconnect_sender, reconnect_receiver) = mpsc::channel();
    let mut reconnected = connect(
        9961,
        reconnect_sender,
        &session,
        Some(&fixture.join("config")),
    )
    .expect("reconnect with the same local, remote and SOCKS listeners");
    wait_text(&mut reconnected, "NATIVE_SMOKE_READY");
    round_trip(&mut connect_loopback(local_port), b"RECONNECT_LOCAL");
    wait_event_count(
        &fixture.join("server.log"),
        "REMOTE_FORWARD_ALLOW",
        remote_requests + 1,
    );

    let mut socks_reconnected = connect_loopback(socks_port);
    socks_reconnected.write_all(&[5, 1, 0]).unwrap();
    let mut greeting = [0; 2];
    socks_reconnected.read_exact(&mut greeting).unwrap();
    assert_eq!(greeting, [5, 0], "reconnected SOCKS greeting failed");
    let [hi, lo] = echo_port.to_be_bytes();
    socks_reconnected
        .write_all(&[5, 1, 0, 1, 127, 0, 0, 1, hi, lo])
        .unwrap();
    let mut reply = [0; 10];
    socks_reconnected.read_exact(&mut reply).unwrap();
    assert_eq!(&reply[0..2], &[5, 0], "reconnected SOCKS routing failed");
    round_trip(&mut socks_reconnected, b"RECONNECT_SOCKS");

    // Release the forwarded TCP channel before asking the PTY to exit.
    drop(socks_reconnected);
    write(&mut reconnected, "exit\n");
    wait_exit(&reconnect_receiver, 9961);
    drop(reconnected);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if [local_port, socks_port, remote_port]
            .into_iter()
            .all(|port| TcpStream::connect(("127.0.0.1", port)).is_err())
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "reconnected SSH forwarding listener survived second session exit"
        );
        thread::sleep(Duration::from_millis(50));
    }
    println!(
        "PASS native SSH -L, -R, -D forwarding, reconnect with same ports, and listener cleanup"
    );
}
