use inspirum_terminal::{
    Session, SshOptions, forward_requires_risk_ack, session_requires_forward_risk_ack,
    terminal::tunnel_launch_args,
};

fn session() -> Session {
    Session {
        name: "tunnels".into(),
        host: "example.test".into(),
        ssh: SshOptions {
            local_forwards: vec!["127.0.0.1:8080:internal:80".into()],
            remote_forwards: vec!["9000:127.0.0.1:3000".into()],
            dynamic_forwards: vec!["127.0.0.1:1080".into()],
            ..SshOptions::default()
        },
        ..Session::default()
    }
}

#[test]
fn tunnel_launch_is_forwarding_only_and_keeps_structured_argv() {
    let args = tunnel_launch_args(&session(), None).unwrap();
    assert!(args.iter().any(|arg| arg == "-N"));
    assert!(args.iter().any(|arg| arg == "-T"));
    assert!(!args.iter().any(|arg| arg == "-tt"));
    assert!(args.windows(2).any(|w| w == ["-L", "127.0.0.1:8080:internal:80"]));
    assert!(args.windows(2).any(|w| w == ["-R", "9000:127.0.0.1:3000"]));
    assert!(args.windows(2).any(|w| w == ["-D", "127.0.0.1:1080"]));
}

#[test]
fn exposure_requires_ack_but_loopback_and_implicit_defaults_do_not() {
    for safe in ["127.0.0.1:8080:host:80", "localhost:8080:host:80", "[::1]:8080:host:80", "8080:host:80"] {
        assert!(!forward_requires_risk_ack(safe, false), "{safe}");
    }
    for exposed in ["0.0.0.0:8080:host:80", "*:8080:host:80", "192.0.2.10:8080:host:80"] {
        assert!(forward_requires_risk_ack(exposed, false), "{exposed}");
    }
    assert!(!forward_requires_risk_ack("1080", true));
    assert!(!forward_requires_risk_ack("127.0.0.1:1080", true));
    assert!(forward_requires_risk_ack("0.0.0.0:1080", true));

    let mut s = session();
    assert!(!session_requires_forward_risk_ack(&s));
    s.ssh.local_forwards.push("0.0.0.0:8081:host:80".into());
    assert!(session_requires_forward_risk_ack(&s));
}

#[test]
fn tunnel_manager_requires_at_least_one_forward_and_no_remote_command() {
    let mut s = session();
    s.ssh.local_forwards.clear();
    s.ssh.remote_forwards.clear();
    s.ssh.dynamic_forwards.clear();
    assert!(tunnel_launch_args(&s, None).is_err());

    let mut s = session();
    s.ssh.remote_command = "echo no".into();
    assert!(tunnel_launch_args(&s, None).is_err());
}
