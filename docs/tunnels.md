# SSH tunnel manager

Inspirum can run the local (`-L`), remote (`-R`) and dynamic SOCKS (`-D`) forwards from the current profile as a dedicated forwarding-only OpenSSH process.

## Where it lives in the Iced GUI

- **Tunnels** in the top toolbar opens the centralised manager. It lists every forward from every saved profile in one table: session, type (`L`/`R`/`D`), listen (`bind:port`), target (`host:port`) and status (Stopped / Running · ssh *pid* / Failed · *OpenSSH error*).
- Each profile has its own **Start** / **Stop** control; one forwarding-only `ssh -N -T` process serves all forwards of that profile. The footer shows the number of running tunnel sessions.
- Forwards are edited in the profile's connection dialog under *Advanced* → *Tunnels*: OpenSSH specs for Local, Remote and Dynamic, separated by `;`. They are saved with the profile and validated by the same rules as the CLI.

The manager UI was dropped when the egui frontend was removed (PR #84); the backend remained. This section documents the restored Iced UI.

## Lifecycle and status

**Start tunnels** launches `ssh -N -T` with the profile's forwarding options and `ExitOnForwardFailure=yes`. Inspirum waits briefly for OpenSSH startup: if a requested listener cannot be created, the OpenSSH error is shown instead of reporting the tunnel as running. While the manager process is alive, configured forwards are shown as managed. **Stop tunnels** terminates and waits for that process; dropping the manager also performs best-effort process cleanup.

Reconnect is explicit: after a tunnel process exits or is stopped, start it again from the current validated profile. Inspirum does not silently reconnect a failed forwarding route.

## Bind safety

Prefer loopback listeners such as `127.0.0.1:8080:target:80`, `[::1]:8080:target:80`, or `127.0.0.1:1080`. An omitted bind address retains OpenSSH's normal default behavior. Explicit non-loopback binds such as `0.0.0.0`, `*`, or a LAN address can expose services to other hosts and require a fresh acknowledgement before the tunnel manager starts.

The acknowledgement is UI state, not saved in the profile.

## Verification

Portable tests verify forwarding-only argv construction and bind-exposure policy. The disposable Linux sshd suite already exercises local, remote and dynamic forwarding round trips and listener teardown; the tunnel-manager fixture additionally verifies that an occupied requested listener is reported as a startup failure and that explicit stop closes a managed listener.
