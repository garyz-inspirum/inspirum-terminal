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

## Queue, progress, cancellation, retry and resume

Transfers are queued and executed one at a time. Each row reports lifecycle state and byte totals. Downloads report bytes currently present in the staging file; uploads report their known total and transition through queued/running/completed state.

Running transfers can be cancelled. Cancellation terminates and waits for the OpenSSH child. A cancelled download preserves its private staging file and exposes **Resume**; OpenSSH `reget` continues from the preserved byte length and the normal final size check still runs before the destination is committed. A cancelled or failed upload can resume with OpenSSH `reput` when the remote partial is no larger than the local source. Resume is deliberately offered only from the same queue job: it is size-based continuation, not proof that an arbitrary pre-existing file has the same prefix.

**Retry** means restart from byte zero. For downloads it discards the queue-owned partial before starting again. Failed or cancelled jobs therefore provide an explicit choice between restart and continuation rather than silently overwriting or guessing.

Closing the Files view drops its queue; any running transfer is terminated by the transfer object's cleanup path. A download partial is retained only after an explicit cancellation or a child-process transfer failure where resume remains possible; completed transfers remove the staging path.

## Verification

Portable tests cover SFTP path/list parsing and browser navigation helpers. The isolated Linux sshd fixture covers graphical binary upload/download, remote browse/mkdir/rename/delete, a negative delete, local no-clobber behavior, failed integrity verification preserving an existing file, confirmed overwrite, cancellation with a preserved private partial, and upload/download continuation through `reput`/`reget`.
