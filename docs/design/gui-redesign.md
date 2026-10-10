# Inspirum terminal: GUI redesign

Status: all GUI workflows have Iced implementations. Iced is the only frontend; the egui fallback has been removed. Manual native responsiveness/IME acceptance remains open in #78.
Date: 8 October 2026. Tracking: issue #78; migration PR #83.

## 1. Design brief

The user rejected the existing interface as cluttered, difficult to read and difficult to use. The goal is not another colour patch. It is a fast, legible, terminal-first native workspace that borrows WindTerm's useful workflows while preserving Inspirum's SSH-first functionality and safety boundaries.

The main screen answers three questions immediately: Which session am I using? Which pane receives my input? Where do I go to connect or transfer a file? Configuration must not compete with the task being performed.

The migration now uses Iced 0.14 for the production candidate shell while preserving the existing OpenSSH launch policy, PTY/parser ownership, paste safeguards, profile store and transfer backends. The legacy egui frontend and runtime dependencies have been removed. The reusable PTY/parser backend is extracted into toolkit-neutral terminal_core.

## 2. Design process and evidence

| Stage | Deliverable | Acceptance |
| --- | --- | --- |
| Diagnose | User feedback, current UI/terminal adapter inspection, feature inventory | Explain the workflow problem, not just styling preferences |
| Structure | Task flows and placement map below | Every existing feature has a reachable home |
| Prototype | Isolated native Iced executable | Search, form, tabs, splits, files drawer and scale controls operate on labelled fixtures |
| Inspect | Native screenshots of start, terminal, split, files and connection form | No clipped labels; readable hierarchy; active target obvious |
| Exercise | Task-based keyboard/mouse tests at native display scales | Correct focus, no accidental connection or destructive action |
| Integrate | Real SSH/PTY Iced spike, reusing policy and transfer logic | Preserve trust, paste and process-lifecycle safeguards |
| Measure | Latency, startup, CPU, memory and stress report | Measured budgets on named hardware; no performance claims from a mock shell |
| Promote | Full feature mapping plus native platform acceptance | Only then replace the default production frontend |

The migration branch now contains a live production-backend Iced implementation rather than only fixture UI. It opens real SSH sessions through the existing OpenSSH/PTY path, renders live terminal state, handles keyboard/mouse/selection/scrollback, and integrates the existing SFTP transfer and remote-editor backends. This is still not user acceptance: successful CI does not prove the interface is pleasant, accessible or complete.

## 3. Primary task flows

### Open a saved server

Search or browse the left session library. The main screen contains saved sessions, folders/favourites and New connection, not an SSH settings form. The production design should distinguish selection from an explicit connection action where needed, and make duplicate/open-existing behaviour predictable. The prototype uses one click to open or focus an existing simulated workspace.

### Create or edit a connection

New connection opens a dedicated, keyboard-accessible dialog. Host, username and port are the basic fields; session name is optional. Authentication, identity path, proxy/jump host, tunnels, algorithms and advanced SSH policy are grouped on demand. Validate inline without discarding entries. Cancel changes nothing. Do not ask for or save authentication secrets in the profile form; preserve the existing OpenSSH prompt/agent boundary.

### Work in one or several terminals

Each session has a clear tab. A workspace can split into resizable panes with independent session identity and lifecycle. A thin accent outline and explicit focus state identify the input destination. Drag headers to rearrange; resize by divider; zoom a pane without disconnecting it. Close/reconnect must not replay input or silently enable broadcast.

### Auxiliary SSH shell beside the main terminal

The workspace's **Shell** toolbar action (Ctrl/Cmd+Shift+J while no text
editor has captured the keys) opens an independent authenticated SSH PTY for
the active session in a right-hand split. It is identified as **AUX SHELL**
and receives input only when explicitly focused. The main terminal remains
connected; the shell may run different commands without sharing its PTY or
scrollback. The shell uses the current session's existing host-key and
authentication policies; opening it does not change sync-input targets.

**Close shell** disconnects that auxiliary PTY without closing the original
terminal. If it is the last pane, the tab's existing close confirmation
continues to apply. Reconnect preserves the auxiliary-shell role. The
four-pane workspace limit applies, and the privacy curtain prevents shell
creation/teardown. The Files dock remains separate. This is a practical
auxiliary shell workflow in the existing draggable split grid, **not** a
second independently dockable file-explorer tree. Native Mac click latency
and keyboard/IME acceptance remain tracked in #78.

