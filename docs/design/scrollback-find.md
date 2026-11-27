# Scrollback Find (Ctrl+F) over very long output: design

Status: proposal, not implemented. Base: `origin/main` acce994.
Owner request: Ctrl+F finds anything that has scrolled through the pane (for example
`docker logs -f`), WindTerm-style, over millions of rows, and stays usable while output
streams.

Evidence tags: **[src]** read from source (file:line); **[calc]** computed from [src]
numbers; **[assumed]** an estimate that the stage that relies on it must measure.
Crate sources: alacritty_terminal 0.25.1 (Cargo.lock:44-45), vte 0.15.0
(Cargo.lock:2920-2921), read on gsb-dev under `~/.cargo/registry/src/*/`. Paths
`alacritty/…` are relative to `alacritty_terminal-0.25.1/src/`.

## 1. Current state

### 1.1 Output path: PTY bytes to viewport jump

1. The backend is an alacritty `Term` behind `Arc<FairMutex<…>>`, built with
   `scrolling_history: 10_000` (vendor/terminal-core/src/backend/mod.rs:330-333,347)
   and driven by alacritty's own `EventLoop` thread (mod.rs:349,352). That thread reads
   the PTY and parses while holding the term lock, up to `MAX_LOCKED_READ` = 65,535
   bytes per hold (alacritty/event_loop.rs:27,104-160), then sends `Event::Wakeup`
   (event_loop.rs:164-168). [src]
2. A per-pane subscription thread forwards events to the app sink (mod.rs:355-369).
   `pty_bridge` turns them into `Message::PtyEvent` (src/iced_app.rs:567-580). On
   `Wakeup` the app sets `display_dirty` and schedules a frame
   (iced_app.rs:7886-7921, 5041-5073). The frame worker sleeps 12 ms per request
   (iced_app.rs:515-520), so frames run at most about 83 per second. [src]
3. `Message::TerminalFrame` (iced_app.rs:7926-7948) calls `refresh_workspace_displays`
   (5080), which calls `display_snapshot`. That copies **only the viewport** cells and
   skips `WIDE_CHAR_SPACER` (mod.rs:472-564, comment 473-477). Then it calls
   `refresh_visible_history()` (7946). [src]
4. `refresh_visible_history` (src/iced_app/tools.rs:1198-1206) runs `history_refresh`
   **on every frame while the Find strip is open** or the History panel is visible.
   `history_refresh` (tools.rs:423-435) calls `terminal.history_lines()` and
   `HistoryState::update_snapshot`. [src]
5. `history_lines` (mod.rs:570-594) holds the term lock and walks every cell from
   `topmost_line` to `bottommost_line`. It builds one `String` per **grid row**, trims
   trailing spaces, and returns a `Vec<String>`. [src]
6. `update_snapshot` (src/history.rs:112-136) runs a KMP suffix/prefix overlap
   (`overlap_len`, 46-81), clones the retained lines (`to_vec`, 120), builds a
   `BTreeSet` of every id (133) and calls `refresh_search` (250-278). That rescans
   every line with `terminal_ux::find_text`, which allocates a lowercase copy of each
   line and of the query (src/terminal_ux.rs:78-111). [src]
7. Find keystrokes: `FindQuery` maps to `Action::HistoryQuery` (iced_app.rs:8400-8403,
   tools.rs:833-838), which calls `set_query`, which runs a full `refresh_search`, and
   then `history_jump`. Next/Prev map to `Action::HistoryNext` (8404-8417,
   tools.rs:839-852). These do not refresh the snapshot. [src]
8. `history_jump` (tools.rs:436-453) calls `scroll_to_history_index(line_number-1)`
   (mod.rs:599-613). That function puts the hit row at the **top** of the viewport, or
   at the bottom when the hit is on the live screen. It writes nothing to the PTY. [src]

### 1.2 Cost per event (N = retained rows, about 10,000 + screen rows; C = columns)

