# SSH diagnostics and privacy-safe support reports

Inspirum exposes the same privacy boundary through two surfaces:

- the headless command line, which can run without a graphical desktop;
- the graphical **Support diagnostics** panel, which can include the current validated app launch policy and recent sanitized in-memory error categories.

## Headless usage

```text
inspirum-terminal --diagnostics
inspirum-terminal --diagnostics --diagnostics-output support.txt
inspirum-terminal --diagnostics --profiles sessions.json --diagnostic-profile "Work laptop"
```

On Windows the executable is `inspirum-terminal.exe`. Existing export destinations are never overwritten. `--diagnostics-output` and `--diagnostic-profile` require `--diagnostics`.

## Graphical usage

Open **Support diagnostics** in the profile pane and choose **Generate support report**. The report is generated from the current validated draft profile, not from terminal screen contents. An optional export path can be supplied; export is no-clobber.

Inspirum keeps at most 12 recent sanitized error categories in memory for the current app run. Raw error strings are not retained by this history. The history has no timestamps, hostnames, usernames, paths, endpoints, commands or authentication responses and can be cleared explicitly. Closing the app discards it.

## What is collected

The support report contains:

- application version;
- operating system and CPU architecture;
- parsed local OpenSSH version;
- whether local `sftp`, `scp` and `ssh-keygen` return recognized usage responses;
- whether local OpenSSH advertises cipher, KEX, key and MAC query support;
- whether an explicit SSH config path is selected, without its path or contents;
- an allowlisted summary of the app launch policy: host-trust mode, whether username/port/identity/ProxyJump/remote-command overrides are configured, structured proxy kind, ControlMaster mode, forwarding counts, authentication policy states, compression and timeout/keepalive presence;
- recent sanitized in-memory error categories when generated from the GUI.

The app-policy summary describes what Inspirum itself will contribute to the OpenSSH launch. It deliberately does **not** evaluate inherited OpenSSH configuration with `ssh -G`, so inherited values are reported as inherited rather than expanded.

## What is never collected

The report never includes passwords, passphrases, private-key material, authentication responses, private-key paths, hostnames, usernames, profile names, proxy endpoints, forwarding endpoints, remote-command contents, known_hosts contents, terminal contents, raw connection/probe errors or arbitrary environment variables.

Recent errors are classified into fixed categories such as authentication failure, host-key verification failure, timeout, connection refused, network/name-resolution failure, proxy failure, multiplexing failure, tmux failure, transfer failure, forwarding failure or generic connection-operation failure. The source error text is never copied into the report.

Diagnostics do not authenticate, read private keys, read known_hosts, dump the process environment or run a remote command. The headless path does not open a graphical window. The local probes are fixed commands: `ssh -V`, `ssh -Q help`, `sftp -h`, `scp -h` and `ssh-keygen -?`.

## Probe and export safety

Each local probe has a two-second deadline, closed stdin and bounded capture. Captures over 64 KiB are rejected and raw stdout/stderr are not copied into the report except for narrowly parsed version/capability facts. Timed-out children are killed and reaped.

Support-text export is atomic/no-clobber: the destination directory must exist and an existing file is never replaced.

## Verification

Shared unit tests cover version parsing, timeout/output-limit handling, deterministic allowlisted policy rendering, bounded recent-error history, privacy canaries and no-clobber export. Canary values resembling passwords, endpoints, profile strings and arbitrary raw errors must not appear in rendered diagnostics.

The CLI integration suite runs the actual executable headlessly, verifies that arbitrary environment variables are absent, verifies optional profile summaries omit sensitive fields, checks no-clobber export, and runs diagnostics twice to prove deterministic output for unchanged local state.

CI runs the normal suite natively on Linux x64, Windows x64 and Apple Silicon. These diagnostics are a support aid, not proof of SSH connectivity, authentication capability or full WindTerm parity.