### Transfer files without losing the terminal

Files opens a resizable lower drawer. Local and Remote have unambiguous headings; the remote side shows the exact session and path. Queued transfers retain their original session ID even when the active tab changes. Progress and errors are real, not decorative status values. Upload/download, conflict/resume, destructive operations and editor save retain existing confirmation and integrity logic.

### Find a command or a less-common feature

Use the command palette and contextual menus. Distinguish application actions from text that will be sent to a server. Snippets/completion stage text rather than execute it. Broadcast controls require explicit target selection and a persistent warning, not a quiet toolbar toggle.

### Recover from an error

Connecting, authentication requested, connected, disconnected, host-key conflict and transfer failure are distinct states. Give an actionable explanation and retry/cancel action. Keep typed connection data after a validation error. Never turn a trust failure into automatic key acceptance.

## 4. Layout specification

- A compact top bar for workspace identity and global actions; native window chrome is retained.
- Left session library: prototype width 256 logical pixels; production must support resize/collapse with a usable minimum width and remembered preference.
- Central tabs and terminals occupy the primary area. The disconnected/start state is quiet: New connection and sample/recent/saved sessions, not empty configuration forms.
- A collapsible files/transfers drawer uses approximately the lower 30-40 percent when open, with a draggable divider. It must not silently replace the terminal.
- A small status bar shows real session state, encoding/terminal context and relevant jobs. Do not invent latency or throughput readings.
- Global preferences and support tools live in dedicated dialogs/panels. Advanced forms do not occupy the default session tree.

The current Iced candidate implements independent tabs, split/grid resizing, reconnect/close lifecycle, a lower resizable Files drawer, live SFTP transfers, remote editing, saved-session launch shortcuts, and validated merge-only profile import. Sidebar width remains fixed apart from collapse/expand, and complete native focus/IME/accessibility acceptance is still outstanding.

## 5. Visual system

| Token | Value | Role |
| --- | --- | --- |
| Workspace | #0E1219 | Quiet terminal/background surface |
| Sidebar/panels | #161D28 | Secondary navigation |
| Elevated/selected | #1E2837 | Hover and selected surfaces |
| Divider | #303E51 | Boundaries without heavy boxes |
| Main text | #E7EDF7 | Primary labels and terminal content |
| Secondary text | #A3B1C4 | Metadata, never almost-black grey |
| Accent | #7BB0FF | Focus and primary action |
| Success | #71D9AB | Positive state, paired with text |

Start with 14 logical-pixel UI text, 15-16 pixel terminal text, 12-13 pixel secondary labels, 32-36 pixel controls and a consistent 4/8/12/16 spacing rhythm. These are starting measurements, not fixed requirements at every display scale. Use an ordinary readable system UI font and a proper monospace terminal font; preserve independent font settings in production. Avoid gradients, oversized dashboards, decorative animation and a different colour for every tool.

Selected, hovered, focused, disabled, pending and error states need separate treatment. Colour alone is insufficient. Production acceptance includes visible keyboard focus, contrast measurement, 100/125/150/200 percent scaling and a light/high-contrast alternative without changing interaction placement.

The native preview deliberately uses simple text controls while the hierarchy is evaluated. A consistent original line-icon set and compact icon-with-tooltip controls are a later visual pass, not a prerequisite for understanding the first layout.

## 6. Feature placement and preservation

This is a migration map, not a claim that every mapped feature has been implemented in the preview.

