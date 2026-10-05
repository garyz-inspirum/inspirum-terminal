# Local SSH diagnostics

Run the installed executable without a graphical desktop:

```text
inspirum-terminal --diagnostics
inspirum-terminal --diagnostics --diagnostics-output support.txt
inspirum-terminal --diagnostics --profiles sessions.json --diagnostic-profile "Work laptop"
```

On Windows the executable is `inspirum-terminal.exe`. An existing export destination is never overwritten. The destination directory must already exist. `--diagnostics-output` and `--diagnostic-profile` require `--diagnostics`.

## What the report contains

The report includes the application version, operating system/architecture, a parsed OpenSSH version, whether SFTP and ssh-keygen return recognized usage text, and whether OpenSSH supports local cipher/key-exchange/key/MAC queries. Probe problems use fixed categories such as missing tool, launch failure, timeout or excessive output; raw stdout, stderr and operating-system errors are not copied into the report.

An optional selected saved profile contributes only allowlisted booleans, policy states and forwarding counts. Names, hosts, usernames, port numbers, identity paths, jump destinations, forwarding endpoints and remote commands are omitted. Remote commands are never considered safe to share merely because they are profile metadata: users may have placed secrets in them. An unspecified profile means the profile store is not read at all. A missing/invalid store or non-unique selection is reported without disclosing identifiers.

## What it does not do

Diagnostics never open SSH/SFTP connections, authenticate, read private keys or known_hosts, evaluate SSH configuration, run a remote command, dump environment variables, inspect terminal contents or export recent session errors. `--ssh-config` is recorded only as an explicit/default source flag; its file is not opened or passed to a probe. No `ssh -G` or shell command is invoked.

The only subprocess arguments are `ssh -V`, `ssh -Q help`, `sftp -h` and `ssh-keygen -?`. The operating system's executables on PATH are trusted, just as for normal terminal connections. An unexpected vendor/version response is not proof that a tool is unsafe or absent; it is reported as unrecognized.

Each started probe has a two-second run deadline and closed stdin. Anonymous temporary files avoid pipe deadlocks and reader-thread leaks; file sizes are checked while the process runs, and captures over 64 KiB are rejected. Temporary files can briefly grow beyond this threshold between polls, so this is not a filesystem sandbox for malicious executables. The direct child is killed/reaped on timeout and early errors; fixed OpenSSH query commands are not expected to spawn descendants. Captures are removed after the probe.

The report is a support aid, not an SSH connectivity test, authentication capability guarantee, platform release certification or full WindTerm parity claim. Graphical diagnostics, effective-connection display and a deliberately sanitized recent-error history remain tracked in #20; this command is the bounded increment in #33.

## Verification

Unit tests in `src/diagnostics/tests.rs` use a native throwaway helper for missing-tool, nonzero exit, dual-stream capture, output overflow and timeout paths. Privacy-canary tests ensure profile strings/raw error text are not rendered. Export tests verify no-clobber behavior and temporary-file cleanup.

`cargo test --locked --test diagnostics_cli` runs the actual application with display variables removed, exercises an optional saved-profile summary and checks that report export cannot overwrite a file. No live server is needed. CI runs the normal suite on Linux x64, Windows x64 and Apple Silicon.