| When | Work | Complexity |
|---|---|---|
| Every frame while Find or History is open | `history_lines` under the term lock, plus snapshot diff, clone, `BTreeSet` and full rescan, all on the UI thread | O(N·C) cell reads + O(N·L) text + about 3N allocations |
| Every Find keystroke | `refresh_search`: N lowercase allocations + `str::find` | O(N·L) |
| Every copy (`selected_terminal_text`, iced_app.rs:5020-5032) | `sync()` **clones the whole grid** (mod.rs:456), then the clone is discarded: `selectable_content` locks the term again (441-445) | O(N·C) cells |
| Every frame | `display_snapshot`: viewport only | O(rows·C) |

Sizes:
- Alacritty `Cell` = `char` 4 + `fg: Color` 4 + `bg: Color` 4 + `Flags` u16 2 +
  `Option<Arc<CellExtra>>` 8 = 22 bytes, padded to **24 bytes**.
  Field sources: alacritty/term/cell.rs:134-140; `Color` is `Named(NamedColor≤u16)|Spec(Rgb 3×u8)|Indexed(u8)`
  (vte ansi.rs:1128-1132, 60-64, 1047). The crate test asserts
  `size_of::<Cell>() <= 24` (cell.rs:301-307). [src + calc]
- `Row<Cell>` = `Vec` (24) + `occ` (8) = 32 bytes (alacritty/grid/row.rs:17-25). Storage
  asserts 4 words (grid/storage.rs `swap` debug_assert). Every row is allocated at full
  width, `Row::new(columns)` (row.rs:37-49). [src]
- **One grid row at 150 columns = 150×24 + 32 = 3,632 bytes**, plus allocator overhead.
  [calc]

| Retained rows (150 cols) | Grid memory [calc] | Text only, 120 chars/line [calc] |
|---|---|---|
| 10,000 (today) | 36.3 MB | 1.2 MB |
| 50,000 | 182 MB | 6 MB |
| 100,000 | 363 MB | 12 MB |
| 1,000,000 | 3.63 GB | 120 MB |
| 10,000,000 | 36.3 GB | 1.2 GB |

- Lines longer than C use ceil(L/C) rows. At 80 columns a 120-character line costs 2
  rows. [calc]
- Per refresh, the `Vec<String>` snapshot costs about 24 + L + 16 bytes per line, and
  `HistoryState.lines` costs another 32 + L + 16. At L = 120 that is about 328 B/line:
  3.3 MB at 10k rows and 328 MB at 1M rows, rebuilt each frame. [calc]
- At 10k × 150 the frame refresh walks 1.5M cells (36 MB) under the parser lock.
  Estimated 5-10 ms per frame, which nearly fills the 12 ms frame budget and grows
  linearly: about 0.5-1 s per frame at 1M rows. [assumed; measure with the existing
  `SlowIcedScope` (iced_app.rs:141-160) around `history_refresh`]

### 1.3 Keys today

- **Plain Ctrl+F goes to the remote program.** The global key `listen_with` closure
  (iced_app.rs:403-493) has no Ctrl+F arm. The key falls through to
  `Keyboard(_) if Ignored => Message::Event` (490-492), then the KeyPressed arm
  (7962-8116), then `terminal_key_bytes`, which maps Ctrl+letter to `letter - 'a' + 1`.
  So Ctrl+F sends 0x06 (4047-4066), which is readline forward-char and vim/less
  page-forward. [src]
- Ctrl/Cmd+Shift+F, only when the event status is Ignored (459-470), produces
  `KeyChord::Find` (enum 2958-2967, message 2970-2981), then `FindOpen`, then
  `open_find_strip` (11405-11429). Edit > Find (menu.rs:76, label "Ctrl+Shift+F") maps
  to `FindToggle` (iced_app.rs:6194). `modifiers.command()` is Cmd on macOS. [src]
- Ownership: while `find_focused`, every other chord is dropped (5638-5643), every
  key is dropped and Escape closes (7990-7998), and `send_to_focused_terminal` returns
  early (4812-4815). The privacy lock closes the strip (5647-5653). Focus mode refuses
  to open it (11409-11412). The connected test arms `UNEXPECTED_FIND_PTY_INPUT`
  (scripts/test-iced-connected-gui.py:617-623, 6139-6160). [src]
