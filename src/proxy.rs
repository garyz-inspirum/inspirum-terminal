//! Built-in no-auth HTTP CONNECT and SOCKS5 proxy transport for OpenSSH ProxyCommand.
use crate::{ProxyAuth, ProxyKind};
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{self, Read, Write},
    net::{IpAddr, Shutdown, TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HTTP_HEADER: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyCredentials {
    username: String,
    password: String,
}

impl ProxyCredentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Result<Self> {
        let username = username.into();
        let password = password.into();
        ensure!(!username.is_empty(), "proxy username is required");
        ensure!(
            username.len() <= 255 && password.len() <= 255,
            "proxy username/password must each be at most 255 UTF-8 bytes"
        );
        ensure!(
            !username.chars().any(char::is_control) && !password.chars().any(char::is_control),
            "proxy username/password may not contain control characters"
        );
        Ok(Self { username, password })
    }
}

pub fn credentials_from_environment(auth: ProxyAuth) -> Result<Option<ProxyCredentials>> {
    match auth {
        ProxyAuth::None => Ok(None),
        ProxyAuth::Environment => {
            let username = std::env::var("INSPIRUM_PROXY_USERNAME")
                .context("INSPIRUM_PROXY_USERNAME is required for environment proxy authentication")?;
            let password = std::env::var("INSPIRUM_PROXY_PASSWORD")
                .context("INSPIRUM_PROXY_PASSWORD is required for environment proxy authentication")?;
            ProxyCredentials::new(username, password).map(Some)
        }
    }
}

fn base64_basic(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let a = chunk[0];
        let b = *chunk.get(1).unwrap_or(&0);
        let c = *chunk.get(2).unwrap_or(&0);
        output.push(TABLE[(a >> 2) as usize] as char);
        output.push(TABLE[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(c & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn connect_tcp(host: &str, port: u16) -> Result<TcpStream> {
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolve proxy endpoint {host}:{port}"))?
        .collect();
    ensure!(
        !addresses.is_empty(),
        "proxy endpoint resolved to no addresses"
    );
    let mut last = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap()).context("connect to proxy")
}

fn authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn read_exact_before(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
    label: &'static str,
) -> Result<()> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .context("proxy handshake timed out")?;
    stream.set_read_timeout(Some(remaining))?;
    stream.read_exact(buffer).with_context(|| label)
}

fn http_connect(
    mut stream: TcpStream,
    target_host: &str,
    target_port: u16,
    deadline: Instant,
    credentials: Option<&ProxyCredentials>,
) -> Result<TcpStream> {
    let destination = authority(target_host, target_port);
    let authorization = credentials
        .map(|credentials| {
            let token = base64_basic(
                format!("{}:{}", credentials.username, credentials.password).as_bytes(),
            );
            format!("Proxy-Authorization: Basic {token}\r\n")
        })
        .unwrap_or_default();
    write!(
        stream,
        "CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\nProxy-Connection: Keep-Alive\r\n{authorization}\r\n"
    )
    .context("write HTTP CONNECT request")?;
    stream.flush().context("flush HTTP CONNECT request")?;

    let mut header = Vec::with_capacity(512);
    let mut one = [0_u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        ensure!(
            header.len() < MAX_HTTP_HEADER,
            "HTTP proxy response headers exceeded 16 KiB"
        );
        read_exact_before(
            &mut stream,
            &mut one,
            deadline,
            "read HTTP CONNECT response",
        )?;
        header.push(one[0]);
    }
    let text = std::str::from_utf8(&header).context("HTTP proxy response was not UTF-8")?;
    let status = text
        .lines()
        .next()
        .context("HTTP proxy response had no status line")?;
    let mut fields = status.split_whitespace();
    let protocol = fields.next().unwrap_or_default();
    let code = fields.next().unwrap_or_default();
    ensure!(
        protocol.starts_with("HTTP/") && code == "200",
        "HTTP CONNECT proxy denied the tunnel"
    );
    Ok(stream)
}

fn socks5_connect(
    mut stream: TcpStream,
    target_host: &str,
    target_port: u16,
    deadline: Instant,
    credentials: Option<&ProxyCredentials>,
) -> Result<TcpStream> {
    let requested_method = if credentials.is_some() { 0x02 } else { 0x00 };
    stream
        .write_all(&[0x05, 0x01, requested_method])
        .context("write SOCKS5 greeting")?;
    let mut greeting = [0_u8; 2];
    read_exact_before(&mut stream, &mut greeting, deadline, "read SOCKS5 greeting")?;
    ensure!(
        greeting == [0x05, requested_method],
        if credentials.is_some() {
            "SOCKS5 proxy did not accept username/password authentication"
        } else {
            "SOCKS5 proxy does not allow unauthenticated tunnelling"
        }
    );
    if let Some(credentials) = credentials {
        let username = credentials.username.as_bytes();
        let password = credentials.password.as_bytes();
        let mut auth = Vec::with_capacity(username.len() + password.len() + 3);
        auth.extend([0x01, username.len() as u8]);
        auth.extend_from_slice(username);
        auth.push(password.len() as u8);
        auth.extend_from_slice(password);
        stream
            .write_all(&auth)
            .context("write SOCKS5 username/password authentication")?;
        let mut response = [0_u8; 2];
        read_exact_before(
            &mut stream,
            &mut response,
            deadline,
            "read SOCKS5 username/password response",
        )?;
        ensure!(
            response == [0x01, 0x00],
            "SOCKS5 proxy rejected username/password authentication"
        );
    }

    let mut request = vec![0x05, 0x01, 0x00];
    match target_host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => {
            request.push(0x01);
            request.extend_from_slice(&address.octets());
        }
        Ok(IpAddr::V6(address)) => {
            request.push(0x04);
            request.extend_from_slice(&address.octets());
        }
        Err(_) => {
            let bytes = target_host.as_bytes();
            ensure!(
                !bytes.is_empty() && bytes.len() <= 255,
                "SOCKS5 target hostname must be 1-255 bytes"
            );
            request.push(0x03);
            request.push(bytes.len() as u8);
            request.extend_from_slice(bytes);
        }
    }
    request.extend_from_slice(&target_port.to_be_bytes());
    stream
        .write_all(&request)
        .context("write SOCKS5 CONNECT request")?;

    let mut response = [0_u8; 4];
    read_exact_before(
        &mut stream,
        &mut response,
        deadline,
        "read SOCKS5 CONNECT response",
    )?;
    ensure!(
        response[0] == 0x05 && response[1] == 0x00,
        "SOCKS5 proxy denied the tunnel"
    );
    let address_len = match response[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut length = [0_u8; 1];
            read_exact_before(
                &mut stream,
                &mut length,
                deadline,
                "read SOCKS5 address length",
            )?;
            usize::from(length[0])
        }
        _ => bail!("SOCKS5 proxy returned an invalid address type"),
    };
    let mut ignored = vec![0_u8; address_len + 2];
    read_exact_before(
        &mut stream,
        &mut ignored,
        deadline,
        "read SOCKS5 bound address",
    )?;
    Ok(stream)
}

