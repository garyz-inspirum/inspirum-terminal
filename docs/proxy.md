# Structured SSH proxy transport

Inspirum supports two structured proxy types in saved SSH profiles:

- HTTP CONNECT (no proxy authentication)
- SOCKS5 CONNECT with the no-auth method

ProxyJump remains a separate OpenSSH feature. A profile cannot enable ProxyJump and a structured HTTP/SOCKS proxy at the same time.

## Security model

The UI does not accept a raw ProxyCommand shell string. Instead it stores only a proxy type, host and port. The proxy endpoint is validated as a hostname or IPv4/IPv6 address and a numeric port. Inspirum resolves the effective SSH destination with `ssh -G`, validates that resolved host/port, and then supplies OpenSSH a ProxyCommand that invokes the same Inspirum executable in a hidden `--proxy-helper` mode.

The helper implements the proxy handshake itself and relays bytes over standard input/output. It does not read profiles, SSH configuration, private keys, agents, passwords, environment variables or terminal contents.

If the proxy cannot be reached, denies the tunnel or returns an invalid protocol response, the helper exits non-zero. OpenSSH treats that as ProxyCommand failure. There is no application path that retries the target directly.

HTTP/SOCKS proxy credentials are intentionally not stored or supported in this increment. Raw arbitrary ProxyCommand editing is also not exposed.

## HTTP CONNECT

The helper opens a TCP connection to the configured proxy, sends a bounded HTTP/1.1 CONNECT request for the effective SSH destination and requires a 200 response. Response headers are limited to 16 KiB and the handshake uses bounded socket timeouts.

## SOCKS5

The helper negotiates SOCKS5 with the no-authentication method and issues CONNECT for the effective destination. IPv4, IPv6 and domain-name destination address types are supported.

## SSH and SFTP

The same structured proxy setting applies to normal SSH terminals and interactive SFTP tabs. Authentication, host-key policy and the rest of the SSH profile remain owned by OpenSSH.

## Verification

Portable tests exercise:

- HTTP CONNECT success and relay
- SOCKS5 success and relay
- proxy denial as a hard error
- endpoint validation and ProxyJump mutual exclusion
- fixed ProxyCommand construction without user-entered shell fragments

The Linux disposable SSH fixture additionally starts a real HTTP CONNECT proxy. One ignored integration test authenticates to the disposable SSH server through that proxy. A second keeps the SSH target directly reachable while the proxy returns HTTP 403 and verifies that the terminal exits without authenticating directly.

Windows x64 and Apple Silicon CI compile and run the portable test suite. Real authenticated proxy-server acceptance on those platforms remains a separate gate.

Tracked by #35 as a bounded increment of #14.
