# UI acceptance reconciliation

## Snapshot and decision

Evidence checked 10 October 2026 against `main` at
`8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018` ([PR #121][pr121]).
This is a documentation reconciliation, **not final UI acceptance**.
Iced is the only production frontend. Bounded implementation issues
[#64][issue64] and [#66][issue66] are closed; [#78][issue78] and the
[roadmap #1][roadmap] remain open. No issue acceptance is removed, waived or
silently narrowed by this document.

Three different claims must remain separate:

1. **Implemented:** source and regression tests expose the feature.
2. **Automated evidence:** only the platform, fixture and paths actually run.
3. **Device acceptance:** a named tester exercises the production connected
   application on named hardware, records measurements and judges usability.

A green build, a synthetic preview, a screenshot, a SFTP handshake, and a
human-approved end-to-end transfer are not interchangeable evidence.

## Evidence index

| ID | Immutable baseline / evidence | What it establishes |
| --- | --- | --- |
| E1 | [PR #121][pr121], merge `8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018`; [#64 completion record][close64] | Bounded interaction implementation merged; remaining device qualification is explicitly retained |
| E2 | [Main CI 38034556562][mainci], merge SHA above | Success: three native build/test/package jobs and three independent downloaded-package verification jobs (Linux x64, Windows x64, macOS ARM64) |
| E3 | [PR-head CI 38034023790][prci], head `033e5b7153be08d5d59295ae0259c8250c6a2e4d` | Successful pre-merge native CI and package verification; not mislabeled as a run at the merge SHA |
| E4 | [Iced run 38034023898][icedci], same PR head; [Linux connected job][linuxjob] | Three successful native Iced jobs; real connected production GUI automation runs on Linux only. Windows/macOS connected GUI steps are skipped by workflow conditions |
| E5 | [Current launcher][launcher], [manifest][manifest], [Iced source/tests][icedsource], [source-first dispatch][dispatch], [native two-PTY test][ptytest] | Iced-only entry point, WGPU/tiny-skia dependencies, per-pane modes, destination-specific cursor encoding and failed-source isolation |
| E6 | [Connected GUI fixture][fixture], [Iced workflow][workflow] | Real authenticated system-OpenSSH client panes against an isolated AsyncSSH server, including its SFTP subsystem, resize, draft/privacy isolation, focus/alternate-screen states and remote Left-key delivery; fixture boundaries are inspectable |
| E7 | [#78 connected acceptance update][update78], [#78 acceptance contract][issue78], [design/review procedure](design/gui-redesign.md), [keyboard caveats](keyboard-accessibility.md) | Records what is demonstrated and retains device/IME/full-screen/files/latency acceptance, not a human sign-off |

CI links are historical evidence, not a promise that expiring artifacts remain
available forever. Preserve the build SHA, run/job URL and relevant original
logs/screenshots alongside any subsequent acceptance report.

## Implementation versus acceptance matrix

“Implemented” below is bounded functionality, not full WindTerm parity.
Source references E5/E6 are pinned to the snapshot above.

| Area | Completed Iced functionality | Automated/native evidence | Still required by #78 / release qualification |
| --- | --- | --- | --- |
| Terminal-first shell and sessions | Session library, connection dialog, tabs, focus state, resizable splits, profile/tools access; no egui fallback | E2/E4/E5; production empty/dialog and connected/split states on Linux, separate fixture-only design preview | Human review of empty, connected, split, Files/transfer and connection-dialog states; readable controls, keyboard flow, preserved #55–#62 workflows on native devices |
| Terminal input and interaction modes | Focus mode, pane-local local navigation, guarded paste, privacy curtain, free-type local draft, remote cursor-key mode | E3–E6; draft/privacy non-delivery assertions, application/normal cursor encoding and source-first failure isolation tests | Native focus restoration, selection/copy/paste, mouse modes, shutdown and real full-screen application interaction; a VT alternate-buffer fixture is not Vim/tmux usability proof |
| Multi-pane and auxiliary shell | Independent PTYs, explicit sync arming/disarming, auxiliary SSH shell, four-pane limit | E3/E5 two authenticated PTYs and cleanup; E4/E6 connected Linux split and resize continuity | Device-level split/reorder/resize/selection and remote application cursor behavior. Windows ConPTY transport test uses printable probes; raw cursor bytes may become console events |
| Files and remote editing | Local/remote browser, right/bottom dock, transfer queue and safe remote-editor backends | E5 source plus E2 backend tests; E4/E6 proves live GUI SFTP handshake/dock, not a completed click-driven transfer | Connected native upload/download/SCP, integrity, overwrite/conflict/cancel/resume, remote-editor save and target identity while terminal stays usable |
| IME/CJK, typography and scaling | Committed Unicode routing, Iced IME integration, terminal fonts/appearance and cursor rendering | E5 source/regressions; parser/encoding tests are not candidate-window tests | Actual native composition/preedit/candidates/commit/cancel in terminal and editors; CJK/dead keys/emoji; native and 100/125/150/200% scales where supported; contrast, clipping, focus and accessibility review |
| GPU and responsiveness | WGPU/Metal build with tiny-skia fallback; bounded redraw/cache work and opt-in timing tools | E2/E4 native build/tests; E5; [trace procedure](design/gui-redesign.md#opt-in-slow-stage-tracing) | Affected physical Apple Silicon Mac: external click-to-visible-caret p50/p95 and text-entry latency, actual renderer selection, GPU/software comparison on the same workload; CPU-stage timings cannot prove screen latency |
| Release packaging | Native archives, dependency notices, architecture/metadata checks and clean extracted executable verification | E2 successful native package/download gates | Named-device operational qualification is still pending. Archives remain unsigned/unnotarized; signing/notarization, installers/app bundles and update policy are separate unresolved release work, not delivered by this docs PR |

Advanced SSH issue #66 is closed with bounded support, not universal runtime
coverage: macOS/Linux agent forwarding is verified; Windows agent and
Windows/macOS X11 remain explicitly native-unverified. See
[advanced SSH acceptance](ssh-advanced-acceptance.md). Closing that issue does
not establish connected GUI acceptance on those platforms.

## Required named-device acceptance record (not yet supplied)

Keep every explicit #78 requirement, including native Linux/Windows/macOS CI,
existing functionality and safety preservation, screenshots **and interaction
results**, terminal-centric design, profile access, obvious tabs/focus/splits,
and consistent SFTP/SCP/remote-editor presentation. The following is a report
schema, not a test result or an exemption for Linux:

| Required record | Pending evidence to attach |
| --- | --- |
| Identity | Tester, date, exact build SHA/archive checksum, machine model/CPU/GPU, OS version, display resolution/refresh/scale, actual renderer and input method |
| Connected environment | Disposable SSH/SFTP server details and OpenSSH version, session/pane count, workload and network conditions; no credentials or sensitive terminal output |
| Visual/design states | Production empty/start, connected terminal, split workspace, Files/transfer and connection dialog; native-scale review plus supported scale variations, explicit human findings |
| Interaction results | Keyboard/mouse focus, terminal/editor IME/CJK, full-screen applications, scrollback, selection/clipboard/paste guards, resize, split/reorder, real file transfers/remote editing, reconnect/shutdown; pass/fail and reproduction per case |
| Latency | On the affected Apple Silicon Mac, external click-to-visible-caret and text-input p50/p95, sample count/raw observations, measurement method and uncertainty, actual GPU/software renderer comparison; retain internal traces only as diagnostic evidence |
| Qualification decision | Named reviewer approval or failures with follow-ups; no unchecked criterion silently marked complete, no screenshots substituted for human usability judgment |

The [design budgets](design/gui-redesign.md#8-performance-architecture-and-provisional-budgets)
remain provisional targets, not measured results. This change performs no new
GUI, hardware, physical display or human acceptance testing. It reconciles
source and already-recorded CI; #78 remains the full acceptance gate.

## Proposed roadmap issue corrections (not applied here)

The live #1 body contains older checkpoints. A separate authorized issue edit
should check #64 and #66 as closed, replace the “#100 under CI” text with its
merged status, update the main/CI snapshot to E1/E2, and retain #78 as an
explicit open acceptance gate before moving beyond SSH polish. Preserve #66's
platform limitations and #78's original full scope; historical checkpoints
should be labeled historical rather than presented as current blockers.
Keep #1 open for the broader feature inventory, non-SSH protocols and release
limitations. No issue body, issue state, release or merge is changed here.

[pr121]: https://github.com/garyz-inspirum/inspirum-terminal/pull/121
[issue64]: https://github.com/garyz-inspirum/inspirum-terminal/issues/64
[issue66]: https://github.com/garyz-inspirum/inspirum-terminal/issues/66
[issue78]: https://github.com/garyz-inspirum/inspirum-terminal/issues/78
[roadmap]: https://github.com/garyz-inspirum/inspirum-terminal/issues/1
[close64]: https://github.com/garyz-inspirum/inspirum-terminal/issues/64#issuecomment-6095124180
[mainci]: https://github.com/garyz-inspirum/inspirum-terminal/actions/runs/38034556562
[prci]: https://github.com/garyz-inspirum/inspirum-terminal/actions/runs/38034023790
[icedci]: https://github.com/garyz-inspirum/inspirum-terminal/actions/runs/38034023898
[linuxjob]: https://github.com/garyz-inspirum/inspirum-terminal/actions/runs/38034023898/job/114160630821
[update78]: https://github.com/garyz-inspirum/inspirum-terminal/issues/78#issuecomment-6094497346
[launcher]: https://github.com/garyz-inspirum/inspirum-terminal/blob/8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018/src/main.rs#L171
[manifest]: https://github.com/garyz-inspirum/inspirum-terminal/blob/8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018/Cargo.toml
[icedsource]: https://github.com/garyz-inspirum/inspirum-terminal/blob/8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018/src/iced_app.rs
[dispatch]: https://github.com/garyz-inspirum/inspirum-terminal/blob/8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018/src/terminal_ux.rs#L55
[ptytest]: https://github.com/garyz-inspirum/inspirum-terminal/blob/8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018/tests/native_ssh_smoke.rs#L128
[fixture]: https://github.com/garyz-inspirum/inspirum-terminal/blob/8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018/scripts/test-iced-connected-gui.py
[workflow]: https://github.com/garyz-inspirum/inspirum-terminal/blob/8b6dbbe74b1c2188e4f0ccb4b33bfa7420aff018/.github/workflows/iced-design.yml
