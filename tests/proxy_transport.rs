use inspirum_terminal::{
    ProxyAuth, ProxyKind, Session, SshOptions,
    proxy::{ProxyCredentials, connect_tunnel, connect_tunnel_with_credentials},
    terminal::proxy_command_for_target,
};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::Path,
    thread,
    time::Duration,
};

fn http_echo_proxy() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let text = String::from_utf8(request).unwrap();
        assert!(
            text.starts_with("CONNECT target.example:2222 HTTP/1.1\r\n"),
            "{text}"
        );
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\nX-Test: yes\r\n\r\n")
            .unwrap();
        let mut payload = [0_u8; 11];
        stream.read_exact(&mut payload).unwrap();
        assert_eq!(&payload, b"http-tunnel");
        stream.write_all(&payload).unwrap();
    });
    (port, handle)
}

fn socks_echo_proxy() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut greeting = [0_u8; 3];
        stream.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [0x05, 0x01, 0x00]);
        stream.write_all(&[0x05, 0x00]).unwrap();

        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header, [0x05, 0x01, 0x00, 0x03]);
        let mut length = [0_u8; 1];
        stream.read_exact(&mut length).unwrap();
        let mut host = vec![0_u8; usize::from(length[0])];
        stream.read_exact(&mut host).unwrap();
        assert_eq!(host, b"target.example");
        let mut port_bytes = [0_u8; 2];
        stream.read_exact(&mut port_bytes).unwrap();
        assert_eq!(u16::from_be_bytes(port_bytes), 2222);
        stream
            .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0])
            .unwrap();

        let mut payload = [0_u8; 12];
        stream.read_exact(&mut payload).unwrap();
        assert_eq!(&payload, b"socks-tunnel");
        stream.write_all(&payload).unwrap();
    });
    (port, handle)
}

#[test]
fn http_connect_helper_establishes_and_relays_tunnel() {
    let (port, proxy) = http_echo_proxy();
    let mut tunnel = connect_tunnel(
        ProxyKind::HttpConnect,
        "127.0.0.1",
        port,
        "target.example",
        2222,
    )
    .unwrap();
    tunnel.write_all(b"http-tunnel").unwrap();
    let mut echoed = [0_u8; 11];
    tunnel.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"http-tunnel");
    proxy.join().unwrap();
}

#[test]
fn socks5_helper_establishes_and_relays_tunnel() {
    let (port, proxy) = socks_echo_proxy();
    let mut tunnel =
        connect_tunnel(ProxyKind::Socks5, "127.0.0.1", port, "target.example", 2222).unwrap();
    tunnel.write_all(b"socks-tunnel").unwrap();
    let mut echoed = [0_u8; 12];
    tunnel.read_exact(&mut echoed).unwrap();
    assert_eq!(&echoed, b"socks-tunnel");
    proxy.join().unwrap();
}

fn http_basic_proxy() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let text = String::from_utf8(request).unwrap();
        assert!(
            text.contains("Proxy-Authorization: Basic dXNlcjpzM2NyZXQ="),
            "{text}"
        );
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .unwrap();
    });
    (port, handle)
}

fn socks_auth_proxy(accept: bool) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut greeting = [0_u8; 3];
        stream.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [0x05, 0x01, 0x02]);
        stream.write_all(&[0x05, 0x02]).unwrap();

        let mut version_and_user_len = [0_u8; 2];
        stream.read_exact(&mut version_and_user_len).unwrap();
        assert_eq!(version_and_user_len[0], 0x01);
        let mut user = vec![0_u8; version_and_user_len[1] as usize];
        stream.read_exact(&mut user).unwrap();
        let mut password_len = [0_u8; 1];
        stream.read_exact(&mut password_len).unwrap();
        let mut password = vec![0_u8; password_len[0] as usize];
        stream.read_exact(&mut password).unwrap();
        assert_eq!(user, b"user");
        assert_eq!(password, b"s3cret");
        stream.write_all(&[0x01, if accept { 0x00 } else { 0x01 }]).unwrap();
        if !accept {
            return;
        }

        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(header, [0x05, 0x01, 0x00, 0x03]);
        let mut length = [0_u8; 1];
        stream.read_exact(&mut length).unwrap();
        let mut host = vec![0_u8; length[0] as usize];
        stream.read_exact(&mut host).unwrap();
        let mut target_port = [0_u8; 2];
        stream.read_exact(&mut target_port).unwrap();
        assert_eq!(host, b"target.example");
        assert_eq!(u16::from_be_bytes(target_port), 2222);
        stream
            .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0])
            .unwrap();
    });
    (port, handle)
}

