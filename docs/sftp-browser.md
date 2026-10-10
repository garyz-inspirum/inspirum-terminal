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

The production connected-Iced Linux fixture uses native pointer events for
local/remote row selection and Upload/Download against an AsyncSSH chroot.
[Run 38041414853](https://github.com/garyz-inspirum/inspirum-terminal/actions/runs/38041414853)
at `ced8b1a11bfd2413350f50f4f19547db3f4c9a93` retained the selected remote
filename and completed queue states as GUI-action evidence, then separately
checked exact binary bytes in the expected disposable local and remote roots.
Neither screenshots nor the SFTP handshake alone are integrity proof. This
synthetic Linux result does not replace the broader backend matrix above or
human/native-device acceptance on Linux, macOS, or Windows.

Issue #78 remains open. Exact-head Linux evidence for PR #126 at
`e398ec0df6a808bb59668105b1572ab0229352b9` did **not** establish bottom-dock
usability: at 1280x800 one completed transfer clipped the file rows and two
completed transfers collapsed both listings because the natural-height queue
consumed their `Fill` space. At 960x640 the listing panels disappeared. That
artifact remains failure evidence only; it supports no minimum-window,
hardware, IME, cross-platform, or universal-usability claim. A replacement
layout and connected run must retain hit-testable listings after queue growth,
minimum-size resize, and bottom/right re-docking before this limitation can be
marked fixed. Roadmap issue #1 is unchanged.


## Local file manager and drag/drop transfers

Issue #61 completes the local side of the graphical SFTP workflow.

The **Local** pane supports native-path navigation, refresh, folder creation, rename, and explicit-confirmation delete. Deletes are deliberately conservative: files and empty directories only; recursive directory deletion is not performed. The path field uses the platform's native `PathBuf` semantics, so Windows drive-letter and UNC paths are preserved rather than rewritten as POSIX paths.

Remote downloads resolve the listed remote filename as exactly one local path component. Dot traversal, path separators, control characters, and absolute-path escapes are rejected before any local destination is created. Existing local destinations still require the existing explicit overwrite confirmation.

Local files can be dropped into the active SFTP browser for upload, but drops are ignored until **Accept dropped files for session '<name>'** is explicitly enabled. The armed session name is shown beside the control so a dropped file cannot silently target an unintended SSH session. Dropped files enter the same transfer queue as button-driven uploads and therefore use the same progress, cancel, retry, resume, integrity, and conflict behavior.

The current Iced/native-window integration accepts operating-system file drops into Inspirum Terminal. Native drag-out of a remote entry to the desktop is not exposed by the toolkit path used here, so remote-to-local transfer remains an explicit **Download** action into the visible Local pane. Internal transfer safety is unchanged: overwrite is never automatic and resumable downloads remain staged until final verification and atomic commit.


## Safe remote text editor

Issue #62 adds an explicit **Open/Edit** action for a selected remote file. The editor downloads the file into a private temporary working directory and opens it in Inspirum Terminal's internal multiline editor. The working copy is deleted when the editor is discarded or the application releases the editor state.

Only bounded UTF-8 text is editable. Files larger than 1 MiB, invalid UTF-8, NUL-containing data, or other binary-style control bytes are rejected rather than interpreted as text.

Closing the editor never uploads anything. **Save** is the only normal upload path. Before every save, the client downloads the live remote file again and compares its bytes with the version originally opened. If the remote content changed, saving stops and requires an explicit **Overwrite changed remote file** confirmation.

Successful saves upload the private working copy to a uniquely named remote staging file and then rename that staging file over the target. If the final rename fails, the client makes a best-effort cleanup of the staging file and reports the error. This staged replacement avoids directly streaming an edited file over the live target. Exact preservation of ownership, timestamps, ACLs and permission metadata depends on the SFTP server/filesystem rename semantics and is not currently guaranteed; operators who require metadata preservation should verify server behavior before using the editor for sensitive system files.

The editor never executes the edited file and never launches it as a local process.


## WindTerm-style Files pane

The graphical SFTP surface follows the same compact workspace language as the main terminal shell. Local and Remote are presented as side-by-side explorer panes with compact path bars and small parent/refresh controls. Low-frequency create/rename/delete forms are hidden under **Local actions** and **Remote actions** rather than occupying the primary workspace.

Upload, Download and Edit are grouped in a small transfer toolbar between the explorers and the optional editor. The remote editor uses a compact file header with modified/saved state plus explicit Save and Close controls. The transfer queue is only shown when jobs exist and is collapsible, keeping the normal file-browsing surface focused on the two file lists.