- **F3 is not wired to Find.** It is forwarded as `\x1bOR` (iced_app.rs:4113), but
  docs/terminal-ux.md:53-55 says F3/Shift+F3 navigate results. The doc is wrong. [src]
- Alt-screen state is available as `TermMode::ALT_SCREEN` (alacritty/term/mod.rs:69).
  The app reads it through `last_content().terminal_mode`, refreshed each frame
  (mod.rs:486), via `focused_terminal_mode()` (iced_app.rs:4872-4879). [src]

### 1.4 Search semantics and limits

- Case-insensitive via `to_lowercase` (terminal_ux.rs:83,86). The query is trimmed
  (79, history.rs:252). Literal only: no regex and no case toggle. [src]
- **At most 500 hits** (history.rs:260,269). The counter shows `n/m` with m ≤ 500
  (`find_counter`, iced_app.rs:11505-11511, reserved width `123/456` 11501). [src]
- Coverage: the active grid only, at most 10,000 rows + screen. Older rows are recycled
  by the ring buffer (grid/mod.rs:262-280, increase_scroll_limit 175-180). In alt
  screen the active grid is the alt grid with 0 history (term/mod.rs:416,714-733), so
  Find sees only the vim/less screen. [src]
- `truncated_lines` (history.rs:43,108,115-117) only feeds History marks. It is not
  shown in the Find strip. [src]
- Text extraction defects (from reading the code; each needs a backend unit test to
  confirm):
  - `history_lines` does not skip `WIDE_CHAR_SPACER` cells, so CJK text reads as
    "東 京" and the query "東京" fails on real grid text. `display_snapshot` does skip
    them (mod.rs:501).
  - Zero-width combining marks in `CellExtra` are dropped, because only `indexed.c` is
    pushed (mod.rs:590).
  - Soft-wrapped rows (`WRAPLINE`, cell.rs flag) are not joined, so a match across a
    wrap point is never found.
  - The history.rs Unicode test (285-299) bypasses the grid, so it misses all three.
- **No hit highlighting in the terminal.** The canvas draws only URL and delimiter
  accents (iced_app.rs:3784-3800, 3955-3971). A hit is shown only by scrolling its row
  to the top. [src]
- Streaming behaviour:
  - While the user is scrolled up, alacritty adds new lines to `display_offset`
    (grid/mod.rs:266-269), so **the view stays put until history is full**. After that
    the offset is clamped at `max_scroll_limit` and content slides under the user. [src]
  - Any PTY write snaps to the bottom (mod.rs:393-396). Find never writes. [src]
  - Every frame recomputes `hits`. `selected_hit` is an **index** that is kept and
    clamped (history.rs:273-277), so once old hits roll off the top, the same `n`
    points at a later hit. `line_number` is a snapshot index, so a jump made after more
    output has arrived lands a few rows off. [src + calc]
  - `overlap_len` matches only a suffix of the old snapshot against a prefix of the new
    one. When an existing row changes (the cursor row, or blank screen rows filling in),
    there is no overlap. Example: `[A,B,""]` then `[A,B,X,""]` gives 0. All ids are then
    reissued, `truncated_lines` grows by N, and History marks are dropped. This happens
    on most output frames until the screen is full. [src, traced by hand; add a unit
    test]

## 2. Gap versus the request

| Need | Today |
|---|---|
| Plain Ctrl+F opens Find | Ctrl+F sends 0x06 to the PTY; only Ctrl+Shift+F opens Find |
| Everything that scrolled past, millions of rows | 10,000 rows; 500 hits; grid rows at 3.6 KB each make 1M rows cost 3.6 GB |
| Usable while streaming | O(N·C) copy and rescan on the UI thread on every frame; parser lock held during the copy; selection drifts by index |
| See where the hit is | No in-terminal highlight; count capped at 500; CJK, combining and wrapped matches missed |

## 3. Options for long scrollback

### A. Raise alacritty `scrolling_history`

- Allocation is lazy. `Storage::with_capacity` allocates only the visible lines
  (grid/storage.rs:67-75). History grows in steps of `max(n, MAX_CACHE_SIZE = 1000)`
  full-width rows (storage.rs:12, 126-136). It is not allocated up front, but it is
  never returned until history shrinks by more than 1,000 rows (107-114). [src]
