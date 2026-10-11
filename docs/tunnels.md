# SSH tunnel manager

Inspirum can run the local (`-L`), remote (`-R`) and dynamic SOCKS (`-D`) forwards from the current profile as a dedicated forwarding-only OpenSSH process.

## Where it lives in the Iced GUI

- **Tunnels** in the top toolbar opens the centralised manager. The layout follows a WindTerm-class list-and-editor workflow: saved tunnel entries are on the left and the selected tunnel's structured editor is on the right.
- Press **+** beside a profile to add a tunnel directly in the manager. Select **Local**, **Remote** or **Dynamic**, give the entry a label, and edit its bind address/listen port plus destination host/port where applicable. Existing raw OpenSSH forward specs are converted into the structured editor automatically.
- Select an existing entry to edit or delete it. **Save** validates the resulting OpenSSH specification through the same session validation used elsewhere. A profile's running tunnel process must be stopped before its forward definitions can be changed.
- Each profile retains its own **Start** / **Stop** control; one forwarding-only `ssh -N -T` process serves all enabled forwards for that profile. Status and startup failure information remain visible beside the profile.
- User-facing labels are optional metadata; the existing `local_forwards`, `remote_forwards`, and `dynamic_forwards` vectors remain the execution source of truth for backward compatibility.
- The connection dialog's Advanced raw-spec fields remain available as an expert/compatibility path.

The manager UI was dropped when the egui frontend was removed (PR #84), restored in PR #128, and then expanded into the structured editor tracked by #129.

## Lifecycle and status

**Start tunnels** launches `ssh -N -T` with the profile's forwarding options and `ExitOnForwardFailure=yes`. Inspirum waits briefly for OpenSSH startup: if a requested listener cannot be created, the OpenSSH error is shown instead of reporting the tunnel as running. While the manager process is alive, configured forwards are shown as managed. **Stop tunnels** terminates and waits for that process; dropping the manager also performs best-effort process cleanup.

Reconnect is explicit: after a tunnel process exits or is stopped, start it again from the current validated profile. Inspirum does not silently reconnect a failed forwarding route.

## Bind safety

Prefer loopback listeners such as `127.0.0.1:8080:target:80`, `[::1]:8080:target:80`, or `127.0.0.1:1080`. An omitted bind address retains OpenSSH's normal default behavior. Explicit non-loopback binds such as `0.0.0.0`, `*`, or a LAN address can expose services to other hosts and require a fresh acknowledgement before the tunnel manager starts.

The acknowledgement is UI state, not saved in the profile.

## Verification

Portable tests verify forwarding-only argv construction and bind-exposure policy. The disposable Linux sshd suite already exercises local, remote and dynamic forwarding round trips and listener teardown; the tunnel-manager fixture additionally verifies that an occupied requested listener is reported as a startup failure and that explicit stop closes a managed listener.
