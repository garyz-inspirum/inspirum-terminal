# Inspirum terminal: GUI redesign

Status: design baseline and native interaction prototype; not a production migration.
Date: 7 October 2026. Tracking: issue #78; native preview PR #83.

## 1. Design brief

The user rejected the existing interface as cluttered, difficult to read and difficult to use. The goal is not another colour patch. It is a fast, legible, terminal-first native workspace that borrows WindTerm's useful workflows while preserving Inspirum's SSH-first functionality and safety boundaries.

The main screen answers three questions immediately: Which session am I using? Which pane receives my input? Where do I go to connect or transfer a file? Configuration must not compete with the task being performed.

The current source couples UI and terminal ownership to eframe/egui_term. Framework migration is a separate engineering decision from interaction design. Iced is the preferred candidate for the redesigned shell, subject to an actual terminal integration and measurement gate.

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

The prototype is evidence of design exploration, not user acceptance. Model tests are not native interaction tests. A successful build does not prove the interface is pleasant, accessible or complete.

## 3. Primary task flows

### Open a saved server

Search or browse the left session library. The main screen contains saved sessions, folders/favourites and New connection, not an SSH settings form. The production design should distinguish selection from an explicit connection action where needed, and make duplicate/open-existing behaviour predictable. The prototype uses one click to open or focus an existing simulated workspace.

### Create or edit a connection

New connection opens a dedicated, keyboard-accessible dialog. Host, username and port are the basic fields; session name is optional. Authentication, identity path, proxy/jump host, tunnels, algorithms and advanced SSH policy are grouped on demand. Validate inline without discarding entries. Cancel changes nothing. Do not ask for or save authentication secrets in the profile form; preserve the existing OpenSSH prompt/agent boundary.

### Work in one or several terminals

Each session has a clear tab. A workspace can split into resizable panes with independent session identity and lifecycle. A thin accent outline and explicit focus state identify the input destination. Drag headers to rearrange; resize by divider; zoom a pane without disconnecting it. Close/reconnect must not replay input or silently enable broadcast.

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

The first native preview implements independent tabs, split/grid resizing and a lower resizable files drawer. Sidebar resizing, genuine connection lifecycle, complete keyboard focus trapping and full menus are not yet implemented in that preview.

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

**Decision: use Iced 0.14 for the isolated design prototype and evaluate it for the replacement shell. Do not change the production default yet.**

Iced supplies native application structure, theming, tasks/subscriptions and pane grids that support dynamic splits and resizing. The preview pins Iced 0.14.0. It uses tiny-skia for the first deterministic layout study. This is not proof that a future Iced terminal is faster than egui or WindTerm.

The possible terminal widget, iced_term 0.8.0, uses an Alacritty backend but must be audited rather than treated as a drop-in replacement. Current application code uses egui_term's PTY and parser ownership. Preserve and separate reusable profile, SSH launch/trust policy, transfer and remote-editor logic from rendering; review egui-coupled event, selection, clipboard, paste and process-lifecycle code explicitly.

The next integration spike must show a real SSH session with safe host-key handling, Unicode/CJK/IME, scrollback, resize, selection, clipboard, multiline/bracketed paste, alternate-screen apps, mouse reporting, multiple panes and clean shutdown. Compare GPU and fallback rendering on named hardware. Do not ship the fixture text display as a terminal.

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

Keep PTY I/O, DNS/authentication, transfers, filesystem work, search and remote-editor operations off the UI thread. Bound queues and apply backpressure; never drop terminal protocol bytes to improve apparent speed. Coalesce redundant redraw/resize requests, not data. Virtualise visible rows, cache text/layout where valid, and avoid holding parser/transfer locks through an entire frame. No meaningless animations or per-frame system polling.

Benchmark with a real terminal: burst output, ANSI styling, Unicode, selection during output, long scrollback, multiple sessions, a live transfer while typing, and close/cancel under load. Record both median and tail latency, plus resource cleanup.

## 9. Native review script

1. Launch at 1280x800 and at the documented minimum size. Confirm no SSH settings form occupies the workspace.
2. Search a profile and open it. Confirm selected session, tab and focused pane agree.
3. Create a connection using the keyboard. Exercise empty/invalid host and port, Cancel, Escape and focus restoration. No backend connection may occur before an explicit action.
4. Open two sessions, switch tabs and split one. Resize and reorder. Closing one workspace must not close another.
5. Open Files while the terminal remains visible. Confirm the remote target and queued job target cannot silently change.
6. Exercise a conflict/destructive action in a disposable backend fixture. Confirmation must state path and host.
7. Test 100/125/150/200 percent scale and Unicode labels. Inspect actual rendered screens, not only screenshots of a design tool.
8. Repeat keyboard, clipboard, IME, scrollback and shutdown checks natively on Linux, Windows and macOS.

The first preview workflow records Linux screenshots and builds the three native executables. It does not establish macOS/Windows visual acceptance, real terminal correctness, user approval or performance acceptance.

## 10. Delivery boundary

PR #83 is an isolated design-preview change, not closure of #78. No production GUI replacement, new protocol, credential storage, signing/notarisation or feature removal is included. The next milestone is the reviewed native layout plus a real-PTY Iced spike with safety and performance evidence.
