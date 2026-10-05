# SSH connection multiplexing

Inspirum can either inherit OpenSSH multiplexing configuration, explicitly disable it, or manage a profile with `ControlMaster=auto`.

## App-managed mode

Select **Auto**, provide a `ControlPath`, and optionally set `ControlPersist` in seconds. Inspirum passes these values as OpenSSH argv options for SSH and SFTP; it does not implement a separate SSH transport and does not put credentials in the profile.

The profile editor exposes **Check master** and **Close master**. These use fixed-argument `ssh -S <path> -O check|exit` lifecycle commands. A missing or stale socket is reported as an error. Inspirum deliberately does not delete a stale control socket automatically.

## Inherited mode

**Inherit** does not pass ControlMaster/ControlPath/ControlPersist overrides. Existing behavior from the user's OpenSSH configuration therefore remains OpenSSH-owned and is not presented as an app-managed master. **Disabled** passes `ControlMaster=no`.

## Portability

Availability and exact socket/path semantics are determined by the installed OpenSSH client and operating system. The portable CI suite validates argument construction on Linux, Windows and macOS; the disposable Linux sshd suite additionally verifies creation, status, reuse-capable persistence, explicit close and post-close failure of a real master connection.
