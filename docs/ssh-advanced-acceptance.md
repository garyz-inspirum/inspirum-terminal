# Advanced SSH native acceptance (issue #66)

Inspirum delegates network SSH transport to the host's system OpenSSH, preserving the host-key verification and option policies from saved profiles. Functional claims distinguish **arguments implemented**, **isolated native runtime tested**, and **not yet proven on that platform**.

## Disposable native CI acceptance

The existing `scripts/test-native-ssh-smoke.py` fixture launches two independent AsyncSSH servers on loopback: a trusted target, and an explicit jump server that accepts only forward requests to the fixture target. A temporary local TCP echo service provides a test destination. Real Inspirum OpenSSH PTYs are exercised from `tests/native_ssh_smoke.rs`.

The test suite checks:
- ProxyJump success with actual SSH terminal roundtrip; jump server records the forwarded channel
- Changed destination host key rejected through a working jump before user authentication
- Failed jump returns an SSH error, **without direct connection fallback** to a reachable target
- Local SSH forwarding (`-L`) carrying TCP bytes
- Dynamic SOCKS5 forwarding (`-D`) with handshake, connect and payload roundtrip
- Remote SSH forwarding (`-R`) carrying bytes back to a disposable client-side listener
- Loopback listener teardown on terminal exit and connection closure

The fixture uses temporary generated keys and known_hosts records, a file-scoped session profile, loopback-only targets, and deletes itself when finished. It never uses a production SSH server or private credential.

## Supported versus unverified

| Workflow | Native CI evidence |
| --- | --- |
| ProxyJump (success, changed key, jump failure) | Linux x64, Windows x64, macOS arm64 only if CI native fixture passes |
| Local, remote and dynamic TCP forwarding | Linux x64, Windows x64, macOS arm64 only if CI native fixture passes |
| Agent forwarding | OpenSSH option supported, but isolated credential-forwarding evidence still required |
| X11 forwarding | Requires a working local X server and server-side X11 tools; not claimed from headless native CI |
| Reconnect and split workspace GUI behavior | Separately tracked under GUI issue #78 |

Keep the issue open for agent-forwarding, X11 support limitations and fully documented per-platform test results. In particular, never conflate a green compile with successful runtime forwarding. Default listener addresses must remain bound to loopback; agent or X11 forwarding should display explicit risk context.