- Cost: 3,632 B/row at 150 columns (table above). A column resize reflows every row,
  `take_all` plus re-push (grid/resize.rs:101-135). The app sends one resize per grid
  size change with no debounce (iced_app.rs:5154-5181). At 50k × 150 that is 7.5M cells
  (182 MB) moved per column step, estimated 50-150 ms each. [src + assumed]
- First remove the per-frame `history_lines` refresh (1.2) and the grid clone on copy
  (`sync()`, iced_app.rs:5027). At 50k rows they cost about 182 MB per copy and a
  multiple of 25-40 ms per frame. [calc/assumed]
- Search over the grid: alacritty's `RegexSearch` / `search_next` (term/search.rs:34,
  121-135) is smart-case (40-41) and walks cell by cell under the lock. Estimated
  50-150 M cells/s, so 50-150 ms for 7.5M cells, during which the parser is blocked.
  [assumed] Copy text out in bounded slices instead (Stage 1).
- 1M rows = 3.6 GB per pane; 10M rows = 36 GB. **Not viable beyond about 50-100k.**
  Risk is low at 50k and unacceptable at 1M+.

### B. Compact append-only line store fed by rows leaving the grid

- **No eviction hook exists.** `scroll_up` rotates the ring buffer and resets the
  recycled row (grid/mod.rs:252-300). Once history is full, `history_size` stays
  constant, so diffing it cannot count evictions. `Storage.zero` is private. [src]
- Exact capture needs a forked PTY loop. `event_loop.rs` is 485 lines, Apache-2.0, and
  alacritty_terminal is already a licensed dependency. The forked loop:
  - calls `parser.advance` in slices of at most S bytes (for example 4 KiB);
  - runs the grid with `scrolling_history = H + headroom`, where headroom ≥ S + screen
    rows. Each LF or wrap adds at most one row, and CSI S is clamped to the region
    height (term/mod.rs:770-780);
  - after each slice, copies rows older than H into the store, then trims with
    `grid_mut().update_history(H)` and restores `update_history(H + headroom)`
    (grid/mod.rs:154-161).
  Keep headroom ≤ 1,000 rows so trimming does not trigger `truncate` reallocation
  (storage.rs:107-114). [src + design] The alternative is a forwarding `Handler`
  wrapper, but that misses scrolls caused by line wrap, which are internal calls to
  `linefeed` and never pass through the wrapper.
- Store layout: **logical lines**, so the wrapping rows are joined. Rewrap is then
  recomputed for display at the current width, and resize never touches the store.
  - Chunks: `Chunk { first_abs: u64, text: Vec<u8> (UTF-8, '\n'-separated), ends: Vec<u32> }`
    of about 1 MiB each, in a `VecDeque`, plus `evicted_lines: u64`.
  - Absolute line number = stable id, so hits survive streaming.
  - Text only: about L + 4 + 1 B/line, which is **125 MB per 1M lines and 1.25 GB per
    10M** at L = 120. [calc]
  - Optional style runs `(start:u16,len:u16,fg,bg,flags)`, about 12 B per run and 0-4
    runs on typical log lines, add +0-48 B/line. [calc/assumed]
- Compression: **no compression crate is in Cargo.lock.** Searched for
  flate/miniz/zstd/lz4/snap/brotli; found none. Options:
  - `lz4_flex` (MIT, pure Rust). Assumed 2-3× on logs and more than 2 GB/s
    decompression.
  - `zstd` (BSD-3/MIT bindings plus C libzstd). Assumed 4-8×.
  Either is a new dependency and needs owner approval and third-party-notice updates.
  Searching compressed chunks needs a decompress-and-scan pass, so cost grows by the
  decompression time.