| Existing feature area | New location | Behaviour to preserve |
| --- | --- | --- |
| Profiles, folders, favourites, tags, search | Session library and profile context menu | Atomic non-secret storage, rename/duplicate/delete semantics |
| Import/export and startup/restore | Session/library actions and preferences | Collision handling; no unsolicited reconnection |
| Host, user, port and configuration path | Basic connection dialog | Validated argv construction; no local shell interpretation |
| Identity/authentication/GSSAPI | Authentication section | Inherit/enable/disable policies; no secret persistence |
| ProxyJump, authenticated proxies, SSH algorithms | Advanced connection sections | Routing policy, warnings, credential boundaries and no unsafe fallback |
| Tunnels, forwarding and ControlMaster | Session tools / connection sections | Explicit lifecycle, listener status and risk acknowledgement |
| Host trust / known_hosts management | Connection security dialog | Changed-key rejection and explicit destructive confirmation |
| Tabs, split panes and workspace restore | Central workspace | Stable session/pane IDs and independent lifecycle |
| Synchronized input | Explicit sender/broadcast panel | Named targets, prominent armed state, disarm on lifecycle changes |
| History search, marks, timestamps, folding | Terminal search/navigation tools | Correct retained-buffer behaviour and bounded work |
| Appearance and pointer/copy settings | Preferences with profile overrides | Independent terminal settings and safe paste policy |
| Command palette, snippets, completion | Palette and sender tools | Separate app actions from remote text; no auto-execute |
| Local/remote browser, SFTP/SCP and transfer queue | Lower Files/Transfers drawer | Path safety, overwrite/conflict, resume, integrity and cleanup |
| Remote editor | Editor tab associated with the correct session/file | Temporary copy, conflict detection, explicit overwrite and cleanup |
| Diagnostics, support export and logging | Help/support and session tools | Redaction, bounded storage and logging off by default |

The full current behaviour is defined by source and tests, not by a stale checked box in the roadmap. Reconcile the map against those tests before porting each feature.

## 7. Framework decision: Iced

**Decision: Iced 0.14 is the only production frontend. Native acceptance gates remain independent of frontend removal.**

Iced supplies native application structure, theming, tasks/subscriptions and pane grids that support dynamic splits and resizing. The migration pins Iced 0.14.0. Linux/Windows use WGPU with a tiny-skia fallback. The macOS performance follow-up enables Metal/WGPU with tiny-skia also built, but must pass third-party license-notice verification before distribution. Hardware timing and renderer-selection evidence are still required: merely compiling WGPU is **not** proof that it was selected at runtime or that it is faster than tiny-skia, egui or WindTerm.

The possible terminal widget, iced_term 0.8.0, uses an Alacritty backend but must be audited rather than treated as a drop-in replacement. Current application code uses toolkit-neutral terminal_core for PTY/parser ownership. Profiles, SSH launch/trust, transfers and remote editors retain their shared policies. The backend has no egui runtime dependency.

The live Iced path now covers real SSH sessions, safe host-key policy, committed CJK text, scrollback, resize, selection, clipboard guards, multiline paste confirmation, mouse reporting, multiple panes and session shutdown. Display snapshots honor application cursor hide/show (DECTCEM) and suppress the live cursor while viewing scrollback. The production Iced renderer also underlines a visible matching (), [] or {} pair when the terminal cursor rests on one of its brackets. It handles nesting and unmatched cases, is read-only, and recalculates the highlight on terminal display generations rather than every redraw; the visible snapshot (not offscreen history) is its deliberate scope. Disposable local PTY tests exercise hide/show and scrolling back to the live cursor on Unix; these do not establish Windows ConPTY or full-screen GUI acceptance. Remaining acceptance includes native IME composition behaviour, alternate-screen/full-screen application testing, edge-case mouse modes, focus restoration/accessibility, and measured GPU/fallback performance on named hardware.

### Visible URL highlighting (issue #64)

The Iced terminal canvas underlines detected `http://` and `https://` addresses in the visible terminal row, excluding common sentence punctuation. Detection happens only when an existing cached row is redrawn; it does not scan the history buffer or introduce a per-frame URL parsing pass. This is a **visual cue only**: the terminal does not automatically open links, execute commands, alter SSH output, or grant a remote host browser/clipboard access. Wrapped/multiline URLs, terminal OSC 8 link metadata and click-to-open behavior are not supported by this bounded enhancement.

### Inline free-type composer (issue #64)

The terminal pane header offers **Free type**, with **Ctrl/Cmd+Shift+E** as a keyboard shortcut. On entry the matching pane's text editor immediately receives keyboard focus (a unique editor ID prevents another split pane taking focus). This opens a compact, editable multiline text area directly beneath that pane's terminal viewport; the terminal remains visible and its resize sensor measures only the terminal viewport. The pane header reads **FREE TYPE** while active. The composer is a local draft **not an editable remote terminal or a movable remote cursor**: the user can position the editing caret freely in the draft, but remote PTY writes and mouse-report commands for that pane are blocked until the draft is explicitly sent or discarded. Drafts are isolated per terminal ID and dropped on pane closure/reconnection; draft content stays in memory and is never persisted.

