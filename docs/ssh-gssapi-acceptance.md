# Kerberos/GSSAPI acceptance (issue #65)

## Scope and truthfulness

GSSAPI authentication is delegated to the **system OpenSSH** executable; Inspirum does not implement its own Kerberos stack. The SSH options `GSSAPIAuthentication` and `GSSAPIDelegateCredentials` remain explicit. Authentication must never silently fall back to password, agent or public key when verifying Kerberos support.

| Platform | Required native evidence | Runtime claim |
| --- | --- | --- |
| Linux x86-64 CI | Disposable MIT KDC + sshd realm, independent OpenSSH success baseline, Inspirum GSSAPI-only success, delegation disabled/enabled and destroyed-ticket failure | Only after the isolated realm job succeeds |
| macOS Apple Silicon | Native build/test of GSSAPI profile options; system OpenSSH version may differ in compiled GSSAPI capability | Not yet authenticated against a disposable Kerberos realm on macOS |
| Windows x64 | Native build/test of GSSAPI profile options; Windows OpenSSH distributions may omit GSSAPI | Not yet authenticated against a disposable Kerberos realm on Windows |

### Disposable Linux fixture

`scripts/test-native-gssapi.py` creates a fresh synthetic `INSPIRUM.TEST` realm inside a mode-0700 temporary directory. It creates an isolated KDC on a dynamic loopback port, one local test user principal, one `host/localhost` service principal, a throwaway OpenSSH host key, an unprivileged sshd loopback listener, a temporary ticket cache, and a fresh known-hosts file. The independent system OpenSSH control must authenticate to the same sshd before the Inspirum native PTY acceptance is exercised.

The adapter test runs with only `gssapi-with-mic` permitted; other SSH authentication mechanisms are disabled. It checks that non-delegated sessions cannot use remote Kerberos credentials, delegated sessions can, and missing or deliberately expired tickets cannot log in. The expiry test requests a disposable three-second ticket, verifies it becomes invalid, then checks authentication fails without fallback. Neither passwords, tickets, nor keytab material are printed; the fixture kills child daemons and deletes its root directory in `finally`.

### Usage and security constraints

Never point this test at a production realm or server. Only the CI Linux job installs packages; the application does not need MIT KDC. GSSAPI delegation should be enabled only for trusted hosts because it grants the target access to delegated credentials for their lifetime. A passing cross-platform build is not evidence of platform-specific GSSAPI availability. 