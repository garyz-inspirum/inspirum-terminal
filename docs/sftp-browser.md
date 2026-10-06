# Graphical SFTP browser and transfer queue

Inspirum provides a graphical file workflow on top of the system OpenSSH `sftp` client. It does not implement its own SSH stack.

## Connection policy

The graphical browser derives its SFTP argv from the same validated profile path as the interactive SFTP terminal. Host-key strictness, explicit SSH config, identity file, ProxyJump or structured proxy, authentication-method restrictions, GSSAPI policy, `IdentitiesOnly`, ControlMaster, compression, timeout, keepalive, user and port are therefore shared.

The graphical workflow uses OpenSSH batch commands for machine-readable operations. Credentials are not stored by Inspirum. Profiles that require an interactive password, MFA, passphrase or first-host-trust prompt should authenticate through a suitable OpenSSH/agent/ControlMaster workflow first; a batch operation cannot display those terminal prompts itself.

## Browsing and remote operations

The Files view shows local and remote directories side by side. Double-click folders to enter them, or use Up and Refresh. Remote operations include:

- create directory;
- rename the selected entry;
- delete a selected file or empty directory with explicit confirmation;
- upload the selected local file;
- download the selected remote file.

Paths are passed to the SFTP command parser with control-character rejection and quoting. They are not interpolated into a local shell.

## Conflict safety and integrity

Overwrites are never automatic. A local download collision or a remote upload name collision requires an explicit Overwrite action.

Downloads are written to a temporary file in the destination directory. The existing destination is not removed before transfer. When the remote listing supplied a size, the staged download must match that byte length before it is committed. A failed transfer or failed verification leaves the previous destination unchanged.

Uploads record the local source length and, after a successful SFTP transfer, query the remote length. A size mismatch is reported as an integrity failure. This is transfer-completeness verification by byte length, not a cryptographic hash.

## Queue, progress, cancellation and retry

Transfers are queued and executed one at a time. Each row reports lifecycle state and byte totals. Downloads report bytes currently present in the staging file; uploads report their known total and transition through queued/running/completed state.

Running transfers can be cancelled. Cancellation terminates and waits for the OpenSSH child. A cancelled or failed download keeps its queue-owned staging file only while that queue row remains available, so **Resume** can continue with OpenSSH `reget`; choosing **Retry from start** drops the staging file first. Failed or cancelled uploads to a path that did not exist before the job can resume with OpenSSH `reput`. Resume is deliberately not offered after a confirmed overwrite upload because a pre-existing remote prefix cannot be proven to belong to the local source.

Closing the Files view drops its queue; any running transfer is terminated and any retained local download staging file is removed by the transfer object's cleanup path. Resume is therefore an explicit in-queue recovery operation, not a promise to retain partial files across application restarts.

SCP remains a separate explicit upload/download workflow. OpenSSH `scp` does not expose a safe resumable offset contract through this integration, so users who need resumable transfers should use the SFTP queue.

## Verification

Portable tests cover SFTP path/list parsing and browser navigation helpers. The isolated sshd fixture covers graphical binary upload/download, remote browse/mkdir/rename/delete, a negative delete, local no-clobber behavior, failed integrity verification preserving an existing file, confirmed overwrite, cancellation cleanup, and resumed download/upload byte integrity.