- Budget and spill:
  - In memory with a per-pane byte budget, dropping whole oldest chunks, with
    `dropped_lines` shown in the strip.
  - Spilling to disk breaks a written promise: docs/terminal-ux.md:47-49 says retained
    history is never written to disk; disk output needs explicit Start logging. If the
    owner approves spill:
    - Use an unnamed `tempfile::tempfile()`, which is unlinked on Linux, or the
      `0o700` dir / `0o600` file pattern from src/remote_edit.rs:266-290.
    - Read it with `memmap2` (in the tree, MIT/Apache).
    - Delete it on pane close and on drop.
    - Never include it in diagnostics (docs/PARITY.md:72).
  - The privacy lock only hides the tree and closes Find (iced_app.rs:5647-5653). It
    imposes no wipe rule. Search may keep running, but no result is rendered under the
    curtain.
- Search crates already in the tree:
  - `memchr` 2.8.3 (`memmem`, Unlicense/MIT). Assumed 2-5 GB/s for a case-sensitive
    literal.
  - `regex-automata` 0.4.18 with default features, including `meta` and
    `perf-literal`: aho-corasick and memchr are its locked deps (Cargo.lock:2255-2263;
    alacritty's dep has no feature override). MIT/Apache. Assumed 0.5-1.5 GB/s for a
    case-insensitive literal, and it adds regex at no extra cost.
  - Both are transitive today. Listing them in Cargo.toml adds no new crate.
  - `rayon` is not in the tree. Use `std::thread::scope` over chunks.
  [src for presence and licences; MB/s assumed]
- Display options:
  - (B1) Hits older than the grid open in a read-only results pane: line number, hit
    and ±2 context lines; Copy line.
  - (B2) The unified terminal view renders store rows when the viewport is above the
    grid top. This needs app-level virtual offset, scrollbar mapping by logical line,
    and selection/copy that spans store and grid, which alacritty's `Selection` cannot
    do. B2 is the WindTerm experience and is the riskiest piece.
- Risk: medium for capture (fork of upstream code to keep in sync on bumps), low for
  store and search, high for B2.

### C. Other

- C1: tee raw PTY bytes with ANSI stripped into the store. It is simple, but `\r`
  progress bars, cursor addressing and clear-screen produce text the user never saw.
  Rejected.
- C2: after B lands, **lower the grid back to 10k** and let the store hold everything
  older. Per-line cost drops from 3.6 KB to about 125 B (29× less) [calc]. This is the
  intended end state.

### Summary at L = 120, 150 columns, single thread

| Option | 1M lines | 10M lines | Literal search 1M / 10M [assumed rates] | Risk |
|---|---|---|---|---|
| A grid | 3.63 GB | 36.3 GB | grid walk 1.2-3.6 s / not feasible | low ≤ 50k, unusable ≥ 1M |
| Today's per-line lowercase + `find` on text | 120 MB text + 208 MB overhead | 1.2 GB + 2 GB | 0.3-0.6 s / 3-6 s (200-400 MB/s) | n/a |
| B text store, memmem (case-sensitive) | 125 MB | 1.25 GB | 25-60 ms / 0.25-0.6 s | medium |
| B text store, regex-automata (case-insensitive) | 125 MB | 1.25 GB | 80-240 ms / 0.8-2.4 s (÷ cores with scoped threads) | medium |
| B + lz4_flex (new dependency) | about 45 MB | about 450 MB | + decompression, about 1.3× | medium + dependency |

## 4. Recommendation: stages

### Stage 1a: Ctrl+F and stop the per-frame copy (this week, no new storage)

- **Key rule:**
  - Plain Ctrl+F (no Shift or Alt, status Ignored) opens Find unless the focused pane
    is in `TermMode::ALT_SCREEN`, in which case 0x06 goes to the program.
  - Ctrl/Cmd+Shift+F always opens Find.
  - On macOS, Cmd+F always opens Find, and Ctrl+F is still forwarded.

  The `listen_with` closure has no app state, so add an arm before the fall-through at
  iced_app.rs:490 that emits `Message::FindOrForward(event)`. In `update`:
  - if `focused_terminal_mode().contains(ALT_SCREEN)`, re-dispatch
    `Message::Event(event)` through the existing KeyPressed path, so remote-keys,
    local-navigation and free-type guards still apply;
  - otherwise call `open_find_strip()`.

  The same menu, dialog and `find_focused` guards as `KeyChord::Find` apply
  (5631-5643). Inside the Find input, Ctrl+F is captured, so the arm does not fire,
  which is correct. Update the menu label (menu.rs:76) to "Ctrl+F" and update
  docs/terminal-ux.md. Fix the F3 claim there: either drop it, or wire F3/Shift+F3
  **only while the strip is open**.
- Remove `history_refresh` from `TerminalFrame` for Find. Keep it for the History
  panel, refreshing on panel open, on Mark and at most every 500 ms while the panel is
  visible (tools.rs:1198-1206). Replace `sync()` in `selected_terminal_text`
  (iced_app.rs:5027) with `selectable_content()` alone.
- Fix text extraction in a new `Terminal::history_text_range(start, count)`:
  - skip `WIDE_CHAR_SPACER` and `LEADING_WIDE_CHAR_SPACER`;
  - append zero-width characters;
  - join `WRAPLINE` rows into one logical line, recording the row index where each
    logical line starts.
- Default `scrolling_history`: **50,000**, which is 182 MB per pane at 150 columns when
  full and lazily allocated [calc]. 100k (363 MB per pane, about 2× resize cost) is not
  justified until Stage 2 lets the grid shrink again. Make it a backend setting field,
  not a constant.
- Tests:
  - unit: chord routing for `FindOrForward`, covering main screen, alt screen,
    `find_focused`, open menu and privacy lock;
  - unit: extraction of CJK, combining characters and wrapped lines through a real
    `Term` fed with bytes, not through `HistoryState`;
  - unit: no `history_lines` call per frame. Count it through a test hook, or assert
    that `TerminalFrame` with the strip open leaves `HistoryState` untouched.
- Connected step:
  - main screen: arm `UNEXPECTED_FIND_PTY_INPUT`, press Ctrl+F, and require the strip
    to paint and no byte to reach the fixture;
  - after `screen-on`, press Ctrl+F and require a new fixture marker `CTRL_F_BYTE`
    (raw-chunk `\x06` detection beside the cursor-key scan in both shell handlers,
    test-iced-connected-gui.py:150-171 and 621-656), and the strip must not open;
  - `screen-off`, then repeat the main-screen case.

### Stage 1b: background search, full count, highlight, stable while streaming (this week)

- A per-pane `FindJob` runs on a worker thread. It holds the term `Arc` and reads text
  in slices of about 2,000 rows per lock hold (keeping the parser lock-out under about
  1 ms) [assumed]. It matches with `regex-automata` meta in literal mode,
  case-insensitive (current behaviour).
  - Each query change bumps an `AtomicU64` generation, which cancels the stale job, and
    results stream back as `Message::FindProgress { pane, gen, hits, scanned, total }`.
  - Typing never touches history on the UI thread.
- Hit model: `Vec<Hit { row: u64, col: u32, len: u32 }>` (16 B), stored up to 1,000,000
  hits (16 MB), with the counter showing `m+` beyond that. Rows are absolute:
  `abs = base + index_from_top`.
  - While the grid is not full, `index_from_top` is stable by construction.
  - Once it is full, the job re-anchors each frame: it hashes the last 64 scanned rows,
    locates that hash in a bounded window to get the eviction shift, and rescans in
    full on a miss.
  - Incremental: on each output frame, scan only rows newer than `last_scanned`.
- Selection follows the hit identity `(row, col)`, not an index. Output arriving never
  moves the viewport. Only Enter, Next, Prev or a typed query jumps. The strip shows
  `n of m` plus "older lines dropped" when the selected hit is evicted.
- Highlight: add `highlights: Vec<(row, col_start, col_end, current)>` to
  `DisplaySnapshot`, built by `display_snapshot` from the visible slice of the hit list
  (O(visible hits)).
  - The canvas fills behind the glyphs before drawing text, accent for the current hit
    and muted for the others, and adds the highlights to the per-row hash
    (iced_app.rs:3784-3800).
  - Highlights are not drawn in alt screen and not under the privacy curtain.
- Tests:
  - unit: a 50k-row `Term` with the needle on row 0 finds it and the count is exact;
    the generation cancels a stale job; anchor re-location after N evictions; the
    selected hit stays on the same text while 1,000 lines stream; a `HistoryState`
    regression keeps the History panel and its tests (history.rs:285-397) unchanged.
  - connected: add `burst:<N>` to both fixture shell handlers. It writes
    `BURST-%07d` lines, with line 1 containing `FIRSTNEEDLE`, in 64 KiB writes, then
    logs `BURST_SENT:<N>`.
    - With N = 45,000: Ctrl+F, type FIRSTNEEDLE, and the counter OCR reads `1 of 1`
      within 1 s.
    - Enter puts `BURST-0000001` on screen (OCR).
    - With N = 60,000, the counter reads `0 of 0`, the dropped notice appears and the
      app stays responsive: an `echo:` probe after Escape is answered.
    - RSS check: `/proc/<pid>/status` VmRSS grows by ≤ 250 MB for 45k lines.
- Must not regress:
  - `find_focused` guard sites (4812-4815, 5638-5643, 7990-7998, 8400-8430) and the
    focus probe;
  - Escape closes with no PTY byte;
  - no PTY writes from Find at all;
  - the existing connected FIND ownership probes (`FIND_P1`…`P3`,
    test-iced-connected-gui.py:6351-6464, 8564-8599);
  - the History tool;
  - privacy-lock and focus-mode closing.

### Stage 2: exact long history store and a results pane

- Fork alacritty `event_loop.rs` into `vendor/terminal-core/src/backend/pty_loop.rs`,
  keeping the upstream licence header, and implement the sliced-advance + drain capture
  (3.B).
- Add `LineStore` (3.B layout), owned by the backend and behind its own lock. The
  default budget is 256 MB per pane, which is about 2M lines at 120 characters [calc];
  past it, whole oldest chunks are dropped and counted. After this stage, set the grid
  history back to 10k (C2).
- The search worker scans store chunks (whole-chunk `memmem` or regex, then map to a
  line by binary search on `ends`), then the grid. Results arrive newest first, so
  recent hits show immediately.
- Add a Match case toggle (smart-case like alacritty) and a Regex toggle, both cheap
  with regex-automata.
- Hits older than the grid open a read-only results pane (B1).
- Tests:
  - unit: capture is exact under bursts with CSI S, wraps and alt-screen toggles (no
    capture in alt screen); budget eviction; logical-line rewrap.
  - bench (`#[ignore]`, release, run on gsb-dev): 1M × 120 B literal search < 250 ms.
  - connected: `burst:1000000` gives `1 of 1` for FIRSTNEEDLE within 2 s of the last
    keystroke, the results pane shows `BURST-0000001`, VmRSS grows by < 400 MB, and an
    `echo:` probe is answered during the search.

### Stage 3: unified scrollback view (WindTerm parity)

- The canvas renders store lines above the grid top, rewrapped at the current width.
  The scrollbar and mouse wheel map over logical lines. Selection and copy are
  app-level when either end lies in the store.
- Jumping to an old hit scrolls the terminal itself, and the results pane becomes
  optional.
- Connected test: Enter on hit 1 of 1M puts `BURST-0000001` in the terminal canvas by
  OCR, a wheel-down by one notch shows `BURST-0000004`, and a drag-copy across the
  boundary is pasted back exactly, using the selection-probe pattern.

### Stage 4: optional, needs approval

- Style runs for colours in old lines.
- Compression (new dependency).
- Owner-only spill to disk (3.B rules).

## 5. Open questions (with defaults so work is not blocked)

1. **On the main screen, Ctrl+F no longer reaches readline (forward-char); in alt-screen
   apps (vim, less, tmux) it still does.** Is that acceptable?
   Default: yes, using the 1a rule, and a later setting to turn plain Ctrl+F off.
2. **Memory per pane, and may history ever go to disk?**
   Default: in memory only, 256 MB per pane (about 2M lines of 120 characters), oldest
   chunks dropped with a visible count. Disk is never used, which keeps the
   terminal-ux.md promise.
3. **May we add a compression crate (`lz4_flex`, MIT) to hold about 3× more lines in the
   same budget?**
   Default: no new dependency through Stage 3. Revisit with Stage 2 measurements.
