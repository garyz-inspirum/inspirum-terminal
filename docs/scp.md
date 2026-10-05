# SCP upload and download operations

Inspirum exposes explicit one-file upload and download actions through the system OpenSSH `scp` client. It does not implement a separate SSH or SCP protocol stack.

## Policy reuse

SCP operations reuse the saved profile's host-key mode, explicit SSH config, identity file, ProxyJump or structured proxy, authentication-method policy, GSSAPI policy, `IdentitiesOnly`, ControlMaster, compression, timeout, keepalive, user and port settings. Terminal-only remote commands, X11/agent forwarding and SSH port forwards are not applied to SCP.

Each local process argument is supplied directly to `Command::args`; Inspirum does not invoke a local shell for SCP operations. Remote file paths are passed as the remote operand expected by OpenSSH `scp`.

## Safe transfer lifecycle

Downloads first obtain the remote size through the existing SFTP policy path, then copy into a temporary file in the destination directory. The staged file must match the expected byte length before it is committed. Without explicit overwrite permission, the final commit is no-clobber.

Uploads first check for an existing remote destination unless overwrite permission was explicitly selected. The SCP payload is sent to an Inspirum staging name on the remote side. After SCP exits successfully, Inspirum checks the staged remote size and only then renames it to the requested destination. Failed or cancelled managed uploads attempt to remove the remote staging file.

The byte-length checks are completeness guards, not cryptographic integrity authentication. Linux acceptance additionally compares SHA-256 of the binary source, uploaded file and downloaded file.

## Progress and errors

The SCP panel reports queued/running completion state, total bytes, cancellation and errors. Download progress is derived from the growing local staging file. The OpenSSH `scp` command does not expose a stable machine-readable per-byte upload progress stream in this non-terminal mode, so uploads show the known total and verified completion rather than estimated live bytes.

## Platform limitations

- The system `scp` executable must be available on `PATH`.
- Inspirum does not enable legacy SCP protocol mode (`scp -O`). Current OpenSSH defaults to its SFTP-based transfer protocol; behavior depends on the installed OpenSSH version.
- Batch-style SCP operations cannot surface interactive password, passphrase, MFA or first-host-trust prompts through this panel. Use an agent, previously established ControlMaster, or other non-interactive OpenSSH authentication suitable for the profile.
- Windows local drive paths are delegated to the installed Windows OpenSSH `scp` parser. Native CI validates argv construction and Rust behavior, while isolated authenticated SCP server acceptance currently runs on Linux only.
- macOS compilation/tests run natively on Apple Silicon; authenticated SCP server acceptance is not yet run there. Existing macOS distribution restrictions described in the release documentation remain unchanged.

## Verification

Portable tests cover discrete argv boundaries, policy mapping, paths containing spaces/metacharacters and invalid profile combinations. The isolated Linux sshd fixture verifies a binary upload/download round trip with matching SHA-256 digests, no-clobber behavior, confirmed overwrite, failed-download preservation and failed-upload staging cleanup.