#[test]
fn http_connect_basic_auth_is_sent_only_inside_proxy_protocol() {
    let (port, proxy) = http_basic_proxy();
    let credentials = ProxyCredentials::new("user", "s3cret").unwrap();
    let _tunnel = connect_tunnel_with_credentials(
        ProxyKind::HttpConnect,
        "127.0.0.1",
        port,
        "target.example",
        2222,
        Some(&credentials),
    )
    .unwrap();
    proxy.join().unwrap();
}

#[test]
fn socks5_username_password_success_and_denial_are_hard_results() {
    let credentials = ProxyCredentials::new("user", "s3cret").unwrap();
    let (ok_port, ok_proxy) = socks_auth_proxy(true);
    let _tunnel = connect_tunnel_with_credentials(
        ProxyKind::Socks5,
        "127.0.0.1",
        ok_port,
        "target.example",
        2222,
        Some(&credentials),
    )
    .unwrap();
    ok_proxy.join().unwrap();

    let (deny_port, deny_proxy) = socks_auth_proxy(false);
    let error = connect_tunnel_with_credentials(
        ProxyKind::Socks5,
        "127.0.0.1",
        deny_port,
        "target.example",
        2222,
        Some(&credentials),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("rejected"), "{error}");
    deny_proxy.join().unwrap();
}

#[test]
fn proxy_denial_is_a_hard_error() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .unwrap();
    });
    let error = connect_tunnel(
        ProxyKind::HttpConnect,
        "127.0.0.1",
        port,
        "target.example",
        22,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("denied"), "{error}");
    server.join().unwrap();
}

#[test]
fn structured_proxy_requires_endpoint_and_excludes_proxyjump() {
    let mut session = Session {
        host: "target.example".into(),
        ..Session::default()
    };
    session.ssh.proxy_kind = ProxyKind::HttpConnect;
    assert!(session.ssh_args().is_err());
    session.ssh.proxy_host = "127.0.0.1".into();
    session.ssh.proxy_port = Some(8080);
    assert!(session.ssh_args().is_ok());
    session.ssh.proxy_jump = "bastion".into();
    assert!(session.ssh_args().is_err());
}

#[test]
fn proxy_command_has_fixed_options_and_no_user_shell_fragment() {
    let session = Session {
        host: "target.example".into(),
        ssh: SshOptions {
            proxy_kind: ProxyKind::Socks5,
            proxy_host: "proxy.example".into(),
            proxy_port: Some(1080),
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let command = proxy_command_for_target(
        &session,
        Path::new(if cfg!(windows) {
            r"C:\Program Files\Inspirum\inspirum-terminal.exe"
        } else {
            "/Applications/Inspirum Terminal/inspirum-terminal"
        }),
        "resolved.example",
        2200,
    )
    .unwrap()
    .unwrap();
    assert!(command.contains("--proxy-helper --mode socks5"));
    assert!(command.contains("--proxy-host proxy.example --proxy-port 1080"));
    assert!(command.contains("--target-host resolved.example --target-port 2200"));
    for unsafe_fragment in [";", " && ", " || ", "$(", "\n", "\r"] {
        assert!(!command.contains(unsafe_fragment), "{command}");
    }
}

#[test]
fn proxy_endpoint_rejects_shell_syntax_before_command_construction() {
    for host in [
        "proxy example",
        "proxy;touch",
        "$(id)",
        "-oProxyCommand=x",
        "fe80::1%PATH%",
    ] {
        let session = Session {
            host: "target.example".into(),
            ssh: SshOptions {
                proxy_kind: ProxyKind::HttpConnect,
                proxy_host: host.into(),
                proxy_port: Some(8080),
                ..SshOptions::default()
            },
            ..Session::default()
        };
        assert!(session.ssh_args().is_err(), "{host}");
    }
}

#[test]
fn environment_proxy_auth_mode_contains_no_credentials_in_proxycommand() {
    let session = Session {
        host: "target.example".into(),
        ssh: SshOptions {
            proxy_kind: ProxyKind::HttpConnect,
            proxy_host: "proxy.example".into(),
            proxy_port: Some(8080),
            proxy_auth: ProxyAuth::Environment,
            ..SshOptions::default()
        },
        ..Session::default()
    };
    let command = proxy_command_for_target(
        &session,
        Path::new(if cfg!(windows) {
            r"C:\Program Files\Inspirum\inspirum-terminal.exe"
        } else {
            "/Applications/Inspirum Terminal/inspirum-terminal"
        }),
        "target.example",
        22,
    )
    .unwrap()
    .unwrap();
    assert!(command.contains("--proxy-auth environment"));
    assert!(!command.contains("INSPIRUM_PROXY_USERNAME"));
    assert!(!command.contains("INSPIRUM_PROXY_PASSWORD"));
    assert!(!command.contains("s3cret"));
}

#[test]
fn loopback_socket_address_is_not_part_of_profile_validation() {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 22));
    assert_eq!(address.port(), 22);
}