**Send explicitly** checks the same paste policy as the normal terminal paste path. If the draft contains multiple lines, the existing preview/normalization confirmation is required, and confirming validates the original text, focused pane and synchronized target IDs again. Cancel preserves the draft; discarding is explicit. An empty or policy-blocked payload is never sent. The model deliberately does **not** claim arbitrary shell cursor positioning, Vim editing, or free-text placement inside the remote PTY: those capabilities require separate scope and live GUI acceptance.

### Local-only terminal navigation (issue #64)

Shift+Enter enters a pane-local scrollback navigation mode and excludes that pane from remote PTY writes, mouse-report forwarding, delayed paste and synchronized command input. Arrow keys and PageUp/PageDown scroll locally. Optional vi-inspired bindings are **j/k** (down/up one row), **Ctrl+d / Ctrl+u** (down/up one page), and **g/G** (top/bottom of available scrollback). These are display navigation gestures, **not** full vi modal editing, horizontal cursor positioning or remote shell commands. Shift+Enter or Escape exits. The active pane explicitly displays LOCAL NAV; other split panes are unaffected.

Primary references checked 7 October 2026:

- https://iced.rs/
- https://docs.rs/iced/0.14.0/iced/
- https://docs.rs/iced/0.14.0/iced/widget/fn.pane_grid.html
- https://docs.rs/crate/iced/0.14.0/features
- https://docs.rs/iced_term/0.8.0/iced_term/
- https://github.com/kemokempo/iced_term
- https://kingtoolbox.github.io/
- Repository: src/app.rs and src/terminal.rs at b66f8dc77ebdd8227746c9c62c53659b394db790.

## 8. Performance architecture and provisional budgets

**Targets below are proposed acceptance budgets, not measured results.** Establish the current baseline and name the machine, OS, display scale, renderer and workload before using them as a release gate.

| Measurement | Initial target | Method |
| --- | --- | --- |
| Local key/action to visible feedback | p95 below 50 ms | Timestamp input and presentation; exclude network round-trip |
| Active resize/scroll frame time | p95 near or below 16.7 ms at 60 Hz | Trace frame times, not average FPS alone |
| First usable window | Around 1.5 seconds on a declared reference machine | Cold/warm launches measured separately; do not wait on network |
| Idle work | No constant forced repaint loop | Observe idle wakeups and CPU against the current build |
| Memory | Bounded per-session scrollback, queues and file pages | Measure RSS for 1, 10 and 30 tabs; no universal MB promise yet |
| File-list interaction | No UI stall on large directories | Test 10k/100k-entry fixtures with paging/virtualisation |

Keep PTY I/O, DNS/authentication, transfers, filesystem work, search and remote-editor operations off the UI thread. Bound queues and apply backpressure; never drop terminal protocol bytes to improve apparent speed. The current Iced path coalesces duplicate PTY wakeups, defers active-terminal snapshots behind a short frame boundary, skips unchanged snapshots, and caches terminal canvas geometry per visible row so a one-line update does not rebuild every row. Resize events are reduced to actual terminal row/column changes. No terminal protocol bytes are dropped.

Benchmark with a real terminal: burst output, ANSI styling, Unicode, selection during output, long scrollback, multiple sessions, a live transfer while typing, and close/cancel under load. Record both median and tail latency, plus resource cleanup.

### Opt-in slow-stage tracing

The macOS GPU experiment is tracked by issue #78. Compare the **same** terminal workload on a named Apple Silicon Mac using the GPU-enabled binary and the prior software-only build. Capture the renderer actually selected, external click-to-caret p50/p95, text entry latency, CPU/GPU and display scale. Falling back to software is permitted if Metal is unavailable; do not hide missing-license notices or claim a speed improvement based on CI compile success.

To diagnose the reported macOS click-to-focus lag, start the actual Iced binary with:

```bash
INSPIRUM_ICED_TRACE_MS=25 ./inspirum-terminal --ui iced 2> iced-timings.log
```

This prints a line **only when** one of these synchronous stages takes at least 25 ms: `startup`, `event_update`, `view_layout`, `terminal_snapshot`, or `terminal_canvas_draw`. Set a different positive millisecond threshold if needed. Tracing is disabled unless explicitly requested, and trace records contain only static stage names and elapsed times, not SSH hosts, commands, passwords, clipboard text, terminal output or file paths. If sharing results, extract only lines beginning `iced slow` from the log; unrelated process diagnostics may include sensitive data.

