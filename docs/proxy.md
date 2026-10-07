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


## Authenticated structured proxies

Issue #63 adds optional authenticated HTTP CONNECT and SOCKS5 support without storing proxy credentials in session profiles.

A profile stores only the authentication source. **Environment credentials** tells the built-in proxy helper to read `INSPIRUM_PROXY_USERNAME` and `INSPIRUM_PROXY_PASSWORD` at helper runtime. The username and password are not written to the session JSON, are not embedded in `ProxyCommand`, and are omitted from support diagnostics.

HTTP CONNECT uses Basic proxy authentication when environment credentials are enabled. SOCKS5 offers username/password authentication (RFC 1929) and does not silently downgrade to unauthenticated SOCKS when credentials were requested. Authentication denial or tunnel denial is a hard connection failure; Inspirum never retries the target directly.

Environment variables are inherited process state rather than an OS credential vault. Their visibility and lifecycle depend on the operating system and how Inspirum Terminal is launched. For environments where process-environment credentials do not meet local security requirements, leave structured proxy authentication disabled and use an approved external OpenSSH configuration or enterprise credential mechanism.

## OpenSSH algorithm policy

Profiles may explicitly set **Ciphers**, **MACs**, **KEX algorithms**, and **Host-key algorithms** using OpenSSH-compatible comma-separated policy values. Blank fields mean **inherit** and add no override, preserving OpenSSH defaults and any SSH configuration policy.

Inspirum does not implement cryptographic algorithms and does not silently enable legacy algorithms. Explicit legacy requests such as `ssh-rsa`, `ssh-dss`, `3des-cbc`, `hmac-md5`, or `diffie-hellman-group1-sha1` display a warning and are still subject to the installed OpenSSH client's own support and security policy.

Algorithm availability can differ between Windows OpenSSH, macOS OpenSSH, and Linux distributions. A syntactically valid profile can therefore still be rejected by the local OpenSSH build or by the remote server. Host-key checking remains governed independently by the existing strict/ask trust policy and is never weakened by these algorithm controls.

Support diagnostics report only whether each algorithm override and proxy-auth mode is configured; actual algorithm strings, proxy endpoints, credentials, and environment-variable values are omitted.