/// Establish a proxy tunnel. Credentials are intentionally unsupported in this increment.
pub fn connect_tunnel(
    kind: ProxyKind,
    proxy_host: &str,
    proxy_port: u16,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream> {
    connect_tunnel_with_credentials(
        kind,
        proxy_host,
        proxy_port,
        target_host,
        target_port,
        None,
    )
}

pub fn connect_tunnel_with_credentials(
    kind: ProxyKind,
    proxy_host: &str,
    proxy_port: u16,
    target_host: &str,
    target_port: u16,
    credentials: Option<&ProxyCredentials>,
) -> Result<TcpStream> {
    ensure!(proxy_port > 0 && target_port > 0, "ports must be non-zero");
    let stream = connect_tcp(proxy_host, proxy_port)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let stream = match kind {
        ProxyKind::HttpConnect => {
            http_connect(stream, target_host, target_port, deadline, credentials)?
        }
        ProxyKind::Socks5 => {
            socks5_connect(stream, target_host, target_port, deadline, credentials)?
        }
        ProxyKind::None => bail!("proxy helper requires HTTP CONNECT or SOCKS5 mode"),
    };
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(stream)
}

/// Relay standard input/output over the established proxy tunnel for OpenSSH ProxyCommand.
pub fn run_stdio(
    kind: ProxyKind,
    auth: ProxyAuth,
    proxy_host: &str,
    proxy_port: u16,
    target_host: &str,
    target_port: u16,
) -> Result<()> {
    let credentials = credentials_from_environment(auth)?;
    let stream = connect_tunnel_with_credentials(
        kind,
        proxy_host,
        proxy_port,
        target_host,
        target_port,
        credentials.as_ref(),
    )?;
    let mut upstream = stream.try_clone().context("clone proxy tunnel")?;
    let mut downstream = stream;

    let writer = std::thread::spawn(move || -> io::Result<()> {
        let mut input = io::stdin().lock();
        io::copy(&mut input, &mut upstream)?;
        upstream.shutdown(Shutdown::Write)?;
        Ok(())
    });

    let mut output = io::stdout().lock();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = downstream
            .read(&mut buffer)
            .context("read proxy response for OpenSSH")?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .context("relay proxy response to OpenSSH")?;
        // ProxyCommand stdout is a protocol transport, not terminal output. Flush every
        // received chunk so interactive handshakes cannot deadlock behind stdio buffering.
        output.flush().context("flush proxy response to OpenSSH")?;
    }
    if writer.is_finished() {
        match writer.join() {
            Ok(result) => result.context("relay OpenSSH input to proxy")?,
            Err(_) => bail!("proxy input relay thread panicked"),
        }
    }
    Ok(())
}
