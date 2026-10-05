use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener},
    process::{Command, Stdio},
    thread,
};

fn proxy_once(deny: bool) -> (u16, thread::JoinHandle<()>) {
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
        if deny {
            stream
                .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            return;
        }
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .unwrap();
        let mut payload = Vec::new();
        stream.read_to_end(&mut payload).unwrap();
        stream.write_all(&payload).unwrap();
    });
    (port, handle)
}

#[test]
fn headless_proxy_helper_cli_relays_without_graphical_initialization() {
    let (port, proxy) = proxy_once(false);
    let mut child = Command::new(env!("CARGO_BIN_EXE_inspirum-terminal"))
        .args([
            "--proxy-helper",
            "--mode",
            "http-connect",
            "--proxy-host",
            "127.0.0.1",
            "--proxy-port",
            &port.to_string(),
            "--target-host",
            "target.example",
            "--target-port",
            "22",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"proxy-cli-roundtrip")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"proxy-cli-roundtrip");
    proxy.join().unwrap();
}

#[test]
fn headless_proxy_helper_cli_returns_failure_on_proxy_denial() {
    let (port, proxy) = proxy_once(true);
    let output = Command::new(env!("CARGO_BIN_EXE_inspirum-terminal"))
        .args([
            "--proxy-helper",
            "--mode",
            "http-connect",
            "--proxy-host",
            "127.0.0.1",
            "--proxy-port",
            &port.to_string(),
            "--target-host",
            "target.example",
            "--target-port",
            "22",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    proxy.join().unwrap();
}
