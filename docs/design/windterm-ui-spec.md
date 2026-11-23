# WindTerm-exact UI specification

Status: **authoritative target** for the Iced production frontend. It supersedes
sections 4 (Layout) and 5 (Visual system) of `gui-redesign.md` and the
"every clickable control has a resting fill and outline" rule recorded there.
This is a design target, not a current-control guide. After PR #145 the
session toolbar and breadcrumb field described below are absent from the Iced
UI; pane headers have no mode/sync action buttons. Current controls are **Edit → Copy/Paste**,
**View → Files/Shell**, **Mode → Free Type Mode/Remote Keys/Sync Input...**,
and **Window → Workspace** for per-pane sync target checkboxes and arming.
**Mode → Sync Input...** opens Workspace with fewer than two selected targets;
otherwise it toggles arm/disarm. When armed, the status badge counts selected
targets; only a selected focused source can mirror to them. An unselected
focused source receives input only to itself. Tunnels opens from the top-right
menu-bar button or **Session → Tunnels**.

Reading this document: numbered sections describe the **target**. Paragraphs
headed **Current build** describe what the code does today (branch state after
PR #146 plus the #140 Light readability commits `8439728` and `69b70ba`); where
the two differ, the target is unchanged and still open. Section 11 is a
historical PR #141 snapshot with later corrections marked inline.

The owner's instruction: the previous UI is hard to read and not intuitive;
**copy the WindTerm UI and mimic it exactly**. Where this document is silent,
do what WindTerm does. Never invent a control for a feature that does not
exist in this application, and never drop an existing feature: every existing
action must stay reachable (menu, toolbar, context menu or dock panel).

Reference images (not committed; on the build host under
`/mnt/data/atlas/windterm-ref/`): `WindTerm.png` (dark, 2560x1520 @2x),
`WindTerm_DigeWhite_Theme.png` (light, @2x), `SplitView.png` (@2x),
`gary-filer.png` (the owner's own WindTerm: light chrome, dark terminal,
all panes as tabs in one bottom dock, @2x). All pixel values below are
**logical pixels** (reference pixels / 2).

Only layout, proportions, colours and interaction are reproduced. Do not copy
WindTerm artwork (logo, bitmap icons) or source code; icons are original
vector drawings of the same generic shapes.

## 1. Window structure (top to bottom)

```
Menu bar                                   32 px
Main row (fills)
  [Left dock]  | Centre workspace | [Right dock]
     214 px        fills             214 px      (docks resizable, hidden when empty)
Bottom dock (when it has panels)          ~ 30% of height, resizable, min 110
Status bar                                 22 px
```

Centre workspace, top to bottom:

```
Session tab strip        26 px
Session toolbar          34 px   (pane buttons + breadcrumb field + overflow)
Terminal split area      fills
```

Separators between all regions are 1 px lines in the `border` colour. No
rounded cards, no drop shadows, no gaps between regions: surfaces touch.
Corner radius is 0 everywhere except: text buttons 2 px, progress bar 1 px.

## 2. Themes

Two chrome themes, switchable at runtime from `View > Theme` and persisted
with the workspace preferences. The terminal palette stays the independent
`TerminalAppearance` setting.

Default = **Light chrome + dark terminal** (what the owner runs).

| Token | Light (DigeWhite) | Dark (DigeBlack) |
| --- | --- | --- |
| `chrome` (menu bar, toolbars, dock headers, tab strip background) | `#ECECEC` | `#1E1E1E` |
| `dock` (dock panel body) | `#F0F0F0` | `#191919` |
| `list` (tables, trees, editors, input interiors) | `#FFFFFF` | `#1E1E1E` |
| `header` (table column header) | `#F2F2F2` | `#222222` |
| `tab_active` | `#F5F5F5` | `#1E1E1E` |
| `tab_inactive` | `#DDDDDD` | `#333333` |
| `raised` (breadcrumb field in dark, hover fill) | `#FFFFFF` / hover `#E4E4E4` | `#333333` / hover `#2A2A2A` |
| `selected_row` | `#D8D8D8` | `#262626` |
| `border` | `#BFBFBF` | `#444444` |
| `text` | `#151515` | `#BFBFBF` |
| `text_dock` | `#333333` | `#BFBFBF` |
| `muted` (placeholders, disabled, timestamps) | `#929292` | `#808080` |
| `accent` (active-tab top line, focus, progress fill) | `#1E90FF` | `#2196F3` |
| `magenta` | `#FF1493` | `#FF0099` |
| `success` | `#339333` | `#A6E22E` |
| `danger` | `#D32F2F` | `#FF5C5C` |
| `status_bg` / `status_text` | `#007ACC` / `#FFFFFF` | `#007ACC` / `#FFFFFF` |

Current build: `src/iced_app/theme.rs` adds tokens not in this table and some
values differ from it (for example Light `muted`). The state cues below use
`selection_indicator` (Light `#0066B3`, Dark `#70B8FF`), `selected_row` and
`control_outline` (Light `#767676`, Dark `#8A8A8A`). Source and its contrast
tests are authoritative for exact current values; this table remains the
WindTerm reference.

Panel marker colours (11x11 px square before each dock title, both themes):
Sender `#A16BE8`, Session `#009FFF`, Outline `#91DDF2`, Transfer `#00CCCC`,
Filer `#555BA9`, Explorer `#7CB342`, Shell `#FF0099`, Tunnels `#F6A623`.

## 3. Typography and metrics

- All chrome text is **monospace** (`Font::MONOSPACE`), 13 px. Status bar
  12 px. No bold except `Remote Mode`/`Local Mode` in the status bar.
  Placeholders (`Filter`) are muted and italic where the font allows.
- Row height in lists, trees and tables: 23 px. Table header: 20 px.
- Dock title strip / dock tab strip: 26 px. Session tabs: 26 px.
- Icon size 14-16 px, drawn with 1.25 px strokes in `text` colour (muted when
  disabled). Icon buttons are flat (no fill, no outline) with a `raised`-hover
  square of 22x22 px. Text buttons (dialogs, Find bar) keep a 1 px `border`
  outline and `list` fill, 2 px radius, 24 px high.
- The global A-/A+ scale keeps working and scales all of the above.

## 4. Menu bar

Labels, in order, exactly: `Session`, `Edit`, `Search`, `Selection`, `Goto`,
`View`, `Mode`, `Window`, `Help`. (WindTerm appends `(1)`-style mnemonics;
omit them unless Alt+digit is actually wired.) A menu is only shown if it has
at least one working item. Clicking a title opens a flat drop-down (1 px
border, `list` background, 24 px rows, shortcut text right-aligned and muted,
separators as 1 px lines); clicking elsewhere or Escape closes it; hovering
another title while one is open switches menus.

Current build (Light): the open menu title (including the top-right Tunnels
title) has a `selected_row` fill plus a 2 px `selection_indicator` underline;
the open state wins over hover. Drop-down row hover uses `selected_row`.
**View → Theme → Light / Dark** show a marker in the shortcut column instead
of shortcut text: filled dot = current theme, hollow circle = available.
Labels and commands are unchanged. Dark keeps its previous open/hover fill
(`hover`, no underline); the theme marker is drawn in both themes. Menu items
have no disabled gating; unavailable commands still report through the status
bar.

Mapping of existing actions (every current button must appear here or in a
toolbar/context menu):

- **Session**: New Session (Ctrl+N), Import Profiles, Edit Session, Duplicate
  tab if available, Reconnect, Close Session, Tunnels, Lock Screen
  (Ctrl+Shift+L), Exit.
- **Edit**: Copy, Paste, Paste Dialog (guarded paste), Command Palette /
  Snippets.
- **Search**: only if a terminal search feature exists; otherwise hidden.
- **Selection** / **Goto**: only items backed by real features (PgUp/PgDn,
  Home/End of scrollback, next/previous tab); otherwise hidden.
- **View**: Theme > Light | Dark, Zoom In (A+), Zoom Out (A-), Focus Mode
  (Alt+Enter), and one checkable item per dock panel (show/hide).
- **Mode**: Remote Mode / Local Mode (local navigation), Free Type Mode,
  Remote Keys, Sync Input (select targets / disarm).
- **Window**: Split Right, Split Down, Close Pane, Next Tab, Previous Tab,
  Layout > `Bottom tabs` | `WindTerm default`, Tools panels (Profiles,
  Workspace, History, Appearance, SSH, SCP, Diagnostics, Snippets).
- **Help**: About.

## 5. Dock system

Panels: **Session**, **Filer** (remote files of the active session),
**Explorer** (local files), **Transfer**, **Shell** (auxiliary shell),
**Sender** (command/snippet sender), **Outline** (only if a real per-session
command list exists; otherwise omit the panel entirely), **Tunnels** (optional;
may stay a dialog).

Three dock areas: Left, Right, Bottom. Each panel lives in exactly one area or
is hidden. Persisted with the workspace.

- An area holding **one** panel shows a title strip: marker square, title,
  spacer, gear icon (panel menu), close `x`.
- An area holding **several** panels shows them as **tabs** in that strip
  (as in `gary-filer.png`): each tab = 1 px `border` outline, marker square,
  title; active tab `tab_active` fill, inactive `tab_inactive`; tab width fits
  the label with 12 px padding, min 86 px. Right end of the strip: collapse
  chevron (bottom dock only), gear, close `x` (closes the active panel).
- Gear menu: `Move to Left`, `Move to Right`, `Move to Bottom`, `Close`.
- Layout presets (Window > Layout):
  - **Bottom tabs** (default; the owner's arrangement): bottom dock tabs in
    this order: Sender, Session, Outline, Transfer, Filer, Explorer, Shell.
    No side docks. Active tab is Session when no session is open, Filer after
    connecting only if the user had opened it.
  - **WindTerm default**: Left = Filer (+Explorer as second tab); Right =
    Session above Outline; Bottom = Sender, Transfer, Shell.
- Dock splitters are 1 px lines with a 5 px invisible grab zone.

Current build (dock tab state): the active dock tab has a `tab_active` fill,
`text` label and a 2 px `selection_indicator` top line; inactive tabs keep the
`text_dock` label (never `muted`, which marks disabled controls). Strip icons
use `text`. These are palette-driven, so Dark draws the same cues with its own
tokens; only Light was the readability review target.

Current build: the dock panels are Sender, Session, Transfer, Filer and
Explorer. Outline and Shell are not available as dock panels (the auxiliary
shell opens as a split pane) and Tunnels remains a dialog. The presets are
`Window > Layout: Bottom tabs` and `Window > Layout: WindTerm default`; the
missing panels are skipped. Layout and theme are saved in `ui-state.json`
next to the profiles file.

### 5.1 Session panel

`Filter` field (no box: text over a 1 px underline, 30 px high), then a tree:
group rows (disclosure triangle + folder icon + group name) and session rows
indented 20 px (marker: filled circle in a per-session colour for SSH; name).
Single click selects, double click or Enter opens. Selected row uses
`selected_row` full width. Right-click / gear: Open, Edit, New Session,
Import. Favourites and tags stay visible as muted suffix text.

Current build (PR #143 and #140): a single left click selects a session row;
a second left click on the same row within 400 ms opens it. Enter does not
open a selected row. Group rows collapse and expand (a non-empty `Filter`
shows all matches regardless). Right-click selects the row without starting a
double-click and opens a menu with Open, Edit, New Session, Import (no
separator). The selected row has a `selected_row` fill, a 2 px
`selection_indicator` left bar and `text` name colour, so it differs from
rows that are merely open/connected. The gear menu holds only the dock
placement items, not the session items.

### 5.2 Filer and Explorer panels

Identical widget; Filer lists the remote directory of the active session,
Explorer the local file system.

- Path bar (34 px): path as plain editable text over a 1 px underline, then
  icon buttons right-aligned: clear-path, up, history drop-down triangle (may
  be omitted if no history), refresh, vertical three-dot menu.
- Table header (20 px, `header` fill, 1 px vertical dividers): `Name`,
  `Date Modified`, `Type`, `Size`, `Permissions`, `Owner`, `Group`. Clicking a
  header sorts; the sorted column shows a small triangle at its right edge.
  Default sort: directories first, then name ascending.
- Rows (23 px, `list` background, no grid lines): icon (folder = cyan
  `#12BFE5` in light / pale yellow `#F6D56F` in dark; file = outlined page)
  + name; `YYYY/MM/DD HH:MM:SS`; `Directory` or `<EXT> File`; size
  right-aligned (`18 Bytes`, `8.0 KB`, blank for directories); permission
  string; owner; group. Column widths (px): 290, 148, 132, 76, 106, 62, fill.
  Narrow docks simply clip columns on the right with a horizontal scrollbar,
  like WindTerm.
- Footer (16 px): `<n> items <total size>`.
- Single click selects, double click opens directory (or remote-edits a file
  in Filer). Context menu and three-dot menu: Open, Download / Upload (to the
  other side's current directory), Edit (remote), New Folder, Rename, Delete,
  Refresh. Destructive actions keep their confirmation dialogs.

Current build: the Up control shows an arrow plus the word `Up`. There is no
history drop-down or three-dot menu. A single left click on a **directory**
row opens it immediately; a single left click on a file selects it (no
double-click gesture). Right-click (PR #143) selects the row, directories
included, and opens a context menu: Open (directories), Download and Edit
(remote files), Upload (local files), New Folder, Rename, Delete, Refresh.
The selected row has a `selected_row` fill and a square 1 px
`selection_indicator` outline drawn inside the 23 px row (hover never draws
the outline). Column header captions and the sort mark use `text`; the footer
summary uses `text_dock`.

Below each table the build keeps an action-button row (not in the WindTerm
reference). Since #140 the selection-dependent buttons are disabled when
they cannot act: Filer Download and Edit need a selected remote file (not a
directory); Filer Rename/Delete and Explorer Upload/Rename/Delete need any
selected item. New folder is always enabled. Disabled buttons use the `dock`
fill, `border` outline and `muted` text; enabled buttons use `list` fill,
`control_outline` and `text`. Request handlers still report "Select a ..."
in the status bar for keyboard/context paths. The disabled look itself is not
new: `quiet` (shared by these and other text buttons, including dialogs)
already styles any button without `on_press` as `Status::Disabled`. What #140
adds is conditional `on_press_maybe` gating for these selection-dependent
file actions. Menu items have no disabled gating.

### 5.3 Transfer panel

One entry per job: file icon, `<name> - <bytes> downloaded|uploaded`, right
aligned `<destination dir> DOWNLOAD|UPLOAD [YYYY-MM-DD HH:MM:SS]`, below it a
full-width 13 px progress bar (1 px accent outline, accent fill, centred
`NN%`). Cancel / Retry / Resume as small icon or text buttons at the right.
Empty state: nothing (blank panel), as in WindTerm.

### 5.4 Shell, Sender, Outline

- Shell: the existing auxiliary SSH shell terminal, filling the panel.
- Sender: the existing command/snippet composer (editor with line numbers,
  Send button, target choice Current / sync targets). No count/interval
  controls unless implemented.
- Outline: `Filter` + list of commands with blue bullet dots, only if backed
  by real data.

Current build: Sender is a dock panel. Shell opens as a split pane in the
session tab, not as a dock panel. Outline is not available.

## 6. Centre workspace

### 6.1 Session tab strip

Flat rectangular tabs, 26 px high, no gaps, no radius: marker (per-session
colour dot), session name, close `x` (visible on the active tab and on hover).
Active tab: `tab_active` fill and a 2 px `accent` line along its top edge.
Inactive: `tab_inactive`. Right end of the strip: `+` (new session) and a
three-line menu icon (tab list). Connection state is shown by the marker
(filled = connected, hollow = connecting, `danger` = failed/closed) and a
tooltip, not by words such as LIVE SSH / CONNECTED / PTY / FOCUSED.

Current build (Light, #140): the active session tab uses the `list` fill
(`#FFFFFF`) against `tab_inactive` (`#DDDDDD`) and a **3 px**
`selection_indicator` (`#0066B3`) top rule, replacing the 2 px `accent`
line, which measured about 3.0:1 and read as a hairline. This is a deliberate
readability deviation from the 2 px WindTerm line. Dark is unchanged:
`tab_active` fill, 2 px `accent` line. WindTerm-style per-session tab
colours are not built; a Window → Workspace tab label adds only a coloured
marker glyph before the tab title.

### 6.2 Session toolbar

Left group of flat icon buttons: split right, split down, close pane, shell.
Then the breadcrumb field filling the width (`raised` fill, 1 px border, 23 px
high): info icon, separator, `ssh > <group> > <session name>` with small
triangles. Right of it: three-dot overflow menu holding the remaining pane
actions (Copy, Paste, Sync input, Free type, Remote keys, Reconnect, Focus
mode). Mode toggles that are **on** are additionally shown as small accent
text chips in the status bar, never as large toolbar buttons.

### 6.3 Terminal panes

Panes touch each other, separated by 1 px splitters. The focused pane has no
thick outline; focus is shown by its header/tab accent line. A pane header is
only drawn when the tab has more than one pane (name + close `x`, 22 px).
The terminal canvas itself (text grid, colours, cursor) is unchanged in this
pass. WindTerm's timestamp / line-number / fold gutter is a later phase.

Current build (Light, #140): a focused split pane's header gets a pale blue
tint (`#D9E8F5`) and its 2 px rule strip switches to `selection_indicator`;
unfocused headers keep `header` and the strip is drawn in the same `header`
colour (no visible rule). The strip stays 2 px, so the terminal grid height
does not change on focus. Dark keeps the `header` fill with a 2 px `accent`
rule. There is still no pane outline.

### 6.4 Empty workspace

No welcome card. Blank `list`-coloured area with one muted centred line:
`Double-click a session to connect, or Session > New Session (Ctrl+N)`.

## 7. Status bar

22 px, `status_bg`. Left: status message (`Ready` when idle, otherwise the
existing status text). Right, in order, 16 px apart: active mode
(`Remote Mode` / `Local Mode`, bold), 1 px separator, terminal size
`<cols>x<rows>` of the focused pane, `ssh`, running-transfer and
running-tunnel counts when non-zero, local date-time `YYYY/MM/DD HH:MM`.
No implementation words (`ICED`, "event bridge").

Current build: unchanged by #140. Both themes keep white text on `#007ACC`
as in the WindTerm reference; a test pins these values and 4.5:1 contrast.

## 8. Dialogs

Connection editor, confirmations, paste guard, tunnels, tools panels, about:
same content, restyled to the theme: `dock` background, 1 px border, title
strip like a dock title, flat inputs with 1 px border, text buttons per
section 3, primary button = accent fill with white text. Privacy curtain and
focus mode keep their behaviour.

### 8.1 Tunnel management (WindTerm reference)

The supplied WindTerm screenshots distinguish **daily operation** from **editing**:

- The top-right `Tunnels` control opens a compact, scrollable menu headed
  `Tunnels`, with a master state at the top, one row per saved forward (name and
  type marker at left; state switch at right), and a separated `Tunnel
  Settings...` command at the bottom. A switch must drive that one forward,
  not silently start or stop every forward belonging to the same SSH profile.
  An independently switchable process must be backed by the existing validated
  OpenSSH launcher, acknowledgement for non-loopback binds, and real status.
  Until that runtime is ready, do not show a decorative switch.
- `Tunnel Settings...` opens the editor: left scrollable saved-tunnel list with
  a neutral selected row and compact add/remove actions; right aligned
  `Name`, `Type`, `Session`, `Listen Port`, `Destination Host`,
  `Destination Port`, `Status` fields, then safety/startup options only when
  actually implemented. In Dynamic mode, destination is visibly inapplicable
  rather than a required input. Selecting a tunnel reveals its actual owner
  profile; save/delete retain current validation and persistence. The settings
  dialog should remain usable with a long list and at minimum window size.
- Do not hard-code names from the supplied screenshots; these are the user's
  personal saved entries, not fixtures or app defaults. Do not copy WindTerm
  icons or source. Match the two-surface workflow, row alignment, density,
  neutral selection, and clear on/off states.
- **Light theme across the whole interface:** primary labels and entered
  values are dark (`text` / `text_dock`), supporting explanations and
  placeholders are a distinct readable gray (`muted`). Text smaller than
  18 px targets 4.5:1 contrast on its real background; control outlines
  needed to identify an input target 3:1. Preserve equally readable dark
  theme. Error/status text is not styled as an ordinary explanation.

Acceptance: from the menu, toggle one of two saved forwards in the same
profile and prove only its listener opens/closes while the other stays up;
open settings, select/add/edit/delete a tunnel, save/reopen and verify the
saved values; confirm a dynamic forward does not ask for a fixed destination;
inspect light/dark screenshots of the menu and expanded editor, including
minimum window size. A master toggle must have defined, tested behavior rather
than merely mirroring a color.

## 9. Things that must keep working

Keyboard shortcuts, IME wrapper, guarded paste, sync-input safety warnings,
privacy lock, focus mode, split/resize/reorder of panes, files dock
transfers/overwrite/delete confirmations, remote editor, tunnels with
acknowledgement gates, tools panels, profile import, scale A-/A+, minimum
window 960x640 with usable file rows, and the Linux connected GUI acceptance
script (updated to the new labels and layout, not weakened).

## 10. Not in this pass (tracked separately)

Terminal gutter (timestamps, line numbers, folding), per-split-group tab
strips, quick bar, Find strip, drag-and-drop docking, session tab colours.

Later: the Find strip was implemented in PR #144; the other items remain
unbuilt (see 11.1).

## 11. Implementation status

The table records the issue #140 / PR #141 implementation snapshot; it is not
a current-control inventory or an acceptance claim. In particular, the
session toolbar and breadcrumb recorded below were removed by PR #145. For
current menu locations use the note at the top of this document and
[`workspace.md`](../workspace.md).
Rows whose PR #141 note has since been contradicted by merged code carry a
bracketed **Later** correction; the **Current build** paragraphs in the
sections above are the fuller description.

| Spec section | Status | Note |
| --- | --- | --- |
| 1. Window structure | Changed after snapshot | Menu bar, session tab strip, dock areas and status bar remain; no session toolbar after PR #145. |
| 2. Themes | Implemented | Light (default) and Dark, View > Theme, `INSPIRUM_THEME` override; persisted in `ui-state.json`. |
| 3. Typography and metrics | Partly | Not measured against every value in the table. |
| 4. Menu bar | Implemented | Session, Edit, View, Mode, Window, Help with drop-downs (items in `src/iced_app/menu.rs`). [Later, #140: Light open-title fill + underline; filled/hollow Theme marker.] |
| 5. Dock system | Partly | Left/Right/Bottom areas, tab strips, gear menu (Move to Left/Right/Bottom, Close), both presets, drag-resizable splitters, persistence. No drag-and-drop docking. |
| 5.1 Session panel | Partly | Single click opens; no collapsible groups, no context menu. [Later, PR #143: single click selects, double click opens, collapsible groups, right-click menu. #140: selected-row left bar. Enter-to-open not built.] |
| 5.2 Filer and Explorer | Partly | WindTerm columns (Name, Date Modified, Type, Size, Permissions, Owner, Group). No context menus. [Later, PR #143: right-click menus. #140: outlined selected row, selection-gated action buttons. Directories still open on single click; no three-dot or history menu.] |
| 5.3 Transfer panel | Implemented | Transfer queue as a dock panel. |
| 5.4 Shell, Sender, Outline | Partly | Sender is a dock panel. Shell opens as a split pane, not a dock panel. Outline not implemented. |
| 6.1 Session tab strip | Partly | Tab strip present; no per-session tab colours. [Later, #140: Light active tab white + 3 px rule.] |
| 6.2 Session toolbar | Target only; removed from current build | Toolbar with breadcrumb describes the original design, not PR #145's menu-driven UI. |
| 6.3 Terminal panes | Partly | Pane headers only for split tabs. No timestamp/line-number/fold gutter. [Later, #140: Light focused header tint.] |
| 6.4 Empty workspace | Implemented | Muted centred hint line. |
| 7. Status bar | Implemented | Mode, `<cols>x<rows>`, `ssh`, counts, date-time. |
| 8. Dialogs | Partly | Restyling not reviewed dialog by dialog. |
| 9. Things that must keep working | Not yet verified | GUI acceptance on the new layout is pending; a local container harness is in `scripts/local-gui/`. |
| 10. Not in this pass | Not yet | Gutter, per-split-group tab strips, quick bar, Find strip, drag-and-drop docking, session tab colours. [Later, PR #144: Find strip implemented. Window → Workspace tab labels add a coloured marker, not WindTerm tab colours.] |

### 11.1 #140 Light readability pass (open)

Commits `69b70ba` and `8439728` (on top of merged PR #146) add Light state
cues for: open menu title, Theme marker, menu hover, session tab, focused pane
header, dock tab, selected session/file row, and selection-gated file action
buttons. Contrast targets (4.5:1 text, 3:1 indicators/outlines) are enforced
by unit tests in `src/iced_app/theme.rs` and `src/iced_app.rs`; that is not a
screenshot or user acceptance. #140 is not complete. Dark receives only the
palette-driven dock/row/button cues and the Theme marker; its menu title,
menu hover, session tab and pane header cues are deliberately unchanged. The
status bar is unchanged in both themes.

Still not built (target remains): Shell and Outline dock panels (Shell is a
split pane; Outline absent); Tunnels as a dock panel (dialog/menu only);
Enter to open a session row; double-click-to-open and three-dot/history
menus in Filer/Explorer; session toolbar and breadcrumb (removed by PR #145);
WindTerm-style per-session tab colours (only a coloured label marker exists);
terminal gutter, per-split-group tab strips, quick bar and drag-and-drop
docking; disabled gating for menu items. The Find strip is built (PR #144).
