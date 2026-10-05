# Interactive SFTP sessions

The **SFTP** button opens the system OpenSSH `sftp` client in an application terminal tab. This is an interactive command-line file-transfer session, not the graphical browser/queue planned in issue #17.

## Connection and authentication policy

The selected profile supplies host/config alias, user, port, identity-file path, ProxyJump, strict host-key policy, compression, timeout and keepalive. All six authentication policies now apply to SFTP as well as SSH: public key, password, keyboard-interactive, GSSAPI authentication, GSSAPI credential delegation and `IdentitiesOnly`.

**Inherit** emits no override; **Enable** and **Disable** emit explicit OpenSSH options. In particular, disabling public-key authentication must not be ignored just because an identity file is present in SSH config. GSSAPI options do not establish platform capability: actual authentication still requires an appropriate OpenSSH build and Kerberos environment.

Using an SSH agent for authentication is different from forwarding it. The profile's terminal-only agent/X11 forwarding settings, port forwards and remote command are not added to SFTP. Authentication responses and private-key material stay with OpenSSH; this change adds no credential storage.

Enter an IPv6 address in the profile without brackets, for example `2001:db8::9` or `fe80::1%eth0`. Inspirum brackets the SFTP destination so its colons are not interpreted as a remote path. Ordinary SSH receives its existing unbracketed host argument. Hostnames and configuration aliases are unchanged.

Unknown-host confirmation and changed-host rejection remain OpenSSH's responsibility. Do not accept a new key without independently verifying its fingerprint.

## Verification

`cargo test --locked --test sftp_policy` checks tri-state policy parity, destination encoding, discrete path arguments and terminal-only exclusions on every native CI target. It does not perform GSSAPI authentication or IPv6 network access.

On Linux, `scripts/test-ssh-integration.sh` also runs three ignored tests from `sftp_policy` against the disposable SFTP server: explicit-key success/exit, disabled-public-key rejection despite a valid configured identity, and changed-host-key rejection. The existing binary upload/download test remains in `ssh_integration`. No production server or user trust store is used.

Windows/macOS authenticated-server acceptance, IPv6 network fixtures, graphical transfer progress/cancellation and a transfer queue remain separate release gates. See issue #31 for the policy fix and `ssh-acceptance.md` for the broader matrix.

## Reference

The OpenSSH SFTP manual documents `-o` for SSH configuration options and square brackets for IPv6 destinations: <https://man.openbsd.org/sftp.1>. Configuration precedence and authentication options are described at <https://man.openbsd.org/ssh_config.5>.