With this option enabled, clicks on the **terminal canvas** also report `iced slow click_to_canvas_draw` when the interval from the delivered selection event to the next focused canvas drawing exceeds the selected threshold. This lets an affected Mac distinguish a delayed application redraw from a slower GPU-present or OS event-delivery path. It does not instrument ordinary Iced text inputs; for median/p95 diagnosis lower the threshold (for example to 1 ms), test at least 30 clicks, and use only that stage's static timing lines. Tracing stores only pane IDs and monotonic instants in a bounded in-memory map; neither hostnames nor terminal text are logged.

After recording a local trace with `INSPIRUM_ICED_TRACE_MS=1`, run `python3 scripts/analyze-iced-trace.py iced-timings.log` to report click-to-canvas p50/p95 using nearest-rank percentiles. The script discards all nonmatching log lines and warns when fewer than 30 clicks are present. These measurements are **not** click-to-visible-caret or GPU-present timings, and cannot close #78 without named-hardware external observation.

These timings measure **CPU-stage durations, not input-to-screen latency**. A slow-GPU presentation or OS event delivery issue may need native macOS Instruments tracing. For acceptance, measure on the affected Mac and record its model, macOS version, display scale, renderer and connected-tab count. Never claim the p95 budget is met from these logs alone.

## 9. Native review script

1. Launch at 1280x800 and at the documented minimum size. Confirm no SSH settings form occupies the workspace.
2. Search a profile and open it. Confirm selected session, tab and focused pane agree.
3. Create a connection using the keyboard. Exercise empty/invalid host and port, Cancel, Escape and focus restoration. No backend connection may occur before an explicit action.
4. Open two sessions, switch tabs and split one. Resize and reorder. Closing one workspace must not close another.
5. Open Files while the terminal remains visible. Confirm the remote target and queued job target cannot silently change.
6. Exercise a conflict/destructive action in a disposable backend fixture. Confirmation must state path and host.
7. Test 100/125/150/200 percent scale and Unicode labels. Inspect actual rendered screens, not only screenshots of a design tool.
8. Repeat keyboard, clipboard, IME, scrollback and shutdown checks natively on Linux, Windows and macOS.

The Iced workflow builds the production candidate on Linux, Windows and macOS and records Linux design-state screenshots. The expanded Linux fixture uses an ephemeral AsyncSSH loopback host and generated SSH keys to click a saved profile in the real Iced application, confirm actual SSH authentication and independently opened PTYs, invoke keyboard-only Ctrl+Shift+R split and Ctrl+Shift+F files-dock shortcuts, capture connected/split/dock screenshots, and assert significant layout changes. A subsequent Ctrl+Shift+E check focuses the inline per-pane free-type editor, types a fixed synthetic line plus Enter, checks the draft changed visually, and verifies the synthetic line was not received by the remote SSH session before Send. The same connected GUI fixture toggles Alt+Enter focus mode and Ctrl+Shift+L privacy curtain using native keyboard events, captures and pixel-compares both states, and verifies fixed synthetic text typed while the curtain is active **never reaches the remote SSH PTY**. The connected Linux fixture also requests alternate-screen enter/exit using fixed VT control sequences through the authenticated PTY and records active/restored screenshots; this tests bounded fullscreen protocol rendering, **not** the interaction behavior of Vim, tmux, or arbitrary remote applications. This proves native Linux GUI connected-state smoke, not SFTP transfer success or native macOS/Windows connected GUI acceptance. It also launches the **real production Iced executable** in a headless Linux desktop with an empty local profile store, captures the empty workspace and New Connection dialog, exercises Escape, and checks the process stays alive. These are automated startup/dialog smoke checks and screenshots for subsequent visual inspection, **not** proof of full native GUI usability. CI also exercises the production Iced feature tests. macOS/Windows visual acceptance, named-hardware latency measurements, real-network interactive acceptance and user approval are still outstanding.

## 10. Delivery boundary

PR #84 completes the GUI workflow migration and removes the egui fallback and dependencies. This does not close #78: manual native IME/full-screen-terminal usability and measured responsiveness on named hardware remain outstanding. No credential storage, signing/notarisation or trust bypass is introduced.
