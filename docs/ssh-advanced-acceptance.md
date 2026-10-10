# Advanced SSH native acceptance (issue #66)

Inspirum delegates network SSH transport to the host's system OpenSSH, preserving the host-key verification and option policies from saved profiles. Functional claims distinguish **arguments implemented**, **isolated native runtime tested**, and **not yet proven on that platform**.

## Disposable native CI acceptance

The existing `scripts/test-native-ssh-smoke.py` fixture launches two independent AsyncSSH servers on loopback: a trusted target, and an explicit jump server that accepts only forward requests to the fixture target. A temporary local TCP echo service provides a test destination. Real Inspirum OpenSSH PTYs are exercised from `tests/native_ssh_smoke.rs`.

The test suite checks:
- ProxyJump success with actual SSH terminal roundtrip; jump server records the forwarded channel
- Changed destination host key rejected through a working jump before user authentication
- Changed **jump-host** key rejected before hop authentication; destination never receives a fallback connection
- Failed jump returns an SSH error, **without direct connection fallback** to a reachable target
- Local SSH forwarding (`-L`) carrying TCP bytes
- Dynamic SOCKS5 forwarding (`-D`) with handshake, connect and payload roundtrip
- Remote SSH forwarding (`-R`) carrying bytes back to a disposable client-side listener
- Loopback listener teardown on terminal exit and connection closure
- Reconnect with the **same** -L/-R/-D listener ports after teardown, rerun -L/SOCKS5 TCP byte roundtrips, confirm the remote -R listener is requested again, and recheck all listener cleanup (on all native CI targets once green)

The fixture uses temporary generated keys and known_hosts records, a file-scoped session profile, loopback-only targets, and deletes itself when finished. It never uses a production SSH server or private credential.


The Linux `scripts/test-ssh-integration.sh` fixture starts an isolated synthetic `ssh-agent` socket with a generated public-key identity, permits forwarding only on its loopback test sshd, and tests both explicit `ForwardAgent=yes` and `ForwardAgent=no` through `tests/ssh_integration.rs`. Only presence/access to the forwarded agent is tested; keys and agent contents are never printed. Agent forwarding gives a remote host the ability to request signing operations with the local agent and should be enabled only for trusted hosts.

## Local capability preflight (advisory)

In the Iced connection editor, an explicit **X11 Enable** selection produces a warning when the application's environment has no non-empty `DISPLAY`. An explicit **SSH agent Enable** selection on Unix produces an advisory if `SSH_AUTH_SOCK` is not present. Disabled or inherited forwarding is not warned about. These are **local prerequisite hints only**: custom `IdentityAgent`, Windows OpenSSH agent services, XQuartz/VcXsrv and server-side configuration may alter effective capabilities. The application does **not** disable the user's chosen forwarding policy or claim native runtime support based on this environment check. Existing trust and risk warnings remain. When a user explicitly enables Windows agent forwarding, the connection dialog additionally states that its Windows-native runtime is unverified even if an agent service is present. Explicit X11 Enable on macOS or Windows similarly shows an unverified-runtime warning regardless of whether `DISPLAY` is set; it does **not** disable OpenSSH or make claims about XQuartz/VcXsrv. Inherit/Disable selections show neither platform capability warning.

## Supported versus unverified

| Workflow | Native CI evidence |
| --- | --- |
| ProxyJump (success, changed key, jump failure) | Linux x64, Windows x64, macOS arm64 only if CI native fixture passes |
| Local, remote and dynamic TCP forwarding | Linux x64, Windows x64, macOS arm64 only if CI native fixture passes |
| Agent forwarding | Linux x64 isolated ssh-agent + OpenSSH sshd opt-in/out (#93). Additional macOS arm64 and Linux x64 disposable AsyncSSH + genuine ssh-agent native acceptance checks the forwarded agent identity listing and explicit disable through the Inspirum terminal adapter **only after the new native CI passes**; Windows named-pipe agent runtime is not verified. |
| X11 forwarding | Linux isolated Xvfb/xauth, genuine OpenSSH -X control and Inspirum client opt-in/opt-out via PTY, **only if CI passes**; Windows/macOS runtime remains unverified |
| Reconnect and split workspace GUI behavior | Separately tracked under GUI issue #78 |

An isolated Linux X11 fixture starts Xvfb with its own temporary xauth cookie, permits X11Forwarding only on a loopback test sshd, validates a system OpenSSH `-X` control, and then verifies Inspirum `Some(true)` can query the forwarded display while `Some(false)` does not expose `DISPLAY`. This is a Linux-only runtime claim and does not prove native macOS XQuartz or Windows X server compatibility. X11 access gives a trusted remote host access to the local display; enable only when necessary.

The native Unix ssh-agent fixture in `scripts/test-native-ssh-smoke.py` generates an ephemeral key, launches an isolated loopback AsyncSSH SSH server with agent forwarding enabled, and probes the server-visible agent identities over the actual forwarded UNIX socket. Three fresh OpenSSH sessions test explicit Enable → Disable → Enable after teardown, assert forwarded identities are available only to the enabled connections, and require deterministic cleanup; no credentials or key lists are logged. The macOS and Linux CI results must be green before describing the runtime as verified. Windows agent named-pipe runtime remains unverified; a Unix socket test cannot validate Windows.

Keep the issue open for native Windows/macOS runtime X11 limitations and any remaining negative/reconnect checks. In particular, never conflate a green compile with successful runtime forwarding. Default listener addresses must remain bound to loopback; agent or X11 forwarding should display explicit risk context.
