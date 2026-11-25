#!/usr/bin/env python3
"""Native Iced GUI smoke against an isolated loopback SSH server (Linux only).

Uses the existing AsyncSSH fixture's synthetic identity and real OpenSSH PTYs.
Launches the PRODUCTION Iced binary inside a private Xvfb display, clicks a
saved session, keyboard-splits a second connected pane, and opens the file dock.
The native file controls then drive one upload and one download; exact fixture
bytes and the isolated target root are checked outside the GUI after each click.
Only generated fixture keys and synthetic hostnames are used. Screenshots and
the non-secret event summary are kept as preview artifacts; credentials are not.
This is NOT a replacement for real macOS Windows visual/IME acceptance.
"""
from __future__ import annotations

import asyncio
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import zlib
import sys
import tempfile
import time

import asyncssh


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "scripts/test-native-ssh-smoke.py"
spec = importlib.util.spec_from_file_location("native_gui_ssh_fixture", SOURCE)
if spec is None or spec.loader is None:
    raise RuntimeError("unable to load native SSH fixture")
fixture = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = fixture
spec.loader.exec_module(fixture)


LOGGED_ECHO_LINES = (
    "echo:TOUR_PROBE",
    "echo:TERMINAL_OWNED",
    "echo:AFTER_MENU",
    "echo:AFTER_SENDER_ESCAPE",
    "echo:AFTER_GEAR",
    "echo:AFTER_CONTEXT_MENU",
    "echo:AFTER_FIND",
    "echo:FIND_P1_PANE",
    "echo:FIND_P1_AFTER",
    "echo:FIND_P2_PANE",
    "echo:FIND_P2_AFTER",
    "echo:FIND_P3_PANE",
    "echo:FIND_P3_AFTER",
    "echo:AFTER_EXIT_CANCEL",
    "echo:AFTER_TUNNEL_SETTINGS",
    "echo:AFTER_TUNNEL_TOGGLES",
    "echo:AFTER_EXTERNAL_EDIT",
)

# Opt-in "Open in External Editor" acceptance step (default OFF so the CI
# invocation is unchanged until the step has passed once).
EXTEDIT = os.environ.get("INSPIRUM_GUI_EXTEDIT") == "1"
EXTEDIT_MARKER = b"INSPIRUM_FAKE_EDITOR_MARKER\n"
EXTEDIT_MENU_LABEL = "Open in External Editor"
# The remote-file popup lists Download, Edit, Open in External Editor,
# Rename, Delete, Refresh, New Folder: 7 rows of 24 px plus borders.
EXTEDIT_MENU_ROWS = 7
EXTEDIT_ROW_HEIGHT = 24


def fake_editor_script(python: str, argv_log: Path, marker: bytes) -> str:
    """Source of the fake external editor.

    It records its argv (JSON, one line) to ``argv_log`` and appends
    ``marker`` to its LAST argument, after a 2 s pause so the app has
    certainly recorded the working copy's baseline stamp before the save.
    """
    return (
        f"#!{python}\n"
        "import json, sys, time\n"
        f"with open({str(argv_log)!r}, 'a', encoding='utf-8') as log:\n"
        "    log.write(json.dumps(sys.argv) + '\\n')\n"
        "time.sleep(2)\n"
        "with open(sys.argv[-1], 'ab') as target:\n"
        f"    target.write({marker!r})\n"
    )


def external_editor_ui_state(editor: Path) -> dict:
    """Partial ui-state.json: UiState is ``#[serde(default)]``, so theme
    (forced by INSPIRUM_THEME anyway) and docks keep their defaults. The
    path is double-quoted because the setting is whitespace-tokenized; no
    ``{file}`` placeholder, so the working copy is the last argument."""
    return {"external_editor": f'"{editor}"'}


def extedit_menu_region(click_x: int, click_y: int, width: int, height: int):
    """Search box for the widened 7-row remote-file popup around the click.

    The popup may open below the pointer or be clamped upward, and it is
    being widened (do not assume 200 px), so allow 7 rows plus slack on
    both sides vertically and up to 460 px to the right.
    """
    reach = EXTEDIT_MENU_ROWS * EXTEDIT_ROW_HEIGHT + 60
    return (
        max(0, click_x - 10),
        max(0, click_y - reach),
        min(width, click_x + 460),
        min(height, click_y + reach),
    )


def extedit_bytes_verdict(original: bytes, current: bytes, marker: bytes) -> str | None:
    """None when ``current`` is exactly ``original + marker``, else why not."""
    if current == original + marker:
        return None
    if not current.endswith(marker):
        return f"remote file does not end with the marker ({len(current)} bytes)"
    return "remote file ends with the marker but its other bytes changed"


class MonitoredSession(fixture.NativeSession):
    """Never log remote input: report only a fixed synthetic test marker."""

    def handle_line(self, line: str) -> None:
        if line in LOGGED_ECHO_LINES:
            self.events.write(f"NATIVE_ECHO:{line[5:]}")
        if line == "echo:FREE_TYPE_PROBE":
            self.events.write("UNEXPECTED_FREE_TYPE_PTY_INPUT")
        if line == "echo:LOCK_PROBE":
            self.events.write("UNEXPECTED_LOCKED_PTY_INPUT")
        if line == "screen-on":
            self.events.write("SCREEN_ON")
            assert self.chan is not None
            self.chan.write("\x1b[?1049h\x1b[2J\x1b[H\x1b[44;97mNATIVE_FULLSCREEN_ACTIVE\x1b[0m\r\n")
            return
        if line == "screen-off":
            self.events.write("SCREEN_OFF")
            assert self.chan is not None
            self.chan.write("\x1b[?1049l")
            return
        super().handle_line(line)


class MonitoredServer(fixture.NativeServer):
    def session_requested(self) -> MonitoredSession:
        return MonitoredSession(self.events)


# Exists only to prove the tour fails when the echo is missing.
# Must not suppress any other echo.
TOUR_WITHHOLD_ECHO = os.environ.get("INSPIRUM_GUI_TOUR_WITHHOLD_ECHO") == "1"
FIND_INJECT_PTY_BYTE = os.environ.get("INSPIRUM_GUI_FIND_INJECT_PTY_BYTE") == "1"
# Opt-in: the saved profile carries two labelled loopback -L forwards to a
# fixture echo service, and the connected run toggles each one from the
# Tunnels quick menu. Default mode keeps the empty-tunnel profile.
POPULATED = os.environ.get("INSPIRUM_GUI_POPULATED") == "1"
POPULATED_LABELS = ("Alpha forward", "Bravo forward")
# Product minimum (src/iced_app.rs window min_size). The X window manager in
# the tour honours any xdotool size, bypassing iced's min_size, so a smaller
# request yields a clipped frame the app never supports. Minimum-size evidence
# must be captured at exactly this size.
MIN_WINDOW = (960, 640)


def require_min_window(actual: tuple[int, int]) -> str:
    """Return the WxH tag for a minimum-size capture, or raise unless the
    actual window is exactly the supported minimum."""
    if tuple(actual) != MIN_WINDOW:
        raise RuntimeError(
            f"minimum-size evidence requires actual {MIN_WINDOW[0]}x{MIN_WINDOW[1]} "
            f"window; actual was {actual[0]}x{actual[1]}"
        )
    return f"{actual[0]}x{actual[1]}"


def free_loopback_port(exclude=()) -> int:
    """An ephemeral loopback port that is currently free (bind, then close)."""
    import socket
    for _ in range(20):
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = int(sock.getsockname()[1])
        if port not in exclude:
            return port
    raise RuntimeError("no distinct free loopback port")


# Quick-menu toggle slot geometry (src/iced_app.rs): the popup sits 4 px from
# the window's right edge with a 1 px border, rows have 10 px padding, and the
# action is a 28 px `toggler(..).size(14)` at the row's right end, so the
# switch spans [W-43, W-15] with its centre at W-29.
TOGGLER_WIDTH = 28
TOGGLER_RIGHT_INSET = 4 + 1 + 10
# Pixel refinement may only nudge the click inside the slot, never leave it.
TOGGLER_SLOT_SLACK = 6


def toggler_slot(window_width: int) -> tuple[int, int]:
    """Inclusive x range of the per-forward toggle in the right-hand popup."""
    right = window_width - TOGGLER_RIGHT_INSET
    return right - TOGGLER_WIDTH, right


def toggler_target(path: Path, label_box, window_width: int):
    """Click point of the per-forward toggle on the row titled by ``label_box``.

    The quick-menu control is an iced ``toggler`` with no text, at the right
    end of the row whose title OCR located. Only the fixed right-hand toggle
    slot (``toggler_slot``) is scanned: the row's grey subtitle runs under and
    past the title, and on Ubuntu run 45110697617 a per-row background sampled
    on a subtitle glyph turned the whole band into "foreground", so the old
    long-band scan picked a run at x=1157 in the subtitle instead of the
    switch at x~1251 and the toggle was never clicked. The background is the
    band's dominant colour (the popup fill), not any single pixel. A run is
    accepted only when it is switch-sized and centred on the slot; anything
    else falls back to the slot centre. The result is always inside the slot.
    """
    width, height, channels, rows = _read_png(path)
    slot_left, slot_right = toggler_slot(window_width)
    centre = (slot_left + slot_right) // 2
    y1 = max(0, label_box[1] - 2)
    y2 = min(height, label_box[3] + 16)
    cy = (label_box[1] + label_box[3]) // 2 + 7
    x1 = max(0, slot_left - TOGGLER_SLOT_SLACK)
    x2 = min(width, slot_right + TOGGLER_SLOT_SLACK + 1, window_width - 6)
    counts: dict[bytes, int] = {}
    for y in range(y1, y2):
        row = rows[y]
        for x in range(x1, x2):
            px = bytes(row[x * channels: x * channels + 3])
            counts[px] = counts.get(px, 0) + 1
    if not counts:
        raise RuntimeError(f"toggle slot {x1}..{x2} outside {path} ({width}x{height})")
    bg = max(counts, key=counts.get)
    columns = []
    for x in range(x1, x2):
        for y in range(y1, y2):
            px = rows[y][x * channels: x * channels + 3]
            if sum(abs(a - b) for a, b in zip(px, bg)) > 60:
                columns.append(x)
                break
    runs = []
    for x in columns:
        if runs and x - runs[-1][1] <= 2:
            runs[-1][1] = x
        else:
            runs.append([x, x])
    target, source = (centre, cy), "geometry"
    for left, right in runs:
        mid = (left + right) // 2
        if (TOGGLER_WIDTH - 8 <= right - left + 1 <= TOGGLER_WIDTH + 8
                and abs(mid - centre) <= TOGGLER_SLOT_SLACK):
            target, source = (mid, cy), "pixels"
            break
    if not slot_left <= target[0] <= slot_right:
        raise RuntimeError(f"toggle click {target} outside slot {slot_left}..{slot_right}")
    return target, source


def populated_profile_ssh(ports, echo_port: int) -> dict:
    """``ssh`` block for the populated profile (schema: src/lib.rs SshOptions)."""
    specs = [f"127.0.0.1:{port}:127.0.0.1:{echo_port}" for port in ports]
    if len(set(specs)) != len(specs):
        raise RuntimeError("populated forwards must be distinct")
    return {
        "local_forwards": specs,
        "tunnel_labels": [
            {"kind": "local", "spec": spec, "label": label}
            for spec, label in zip(specs, POPULATED_LABELS)
        ],
    }


def fixture_profiles(forward_ports, echo_port) -> list[dict]:
    """Saved profiles for the connected run.

    The first profile is always the plain terminal target ('CI connected
    fixture', no forwards): the UI test opens it and then extra panes /
    auxiliary shells, each its own SSH process, which would collide on any
    ``-L`` listener. Populated mode therefore puts the labelled forwards on a
    SECOND profile with the same SSH target; the Tunnels quick menu lists
    saved forwards from every profile.
    """
    base = {
        "host": "native-smoke",
        "user": "native-smoke",
        "port": None,
        "strict": True,
    }
    out = [{"name": "CI connected fixture", **base}]
    if POPULATED:
        out.append({
            "name": "CI tunnel fixture", **base,
            "ssh": populated_profile_ssh(forward_ports, echo_port),
        })
    return out


async def forward_echo(port: int, message: bytes, *, timeout: float = 15) -> None:
    """Connect to a GUI-started -L listener and require the fixture echo."""
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            reader, writer = await asyncio.wait_for(
                asyncio.open_connection("127.0.0.1", port), 3,
            )
        except (OSError, asyncio.TimeoutError) as error:
            last = error
            await asyncio.sleep(.25)
            continue
        try:
            writer.write(message)
            await writer.drain()
            data = await asyncio.wait_for(reader.readexactly(len(message)), 5)
        except (OSError, asyncio.TimeoutError, asyncio.IncompleteReadError) as error:
            last = error
            data = b""
        finally:
            writer.close()
            try:
                await writer.wait_closed()
            except OSError:
                pass
        if data == message:
            return
        await asyncio.sleep(.25)
    raise RuntimeError(f"forward listener {port} never echoed; last: {type(last).__name__}")


async def forward_refused(port: int, context: str, *, timeout: float = 8) -> None:
    """Require a real ECONNREFUSED on a stopped (or not yet started) listener."""
    deadline = time.monotonic() + timeout
    last = "connected"
    while time.monotonic() < deadline:
        try:
            _reader, writer = await asyncio.wait_for(
                asyncio.open_connection("127.0.0.1", port), 3,
            )
        except ConnectionRefusedError:
            return
        except (OSError, asyncio.TimeoutError) as error:
            last = type(error).__name__
        else:
            writer.close()
            last = "connected"
        await asyncio.sleep(.25)
    raise RuntimeError(f"{context}: listener {port} did not refuse ({last})")


def observe_pty_chunk(events, probe_state: dict[str, str | None], chunk: str) -> None:
    """Record any PTY input while a chrome probe is armed, without logging bytes."""
    if chunk and probe_state["phase"] is not None:
        events.write(f"UNEXPECTED_{probe_state['phase']}_PTY_INPUT")


async def monitored_shell(
    stdin: asyncssh.SSHReader[str],
    stdout: asyncssh.SSHWriter[str],
    stderr: asyncssh.SSHWriter[str],
    events: fixture.EventLog,
    probe_state: dict[str, str | None],
) -> None:
    # AsyncSSH dispatches all sessions through SSHServerStreamSession when
    # sftp_factory is configured. In that mode SSHServer.session_requested()
    # is bypassed, so supply an explicit shell stream handler as well.
    del stderr
    events.write("SESSION_OPEN")
    events.write("SESSION_STARTED")
    stdout.write("NATIVE_SMOKE_READY\r\n")
    buffer = ""
    try:
        while True:
            try:
                chunk = await stdin.read(8192)
            except asyncssh.TerminalSizeChanged:
                # AsyncSSH injects terminal-size updates into SSHReader as
                # exceptions. Splitting the Iced pane and opening the Files
                # dock both resize a genuine PTY: these are not disconnects.
                events.write("SHELL_PTY_RESIZE")
                continue
            if not chunk:
                events.write("SHELL_STREAM_EOF")
                return
            # Observe *any* PTY bytes while a chrome control owns focus,
            # including incomplete lines and a bare Escape. Never log input.
            # Phase names: MENU, SENDER, GEAR, CONTEXT, FIND. The marker is
            # UNEXPECTED_<PHASE>_PTY_INPUT, so FIND is
            # UNEXPECTED_FIND_PTY_INPUT.
            observe_pty_chunk(events, probe_state, chunk)
            buffer += chunk.replace("\r\n", "\n").replace("\r", "\n")
            # Cursor-key escape sequences are unbuffered PTY input, not
            # newline-terminated shell commands. Detect them as bytes arrive;
            # retain incomplete fragments across SSHReader.read() boundaries.
            # Only record a fixed synthetic marker, never actual user input.
            for sequence in ("\x1b[D", "\x1bOD"):
                while sequence in buffer:
                    buffer = buffer.replace(sequence, "", 1)
                    events.write("REMOTE_CURSOR_LEFT")
                    stdout.write("NATIVE_REMOTE_CURSOR_LEFT\r\n")
            while "\n" in buffer:
                line, buffer = buffer.split("\n", 1)
                if line == "echo:FREE_TYPE_PROBE":
                    events.write("UNEXPECTED_FREE_TYPE_PTY_INPUT")
                elif line == "echo:LOCK_PROBE":
                    events.write("UNEXPECTED_LOCKED_PTY_INPUT")
                elif line == "screen-on":
                    events.write("SCREEN_ON")
                    stdout.write("\x1b[?1049h\x1b[2J\x1b[H\x1b[44;97mNATIVE_FULLSCREEN_ACTIVE\x1b[0m\r\n")
                elif line == "screen-off":
                    events.write("SCREEN_OFF")
                    stdout.write("\x1b[?1049l")
                elif line in ("\x1b[D", "\x1bOD"):
                    events.write("REMOTE_CURSOR_LEFT")
                    stdout.write("NATIVE_REMOTE_CURSOR_LEFT\r\n")
                elif line.startswith("echo:"):
                    if line == "echo:TOUR_PROBE" and TOUR_WITHHOLD_ECHO:
                        continue
                    answer = f"NATIVE_ECHO:{line[5:]}"
                    if line in LOGGED_ECHO_LINES:
                        events.write(answer)
                    stdout.write(f"{answer}\r\n")
                elif line == "exit":
                    events.write("SESSION_EXIT_REQUEST")
                    return
    except Exception as exc:
        # No private material or server input is included in diagnostics.
        events.write(f"SHELL_HANDLER_EXCEPTION:{type(exc).__name__}")
        raise
    finally:
        events.write("SHELL_HANDLER_ENDED")
        events.write("SESSION_CLOSED:clean")



class MonitoredSFTPServer(asyncssh.SFTPServer):
    """Record only bounded fixture lifecycle markers, never paths or contents."""

    def __init__(self, chan, root: Path, events: fixture.EventLog) -> None:
        super().__init__(chan, chroot=str(root))
        self.events = events

    async def scandir(self, path: bytes):
        async for entry in super().scandir(path):
            yield entry
        self.events.write("SFTP_LIST_READY")


def isolated_sftp_server(
    chan: asyncssh.SSHServerChannel,
    root: Path,
    events: fixture.EventLog,
) -> asyncssh.SFTPServer:
    # This is the *real* SSH subsystem used by the Files dock; it is restricted
    # to a generated disposable directory and not the runner's home folder.
    events.write("SFTP_SUBSYSTEM_STARTED")
    return MonitoredSFTPServer(chan, root, events)


def command(*args: str, env: dict[str, str], timeout: float = 12) -> str:
    result = subprocess.run(
        args, env=env, capture_output=True, text=True,
        check=True, timeout=timeout,
    )
    return result.stdout.strip()


def screenshot(window: str, path: Path, env: dict[str, str]) -> None:
    result = subprocess.run(
        ["import", "-window", window, f"png:{path}"],
        env=env, capture_output=True, text=True, timeout=15,
    )
    if result.returncode != 0:
        raise RuntimeError(f"ImageMagick screenshot failed: {result.stderr[:300]}")
    if not path.is_file() or path.stat().st_size < 1000:
        raise RuntimeError(f"empty GUI screenshot: {path.name}")


def parse_visible_words(tsv: str):
    """Return word boxes above a zero confidence floor.

    The old locator dropped anything below 40. That discarded the only
    visible fragments of the remote fixture name. Zero and negative
    confidences are still noise (tesseract uses -1 for non-words).
    """
    rows = []
    for line in tsv.splitlines():
        parts = line.split("\t")
        if len(parts) < 12 or not parts[11].strip():
            continue
        try:
            confidence = float(parts[10])
            left = int(parts[6])
            top = int(parts[7])
            width = int(parts[8])
            height = int(parts[9])
        except ValueError:
            continue
        if confidence <= 0 or width <= 0 or height <= 0:
            continue
        rows.append((parts[11].strip(), left, top, width, height, confidence))
    return rows


def _key(text: str) -> str:
    """Compare labels after OCR drops spaces, underscores, and arrows."""
    return "".join(ch for ch in text if ch.isalnum() or ch == ".")


# Monospace dotted zeros in 13 px table cells are read as ``@``, which _key
# would drop entirely. Map it back before the exact 3-char prefix check.
_OCR_GLYPH_FIXES = str.maketrans({"@": "0"})


def _near_key(text: str, label: str) -> bool:
    """Whole-line match tolerating a few single-glyph OCR confusions.

    Only used for long labels (filenames), where two substitutions such as
    ``GUI`` read as ``GUL`` or ``bin`` as ``bir`` cannot turn one fixture name
    into another; the digit prefix and length must still match exactly.
    """
    got, want = _key(text.translate(_OCR_GLYPH_FIXES)), _key(label)
    if len(want) < 12 or len(got) != len(want):
        return False
    if got[:3] != want[:3]:
        return False
    return sum(a != b for a, b in zip(got, want)) <= 2


def match_visible_segment(segments, label: str):
    """Match a label against each OCR'd segment as a whole, with tolerance."""
    for words in segments:
        if not words:
            continue
        # Drop leading icon glyphs (file icons read as ``)`` or ``*``) so the
        # bounds and click centre sit on the name, not the icon.
        while words and not _key(words[0][0].translate(_OCR_GLYPH_FIXES)):
            words = words[1:]
        if not words:
            continue
        joined = "".join(word[0] for word in words)
        if _near_key(joined, label):
            return _bounds(words)
    return None


def _same_line(left, right) -> bool:
    overlap = min(left[2] + left[4], right[2] + right[4]) - max(left[2], right[2])
    if overlap <= 0:
        return False
    return overlap >= min(left[4], right[4]) * 0.45


def _group_lines(rows):
    ordered = sorted(rows, key=lambda row: (row[2], row[1]))
    lines = []
    for row in ordered:
        placed = False
        for line in lines:
            if _same_line(line[0], row):
                line.append(row)
                placed = True
                break
        if not placed:
            lines.append([row])
    for line in lines:
        line.sort(key=lambda row: row[1])
    return lines


def _bounds(matched):
    bounds = (
        min(item[1] for item in matched) + 4,
        min(item[2] for item in matched) + 3,
        max(item[1] + item[3] for item in matched) - 4,
        max(item[2] + item[4] for item in matched) - 3,
    )
    if bounds[2] > bounds[0] and bounds[3] > bounds[1]:
        return bounds
    return None


def _token_matches(text: str, label: str) -> bool:
    got = _key(text)
    want = _key(label)
    if not got or not want:
        return False
    if got == want:
        return True
    if got.startswith(want):
        remainder = got[len(want):]
        return bool(remainder) and all(not ch.isalnum() for ch in remainder)
    return False


def match_visible_label(rows, label: str):
    """Return interior bounds for label, or None.

    A match is either one OCR token (arrows may be glued on, as in
    ``Upload->``) or a left-to-right run of tokens on the same visible
    line whose normalized text equals the label. A proper prefix such as
    ``444_GUI_DOWNLOAD`` is not a click target. Tokens on the next line
    are not consumed.
    """
    target = _key(label)
    if not target:
        return None
    for line in _group_lines(rows):
        for row in line:
            if _token_matches(row[0], label):
                bounds = _bounds([row])
                if bounds is not None:
                    return bounds
        for start, first in enumerate(line):
            if not target.startswith(_key(first[0])):
                continue
            matched = []
            joined = ""
            previous = None
            for row in line[start:]:
                if previous is not None:
                    gap = row[1] - (previous[1] + previous[3])
                    limit = max(64, previous[4] * 3, row[4] * 3)
                    if gap > limit or gap < -max(8, previous[4]):
                        break
                piece = _key(row[0])
                if not piece:
                    continue
                trial = joined + piece
                if trial != target and not target.startswith(trial):
                    break
                matched.append(row)
                joined = trial
                previous = row
                if joined == target:
                    bounds = _bounds(matched)
                    if bounds is not None:
                        return bounds
                    break
                if len(matched) >= 12:
                    break
    return None


def _read_png(path: Path):
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise RuntimeError(f"visible label targeting expected a PNG screenshot: {path.name}")
    pos = 8
    width = height = color_type = None
    idat = []
    while pos + 8 <= len(data):
        length = struct.unpack(">I", data[pos:pos + 4])[0]
        kind = data[pos + 4:pos + 8]
        chunk = data[pos + 8:pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, _depth, color_type = struct.unpack(">IIBB", chunk[:10])
        elif kind == b"IDAT":
            idat.append(chunk)
        elif kind == b"IEND":
            break
    if not width or not height or color_type not in (2, 6):
        raise RuntimeError(f"unsupported screenshot PNG: {path.name}")
    raw = zlib.decompress(b"".join(idat))
    channels = 3 if color_type == 2 else 4
    stride = width * channels
    rows = []
    index = 0
    previous = bytearray(stride)
    for _y in range(height):
        filt = raw[index]
        index += 1
        row = bytearray(raw[index:index + stride])
        index += stride
        if filt == 1:
            for x in range(stride):
                left = row[x - channels] if x >= channels else 0
                row[x] = (row[x] + left) & 255
        elif filt == 2:
            for x in range(stride):
                row[x] = (row[x] + previous[x]) & 255
        elif filt == 3:
            for x in range(stride):
                left = row[x - channels] if x >= channels else 0
                row[x] = (row[x] + ((left + previous[x]) // 2)) & 255
        elif filt == 4:
            for x in range(stride):
                left = row[x - channels] if x >= channels else 0
                up = previous[x]
                ul = previous[x - channels] if x >= channels else 0
                predict = left + up - ul
                pa, pb, pc = abs(predict - left), abs(predict - up), abs(predict - ul)
                pred = left if pa <= pb and pa <= pc else up if pb <= pc else ul
                row[x] = (row[x] + pred) & 255
        elif filt != 0:
            raise RuntimeError(f"unsupported PNG filter {filt} in {path.name}")
        previous = row
        rows.append(row)
    return width, height, channels, rows


def _write_png(path: Path, width: int, height: int, rgb: bytes) -> None:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )
    raw = bytearray()
    stride = width * 3
    for y in range(height):
        raw.append(0)
        raw.extend(rgb[y * stride:(y + 1) * stride])
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
        + chunk(b"IEND", b"")
    )


def _text_threshold(width, height, channels, rows) -> int:
    sample = []
    for y in range(0, height, 4):
        row = rows[y]
        for x in range(0, width, 4):
            base = x * channels
            sample.append((row[base] + row[base + 1] + row[base + 2]) // 3)
    sample.sort()
    if not sample:
        return 120
    p90 = sample[int(len(sample) * 0.90)]
    p99 = sample[min(len(sample) - 1, int(len(sample) * 0.99))]
    # Measured on the failing 1280x800 dock: background p90 is ~30 and the
    # light filename pixels sit near 205. The midpoint separates them
    # without a hardcoded crop.
    return max(90, min(160, (p90 + p99) // 2))


def _ocr_tsv(tesseract: str, image: Path, psm: str) -> str:
    try:
        result = subprocess.run(
            [tesseract, str(image), "stdout", "--psm", psm, "tsv"],
            capture_output=True, text=True, timeout=15 * TIME_SCALE,
        )
    except subprocess.TimeoutExpired:
        return ""
    if result.returncode != 0:
        return ""
    return result.stdout


def _scaled_words(tsv: str, origin, scale: int):
    words = []
    ox, oy = origin
    for text, x, y, width, height, confidence in parse_visible_words(tsv):
        words.append((
            text,
            ox + x // scale,
            oy + y // scale,
            max(1, width // scale),
            max(1, height // scale),
            confidence,
        ))
    return words


def _upscaled_crop(
    rows, channels, box, scale: int, dest: Path, *, invert: bool = False,
) -> None:
    left, top, right, bot = box
    width = right - left + 1
    height = bot - top + 1
    out_w = width * scale
    out_h = height * scale
    rgb = bytearray(out_w * out_h * 3)
    for y in range(out_h):
        source = rows[top + min(height - 1, y // scale)]
        for x in range(out_w):
            base = (left + min(width - 1, x // scale)) * channels
            dest_at = (y * out_w + x) * 3
            if invert:
                lum = 255 - (source[base] + source[base + 1] + source[base + 2]) // 3
                rgb[dest_at:dest_at + 3] = bytes((lum, lum, lum))
            else:
                rgb[dest_at:dest_at + 3] = source[base:base + 3]
    _write_png(dest, out_w, out_h, bytes(rgb))


def _visible_bands(width, height, channels, rows, thresh: int):
    bands = []
    active = False
    start = 0
    minimum = max(6, width // 200)
    for y in range(height):
        count = 0
        row = rows[y]
        for x in range(0, width, 2):
            base = x * channels
            if (row[base] + row[base + 1] + row[base + 2]) // 3 >= thresh:
                count += 1
        lit = count >= minimum
        if lit and not active:
            start = y
            active = True
        elif not lit and active:
            if y - start >= 5:
                bands.append((start, y - 1))
            active = False
    if active and height - start >= 5:
        bands.append((start, height - 1))
    return bands


def _band_clusters(width, rows, channels, thresh, y1, y2, gap: int):
    columns = []
    for x in range(width):
        for y in range(y1, y2 + 1):
            base = x * channels
            pixel = rows[y]
            if (pixel[base] + pixel[base + 1] + pixel[base + 2]) // 3 >= thresh:
                columns.append(x)
                break
    if not columns:
        return []
    clusters = []
    start = previous = columns[0]
    for x in columns[1:]:
        if x - previous > gap:
            if previous - start >= 8:
                clusters.append((start, previous))
            start = x
        previous = x
    if previous - start >= 8:
        clusters.append((start, previous))
    return clusters


_PIXEL_CACHE: dict[tuple, list] = {}


def _band_segments(width, rows, channels, thresh, y1, y2, gap: int):
    """Split one light band into wide segments separated by dark columns.

    Outlined controls put a bright vertical border at both ends of every row,
    which full-width OCR reads as glyphs. Segments wider than a short control
    are OCR'd on their own so one row never carries its neighbours' borders.
    """
    columns = []
    for x in range(width):
        base = x * channels
        for y in range(y1, y2 + 1):
            pixel = rows[y]
            if (pixel[base] + pixel[base + 1] + pixel[base + 2]) // 3 >= thresh:
                columns.append(x)
                break
    if not columns:
        return []
    segments = []
    start = previous = columns[0]
    for x in columns[1:]:
        if x - previous > gap:
            segments.append((start, previous))
            start = x
        previous = x
    segments.append((start, previous))
    return [(x1, x2) for x1, x2 in segments if x2 - x1 >= 40]


def visible_segment_rows(path: Path, tesseract: str, *, invert: bool = False):
    """OCR each light band segment by segment; coordinates in source pixels.

    Returns a list of per-segment word lists so a caller can match one
    segment as a whole without neighbouring controls on the same line.

    ``invert`` re-reads each segment as dark-on-light grey. On the dark theme
    a 13 px monospace table cell (file icon + name) read as noise in colour
    but as ``@@@_GUI_DOWNLOAD_FIXTURE. bir`` inverted.
    """
    stat = path.stat()
    key = (str(path), stat.st_mtime_ns, stat.st_size, "segments", invert)
    cached = _PIXEL_CACHE.get(key)
    if cached is not None:
        return cached
    width, height, channels, rows = _read_png(path)
    thresh = _text_threshold(width, height, channels, rows)
    segments = []
    temporary = Path(tempfile.mkstemp(suffix=".png")[1])
    try:
        for y1, y2 in _visible_bands(width, height, channels, rows, thresh):
            for x1, x2 in _band_segments(width, rows, channels, thresh, y1, y2, 24):
                # Trim one-pixel outlines at either end, then pad with the
                # row's own background so tesseract sees a clean line.
                box = (
                    min(width - 1, x1 + 3),
                    max(0, y1 - 4),
                    max(0, x2 - 3),
                    min(height - 1, y2 + 4),
                )
                if box[2] - box[0] < 24:
                    continue
                _upscaled_crop(rows, channels, box, 4, temporary, invert=invert)
                segments.append(_scaled_words(
                    _ocr_tsv(tesseract, temporary, "7"), (box[0], box[1]), 4,
                ))
    finally:
        temporary.unlink(missing_ok=True)
    _PIXEL_CACHE[key] = segments
    return segments


def visible_pixel_rows(path: Path, tesseract: str, *, clusters: bool = False):
    """OCR each light text row, optionally each narrow glyph cluster.

    Coordinates are mapped back to the source screenshot. This is the
    fallback when full-window PSM 11 splits or drops a label. Cluster
    passes recover short controls such as ``Dock right`` that a long-line
    OCR pass garbles, using the glyph's own pixels rather than a fixed crop.
    """
    stat = path.stat()
    key = (str(path), stat.st_mtime_ns, stat.st_size, clusters)
    cached = _PIXEL_CACHE.get(key)
    if cached is not None:
        return cached
    width, height, channels, rows = _read_png(path)
    thresh = _text_threshold(width, height, channels, rows)
    words = []
    temporary = Path(tempfile.mkstemp(suffix=".png")[1])
    try:
        for y1, y2 in _visible_bands(width, height, channels, rows, thresh):
            pad = 3
            box = (
                0,
                max(0, y1 - pad),
                width - 1,
                min(height - 1, y2 + pad),
            )
            _upscaled_crop(rows, channels, box, 4, temporary)
            words.extend(_scaled_words(_ocr_tsv(tesseract, temporary, "7"), (box[0], box[1]), 4))
            if not clusters:
                continue
            for x1, x2 in _band_clusters(width, rows, channels, thresh, y1, y2, 14):
                if x2 - x1 > 160:
                    continue
                cluster = (
                    max(0, x1 - 4),
                    max(0, y1 - 4),
                    min(width - 1, x2 + 4),
                    min(height - 1, y2 + 4),
                )
                _upscaled_crop(rows, channels, cluster, 6, temporary)
                words.extend(_scaled_words(
                    _ocr_tsv(tesseract, temporary, "7"),
                    (cluster[0], cluster[1]),
                    6,
                ))
    finally:
        temporary.unlink(missing_ok=True)
    _PIXEL_CACHE[key] = words
    return words


def _region_crop_upscaled(path: Path, region, scale: int, dest: Path, *, invert: bool = False):
    """Crop ``region`` from ``path`` and write it upscaled (nearest) as grey.

    Pure python (no ImageMagick): grey conversion once per source pixel, then
    whole rows are repeated with bytes multiplication, so a 1280x250 crop at
    x3 stays well under a second even on a loaded host. Returns the clamped
    crop origin.
    """
    width, height, channels, rows = _read_png(path)
    x1, y1, x2, y2 = region
    x1 = max(0, min(width - 1, x1))
    y1 = max(0, min(height - 1, y1))
    x2 = max(x1 + 1, min(width, x2))
    y2 = max(y1 + 1, min(height, y2))
    out_rows = []
    for y in range(y1, y2):
        row = rows[y]
        line = bytearray()
        for x in range(x1, x2):
            base = x * channels
            lum = (row[base] + row[base + 1] + row[base + 2]) // 3
            if invert:
                lum = 255 - lum
            line += bytes((lum, lum, lum)) * scale
        out_rows.append(bytes(line) * scale)
    _write_png(dest, (x2 - x1) * scale, (y2 - y1) * scale, b"".join(out_rows))
    return x1, y1


def region_upscaled_rows(path: Path, tesseract: str, region, *, psm: str = "6",
                         scale: int = 3, invert: bool = False):
    """OCR an upscaled crop of ``region``; word boxes in window coordinates."""
    temporary = Path(tempfile.mkstemp(suffix=".png")[1])
    try:
        origin = _region_crop_upscaled(path, region, scale, temporary, invert=invert)
        tsv = _ocr_tsv(tesseract, temporary, psm)
    finally:
        temporary.unlink(missing_ok=True)
    return _scaled_words(tsv, origin, scale)


def ocr_rows(path: Path, tesseract: str, region=None):
    if region is not None:
        return region_upscaled_rows(path, tesseract, region, psm="6")
    return _scaled_words(_ocr_tsv(tesseract, path, "11"), (0, 0), 1)


# One locate attempt must fit well inside the caller's retry window
# (click_label / locate_rendered_label retry with fresh screenshots).
# Slow/loaded hosts (local docker runs) raise it via INSPIRUM_GUI_OCR_BUDGET.
# The budget gates only the expensive whole-window stages; the region crop
# stages and the bounded inverted segment search always get their turn.
LOCATE_BUDGET_S = float(os.environ.get("INSPIRUM_GUI_OCR_BUDGET", "12"))
# Multiplier for waits that wrap screenshots + OCR (not protocol events).
TIME_SCALE = float(os.environ.get("INSPIRUM_GUI_TIME_SCALE", "1.0"))
# Last-resort inverted stage gets at least this long even after slow stages.
INVERTED_MIN_S = 10.0 * TIME_SCALE
# Session toolbar + tab bar; the inverted stage never scans above it unless
# the caller passes an explicit region.
TOOLBAR_BOTTOM_Y = 90


# Tunnel Settings dialog geometry (src/iced_app.rs): width is
# clamp(window - 2*24, 600, 840), centred; 12 px padding; the 300 px list
# card, a 12 px gap, then the editor card. Measured on the 1280x800 populated
# dark frame (connected-tunnels-editor-selected.png): dialog x 220..1060,
# list card 245..545, editor card 557..1035, "Listen Port" label x 569..650,
# its input 719..1023 x 287..310 with digits at y 295..304, Destination Port
# input y 357..380; OCR puts the "Tunnel Manager" heading top at y 151.
TUNNEL_DIALOG_MAX_W = 840
TUNNEL_DIALOG_MIN_W = 600
TUNNEL_DIALOG_MARGIN = 24
# Editor card left edge relative to the dialog's left edge (557 - 220).
TUNNEL_EDITOR_CARD_DX = 337
# Listen Port input left edge (719 - 220) at the widest label column; the
# label column can shrink to 128 px (22 px less) on narrow dialogs.
TUNNEL_LISTEN_INPUT_DX = 499 - 22 - 6
# Listen Port row relative to the heading's OCR top: digits at +144..+153.
# The band stops above the Destination Host input (+171) and far above the
# Destination Port digits (+216), so a form showing the port in another row
# cannot satisfy it.
TUNNEL_LISTEN_ROW_DY = (124, 170)


def tunnel_dialog_left(width: int) -> tuple[int, int]:
    """``(left, dialog_width)`` of the centred Tunnel Settings dialog."""
    dialog = max(TUNNEL_DIALOG_MIN_W, min(TUNNEL_DIALOG_MAX_W, width - 2 * TUNNEL_DIALOG_MARGIN))
    return max(0, (width - dialog) // 2), dialog


def editor_form_region(width: int, height: int):
    """Tunnel Manager editor card: right of the 300 px list card, inside the
    dialog. The list sits on the left, so a list-row label can never prove
    which forward the form is editing. Used for "sibling port absent"."""
    left, dialog = tunnel_dialog_left(width)
    return (left + TUNNEL_EDITOR_CARD_DX - 6, 42, min(width, left + dialog), height)


def listen_port_row_region(heading_bounds, width: int):
    """Tight box around the Listen Port input, anchored to the located
    "Tunnel Manager" heading. Only digits here prove the selected forward:
    the editor card also shows the destination port (the shared echo
    service), so the whole card would accept a port from the wrong row."""
    left, dialog = tunnel_dialog_left(width)
    top = heading_bounds[1]
    return (
        left + TUNNEL_LISTEN_INPUT_DX,
        top + TUNNEL_LISTEN_ROW_DY[0],
        min(width, left + dialog - 20),
        top + TUNNEL_LISTEN_ROW_DY[1],
    )


# Remove ("−") button under the 350 px list: x 285..313, y 544..567 on the
# 1280x800 frame, i.e. centre (dialog left + 79, heading top + 405).
TUNNEL_REMOVE_BUTTON_D = (79, 405)
# Footer Save (primary fill, right-aligned): y 661..684 = heading top +510..
# +533. Scanned for, not assumed: the band searched is +300..+620.
TUNNEL_SAVE_SCAN_DY = (300, 620)


def listen_input_centre(heading_bounds, width: int) -> tuple[int, int]:
    """Click target inside the Listen Port text input."""
    x1, y1, x2, y2 = listen_port_row_region(heading_bounds, width)
    return ((x1 + x2) // 2, (y1 + y2) // 2)


def tunnel_remove_centre(heading_bounds, width: int) -> tuple[int, int]:
    left, _dialog = tunnel_dialog_left(width)
    return (left + TUNNEL_REMOVE_BUTTON_D[0], heading_bounds[1] + TUNNEL_REMOVE_BUTTON_D[1])


def primary_fill_run(path: Path, y: int, x1: int, x2: int, *, min_width: int = 30,
                     image=None):
    """Like ``danger_fill_run`` for the blue primary fill (dark theme
    (33,150,243)). Returns ``(left, right)`` or ``None``. ``image`` is an
    already decoded ``_read_png`` result."""
    width, height, channels, rows = image or _read_png(path)
    if not 0 <= y < height:
        return None
    row = rows[y]
    blue = [
        x for x in range(max(0, x1), min(width, x2))
        if row[x * channels + 2] >= 200
        and row[x * channels + 2] - row[x * channels] >= 120
    ]
    if len(blue) < min_width:
        return None
    return (blue[0], blue[-1] + 1)


def tunnel_save_button(path: Path, heading_bounds, width: int):
    """Centre and span of the footer Save fill, or ``None``.

    Only the right half of the dialog below the editor fields is scanned,
    so the primary "Local" kind toggle (heading +74..+97) cannot match; the
    run must end near the dialog's right edge (Save is right-aligned).
    """
    left, dialog = tunnel_dialog_left(width)
    right = left + dialog
    top = heading_bounds[1]
    image = _read_png(path)
    hits = []
    for y in range(top + TUNNEL_SAVE_SCAN_DY[0], top + TUNNEL_SAVE_SCAN_DY[1]):
        run = primary_fill_run(path, y, left + dialog // 2, right, image=image)
        if run and run[1] >= right - 60 and run[1] - run[0] <= 120:
            hits.append((y, run))
    if len(hits) < 8:
        return None
    ys = [y for y, _run in hits]
    lefts = [run[0] for _y, run in hits]
    rights = [run[1] for _y, run in hits]
    box = (min(lefts), min(ys), max(rights), max(ys) + 1)
    return ((box[0] + box[2]) // 2, (box[1] + box[3]) // 2), box


def tunnel_notice_region(save_box, width: int):
    """Footer notice text, left of Cancel/Save on the Save row."""
    left, _dialog = tunnel_dialog_left(width)
    return (left + 6, save_box[1] - 12, max(left + 7, save_box[0] - 80), save_box[3] + 12)


def saved_local_forwards(profiles_path: Path, profile_name: str) -> list[str]:
    """``ssh.local_forwards`` of ``profile_name`` as persisted on disk."""
    data = json.loads(profiles_path.read_text(encoding="utf-8"))
    sessions = data.get("sessions", data) if isinstance(data, dict) else data
    for session in sessions:
        if session.get("name") == profile_name:
            return list((session.get("ssh") or {}).get("local_forwards") or [])
    raise RuntimeError(f"profile {profile_name!r} missing from {profiles_path}")


def saved_profile_names(profiles_path: Path) -> list[str]:
    """Profile names as persisted on disk (same layouts as above)."""
    data = json.loads(profiles_path.read_text(encoding="utf-8"))
    sessions = data.get("sessions", data) if isinstance(data, dict) else data
    return [session.get("name") for session in sessions]


# Sessions-pane Edit Profile probe (Edit SSH connection dialog).
# DIALOG_DEFAULT_WIDTH (src/iced_app.rs) centred on the window; the dialog
# frame hangs below the 32 px menu bar and stops above the status bar.
CONNECTION_DIALOG_W = 600
CONNECTION_DIALOG_TITLE = "Edit SSH connection"
# Rename target for the running owner; must never reach the profile file.
PROFILE_RENAME_PROBE = "CI tunnel renamed"
# A refused Submit keeps the dialog open and pushes `form.error` (palette
# danger, dark #FF5C5C) above the buttons. Counted as fresh danger-text
# pixels against the pre-Submit frame so the wording is not assumed.
PROFILE_REFUSAL_MIN_RED = 40


def connection_dialog_region(width: int, height: int):
    """Box of the centred Edit/New SSH connection dialog."""
    half = CONNECTION_DIALOG_W // 2 + 4
    return (max(0, width // 2 - half), 32, min(width, width // 2 + half), max(33, height - 22))


def danger_text_pixels(path: Path, region, *, image=None) -> int:
    """Pixels of saturated danger-red text inside ``region``.

    Dark danger is (255,92,92); the amber preflight warning (246,184,75)
    and anything dimmed by the modal backdrop (R <= 166) are excluded.
    """
    width, height, channels, rows = image or _read_png(path)
    x1, y1, x2, y2 = region
    count = 0
    for y in range(max(0, y1), min(height, y2)):
        row = rows[y]
        for x in range(max(0, x1), min(width, x2)):
            r, g, b = row[x * channels], row[x * channels + 1], row[x * channels + 2]
            if r >= 200 and g <= 140 and b <= 140 and r - g >= 90:
                count += 1
    return count


def connection_submit_button(path: Path, save_only_bounds, width: int):
    """Centre and box of the primary "Save changes" fill right of the
    OCR-located "Save only" label on the same button row, or ``None``.
    Only that row is scanned, so a selected toggle elsewhere in the form
    (flat_selected) cannot match."""
    image = _read_png(path)
    a1, a2, a3, a4 = save_only_bounds
    hits = []
    for y in range(a2 - 8, a4 + 9):
        run = primary_fill_run(path, y, a3 + 4, min(width, a3 + 260), min_width=40, image=image)
        if run and run[1] - run[0] <= 200:
            hits.append((y, run))
    if len(hits) < 6:
        return None
    ys = [y for y, _run in hits]
    box = (min(r[0] for _y, r in hits), min(ys), max(r[1] for _y, r in hits), max(ys) + 1)
    return ((box[0] + box[2]) // 2, (box[1] + box[3]) // 2), box


def local_forward_input_centre(label_bounds, width: int) -> tuple[int, int]:
    """Click target inside the "Local forward (-L)" text input, which sits
    4 px below its 13 px label (padding 8: ~33 px tall)."""
    left, _top, right, _bottom = connection_dialog_region(width, 10_000)
    x = max(label_bounds[0] + 120, left + 160)
    return (min(x, right - 40), label_bounds[3] + 22)


def profile_edit_refused(*, file_before: bytes, file_after: bytes, title_visible: bool,
                         red_before: int, red_after: int, context: str) -> None:
    """Verdict for a Submit that must be refused BEFORE any disk write.

    Truth is the profile file (byte-identical); the dialog must still be
    open, and fresh danger text is the positive signal that the click
    reached Submit (a missed click also leaves the file unchanged)."""
    if file_after != file_before:
        raise RuntimeError(f"{context}: refused profile edit reached the profile file")
    if not title_visible:
        raise RuntimeError(f"{context}: Submit closed the {CONNECTION_DIALOG_TITLE!r} dialog")
    if red_after - red_before < PROFILE_REFUSAL_MIN_RED:
        raise RuntimeError(
            f"{context}: no inline refusal after Submit (danger pixels "
            f"{red_before} -> {red_after}, need +{PROFILE_REFUSAL_MIN_RED})"
        )


def light_locate(path: Path, label: str, *, region, budget=None, tesseract=None):
    """Dark-text-on-light locator for the LIGHT tour chrome.

    The default locator's band/segment stages assume light text on a dark
    background (the connected run forces the dark theme), and on the light
    Tunnel Manager they miss even the heading. Tesseract reads dark-on-light
    natively, so this OCRs a non-inverted grey x3 crop of ``region`` (psm 6,
    then 11) and keeps only boxes wholly inside it. ``budget`` is accepted
    for ``require_editor_selection`` compatibility.
    """
    tesseract = tesseract or shutil.which("tesseract")
    if tesseract is None:
        raise RuntimeError("tesseract unavailable")
    words = []
    for psm in ("6", "11"):
        rows = region_upscaled_rows(path, tesseract, region, psm=psm)
        box = _bounds_inside(match_visible_label(rows, label), region)
        if box:
            return box
        words.extend(row[0] for row in rows[:30])
    raise RuntimeError(f"light label {label!r} not in {region} of {path}; words: {words[:40]}")


# Tunnels quick popup row (src/iced_app.rs tunnel_quick_row): title line,
# then the "<profile> . port N" subtitle. Measured on the populated 960x640
# light frame: rows are 36 px apart, title OCR top 83/121, subtitle 101/137.
POPUP_ROW_ABOVE = 4
POPUP_ROW_BELOW = 32
# Row-band OCR passes, cheapest first. Small grey digits flip 0 -> 6 at
# some scales (55057 read 55657), so several are tried; a pass only counts
# when it reads the exact port.
POPUP_ROW_PASSES = (("6", 3), ("6", 2), ("11", 2), ("11", 3), ("6", 4), ("7", 2))


def _light_rows(path, region, *, psm, scale):
    tesseract = shutil.which("tesseract")
    if tesseract is None:
        raise RuntimeError("tesseract unavailable")
    return region_upscaled_rows(path, tesseract, region, psm=psm, scale=scale)


def light_popup_row(path: Path, name: str, port: int, sibling_port: int, *,
                    region, footer_top: int, ocr=None):
    """Prove a LIGHT Tunnels popup row shows ``name`` with its own ``port``.

    The full title ("Bravo forward") is not required: tesseract reads its
    second word inconsistently (``Torward``). Instead the exact first word
    ``name`` must be inside ``region`` above ``footer_top``, AND that word's
    row band (its title line plus the subtitle below, cut at the footer)
    must contain the exact ``port`` without ``sibling_port``. A label or a
    port elsewhere (another row, below the footer) never proves the row.
    Returns bounds spanning the name word and the port.
    """
    ocr = ocr or _light_rows
    want = _key(name)
    words, seen = [], set()
    for psm in ("6", "11"):
        rows = ocr(path, region, psm=psm, scale=3)
        words.extend(row[0] for row in rows[:30])
        for row in rows:
            if _key(row[0]) != want or not _word_inside(row, region):
                continue
            if row[2] + row[4] > footer_top or (row[1], row[2]) in seen:
                continue
            seen.add((row[1], row[2]))
            band = (row[1] - 6, row[2] - POPUP_ROW_ABOVE, region[2],
                    min(footer_top, row[2] + POPUP_ROW_BELOW))
            for band_psm, scale in POPUP_ROW_PASSES:
                band_rows = [w for w in ocr(path, band, psm=band_psm, scale=scale)
                             if _word_inside(w, band)]
                keys = [_key(w[0]) for w in band_rows]
                if str(sibling_port) in keys:
                    break  # sibling's port in this row: not this forward
                hits = [w for w in band_rows if _key(w[0]) == str(port)]
                if hits:
                    hit = hits[0]
                    return (min(row[1], hit[1]), row[2],
                            max(row[1] + row[3], hit[1] + hit[3]),
                            max(row[2] + row[4], hit[2] + hit[4]))
    raise RuntimeError(
        f"popup row {name!r} with listen port {port} not in {region} above "
        f"footer y={footer_top} of {path}; words: {words[:40]}"
    )


def require_editor_selection(path, port: int, sibling_port: int, form_region, *,
                             row_region=None, locate=None):
    """Prove the editor form shows ``port`` and NOT ``sibling_port``.

    A no-op click leaves the previous selection's port in the form, so the
    sibling must be absent anywhere in ``form_region``. The selected port
    must be inside ``row_region`` (the Listen Port input) when given, else
    inside ``form_region`` -- never a list subtitle. Returns its bounds.
    """
    locate = locate or locate_visible_label
    where = row_region or form_region
    try:
        box = locate(path, str(port), region=where)
    except RuntimeError as exc:
        raise RuntimeError(
            f"editor form {where} does not show selected listen port {port}"
        ) from exc
    if _bounds_inside(box, where) is None:
        raise RuntimeError(f"listen port {port} box {box} outside editor row {where}")
    try:
        stale = locate(path, str(sibling_port), region=form_region)
    except RuntimeError:
        return box
    raise RuntimeError(
        f"editor form still shows sibling port {sibling_port} at {stale}; "
        f"selection click was a no-op"
    )


def _bounds_inside(bounds, region):
    """Return ``bounds`` only when the box lies wholly inside ``region``.

    ``region`` is ``(left, top, right, bottom)``. ``None`` keeps every box,
    matching the historical whole-window search. An outside box is not a
    match: callers skip it and keep searching.
    """
    if bounds is None or region is None:
        return bounds
    left, top, right, bottom = bounds
    x1, y1, x2, y2 = region
    if left >= x1 and top >= y1 and right <= x2 and bottom <= y2:
        return bounds
    return None


def _word_inside(word, region) -> bool:
    if region is None:
        return True
    left, top, width, height = word[1], word[2], word[3], word[4]
    return _bounds_inside((left, top, left + width, top + height), region) is not None


def _inverted_segment_search(path: Path, tesseract: str, label: str, region, deadline):
    """Last resort: re-read light segments inverted (dark on light grey, x4).

    Bounded: only segments inside ``region`` (or below the session toolbar
    when no region is given) are OCR'd, the search stops at the first
    matching segment, and it gives up at ``deadline``. Returns
    ``(bounds_or_None, segments_read)``.
    """
    width, height, channels, rows = _read_png(path)
    if region is not None:
        rx1, ry1, rx2, ry2 = region
    else:
        rx1, ry1, rx2, ry2 = 0, TOOLBAR_BOTTOM_Y, width, height
    thresh = _text_threshold(width, height, channels, rows)
    segments = []
    temporary = Path(tempfile.mkstemp(suffix=".png")[1])
    try:
        for y1, y2 in _visible_bands(width, height, channels, rows, thresh):
            if y2 < ry1 or y1 > ry2:
                continue
            for x1, x2 in _band_segments(width, rows, channels, thresh, y1, y2, 24):
                if x2 < rx1 or x1 > rx2:
                    continue
                if time.monotonic() >= deadline:
                    return None, segments
                box = (
                    min(width - 1, x1 + 3),
                    max(0, y1 - 4),
                    max(0, x2 - 3),
                    min(height - 1, y2 + 4),
                )
                if box[2] - box[0] < 24:
                    continue
                _upscaled_crop(rows, channels, box, 4, temporary, invert=True)
                words = _scaled_words(
                    _ocr_tsv(tesseract, temporary, "7"), (box[0], box[1]), 4,
                )
                if not words:
                    continue
                segments.append(words)
                bounds = match_visible_label(words, label)
                if bounds is None:
                    bounds = match_visible_segment([words], label)
                # A band that merely overlaps the region can still OCR the
                # status line below it. Skip that box and keep scanning.
                bounds = _bounds_inside(bounds, region)
                if bounds is not None:
                    return bounds, segments
    finally:
        temporary.unlink(missing_ok=True)
    return None, segments


def _region_band_label(path: Path, tesseract: str, label: str, region):
    """Re-read each light text band inside ``region``, one column at a time.

    A dock-body crop of a 13 px monospace name loses the trailing token
    (``444_GUI_DOWNLOAD`` plus a 2 px ``FIXTURE.``). The same glyphs as one
    row band, without the date column beside them, read as the filename.
    Narrow regions are already one column; wide regions are split on the
    dark gaps between columns. Returns a box wholly inside ``region``.
    """
    width, height, channels, rows = _read_png(path)
    x1, y1, x2, y2 = region
    thresh = _text_threshold(width, height, channels, rows)
    for by1, by2 in _visible_bands(width, height, channels, rows, thresh):
        if by2 < y1 or by1 >= y2:
            continue
        if x2 - x1 <= 360:
            spans = [(x1, x2)]
        else:
            spans = []
            for sx1, sx2 in _band_segments(width, rows, channels, thresh, by1, by2, 24):
                left = max(x1, sx1)
                right = min(x2, sx2 + 1)
                if right - left >= 24:
                    spans.append((left, right))
        for sx1, sx2 in spans:
            band = (
                max(0, sx1),
                max(0, by1 - 2),
                min(width, sx2),
                min(height, by2 + 3),
            )
            if band[2] - band[0] < 24 or band[3] - band[1] < 8:
                continue
            words = region_upscaled_rows(
                path, tesseract, band, psm="7", scale=4, invert=True,
            )
            words = [word for word in words if _word_inside(word, region)]
            bounds = match_visible_label(words, label)
            if bounds is None:
                bounds = match_visible_segment([words], label)
            bounds = _bounds_inside(bounds, region)
            if bounds is not None:
                return bounds
    return None


def locate_visible_label(
    path: Path,
    label: str,
    *,
    region=None,
    budget: float = LOCATE_BUDGET_S,
):
    """Return conservative interior bounds for one visibly rendered label.

    Iced does not expose these production controls through AT-SPI. The point
    is derived from the screenshot taken immediately before the click. Full
    window OCR is matched by joining same-line fragments, including the
    low-confidence filename split seen on Ubuntu tesseract 5.3.4. If that
    misses, each light text band is cropped from the pixels themselves and
    re-read. Returned coordinates are always in the source image.
    """
    tesseract = shutil.which("tesseract")
    if tesseract is None:
        raise RuntimeError("visible label targeting requires tesseract")
    started = time.monotonic()
    deadline = started + budget
    bounds = None
    first_words = []
    if region is not None:
        # Small monospace text reads far better upscaled; a crop is cheap.
        # Never skipped by the budget.
        for psm, invert in (("6", False), ("11", False), ("6", True)):
            words = region_upscaled_rows(
                path, tesseract, region, psm=psm, invert=invert,
            )
            if not first_words:
                first_words = words
            bounds = match_visible_label(words, label)
            if bounds is None:
                bounds = match_visible_segment([words], label)
            bounds = _bounds_inside(bounds, region)
            if bounds is not None:
                break
        # Full-width upscale still drops the trailing token of a 13 px name.
        # Row bands inside the region recover it; never a box outside region.
        if bounds is None:
            bounds = _region_band_label(path, tesseract, label, region)
    else:
        first_words = ocr_rows(path, tesseract, None)
        bounds = match_visible_label(first_words, label)
    if bounds is None and time.monotonic() < deadline:
        pixel_rows = visible_pixel_rows(path, tesseract, clusters=False)
        if region is not None:
            pixel_rows = [row for row in pixel_rows if _word_inside(row, region)]
        bounds = _bounds_inside(match_visible_label(pixel_rows, label), region)
    if bounds is None and time.monotonic() < deadline:
        clustered = visible_pixel_rows(path, tesseract, clusters=True)
        if region is not None:
            clustered = [row for row in clustered if _word_inside(row, region)]
        bounds = _bounds_inside(match_visible_label(clustered, label), region)
    if bounds is None and time.monotonic() < deadline:
        segments = visible_segment_rows(path, tesseract)
        if region is not None:
            segments = [
                [word for word in words if _word_inside(word, region)]
                for words in segments
            ]
            segments = [words for words in segments if words]
        flat = [word for words in segments for word in words]
        bounds = match_visible_label(flat, label)
        if bounds is None:
            bounds = match_visible_segment(segments, label)
        bounds = _bounds_inside(bounds, region)
    if bounds is None:
        bounds, inverted = _inverted_segment_search(
            path, tesseract, label, region,
            max(deadline, time.monotonic() + INVERTED_MIN_S),
        )
        bounds = _bounds_inside(bounds, region)
    else:
        inverted = []
    if bounds is None:
        seen = [row[0] for row in first_words][:40]
        segs = [" ".join(word[0] for word in words) for words in inverted][:20]
        print(
            f"OCR miss for {label!r} in {path} region={region} "
            f"after {time.monotonic() - started:.1f}s\n"
            f"  words: {seen}\n  inverted segments: {segs}",
            file=sys.stderr,
        )
        raise RuntimeError(
            f"visible label not found in screenshot: {label} "
            f"(screenshot: {path}, region={region})"
        )
    return bounds


# 13px monospace hit counter on the dark theme. Tesseract confuses these
# glyphs; the translated token must still be a slash-separated digit pair.
# Accepted: 0↔O/o (and @ for the right-hand zero observed on Linux),
# 1↔l/I/|, 2↔Z/z, 5↔S/s, 8↔B. A missing slash is a miss.
_FIND_COUNTER_DIGIT = str.maketrans({
    "O": "0", "o": "0", "@": "0",
    "l": "1", "I": "1", "|": "1",
    "Z": "2", "z": "2",
    "S": "5", "s": "5",
    "B": "8",
})
_FIND_COUNTER_RE = re.compile(
    r"^([0-9OoIlI|ZzSsB]{1,3})\s*/\s*([0-9OoIlI|ZzSsB]{1,3}|@)$"
)


def ocr_words_in_region(path: Path, region):
    """Return OCR words inside ``region`` without requiring a fixed label.

    Reuses ``locate_visible_label``'s region preprocessing: an x3 crop at
    psm 6, psm 11, and inverted psm 6. A psm 7 pass is added because the
    Find band is one 13px line and a full-width block can drop a
    3-character counter. Words are ``(text, x, y, width, height,
    confidence)`` in window coordinates. An empty list is a miss, not an
    exception.
    """
    tesseract = shutil.which("tesseract")
    if tesseract is None:
        raise RuntimeError("visible label targeting requires tesseract")
    words = []
    for psm, invert in (("6", False), ("11", False), ("6", True), ("7", False)):
        words.extend(region_upscaled_rows(
            path, tesseract, region, psm=psm, invert=invert,
        ))
    return words


def find_hit_counter(words):
    """Return ``(current, total, raw)`` for a slash-separated counter.

    A single OCR word wins. Otherwise a 2- or 3-token run on the same line
    (``1`` ``/`` ``5``, or ``1/`` ``5``). Does not join the whole band, so
    neighbouring labels cannot invent a pair. ``None`` if nothing matches.
    """
    def parse(token):
        match = _FIND_COUNTER_RE.fullmatch(token.strip())
        if match is None:
            return None
        left = match.group(1).translate(_FIND_COUNTER_DIGIT)
        right = match.group(2).translate(_FIND_COUNTER_DIGIT)
        if not left.isdigit() or not right.isdigit():
            return None
        return int(left), int(right), token.strip()

    for word in words:
        parsed = parse(word[0])
        if parsed is not None:
            return parsed
    for line in _group_lines(words):
        texts = [word[0] for word in line]
        for index in range(len(texts)):
            for span in (2, 3):
                if index + span > len(texts):
                    continue
                parsed = parse("".join(texts[index:index + span]))
                if parsed is not None:
                    return parsed
    return None


# Right-hand control cluster of the Find strip. 170px clips a shifted
# counter once Prev/Next become padded buttons; 260px still excludes the
# long input, whose outline otherwise owns the segmentation.
_STRIP_CLUSTER_PX = 260
_STRIP_SCALE = 4
_STRIP_PAD = 12
# 13px glyphs are shorter than a 1px button border or the input's vertical
# edge. Horizontal runs of this length are the input outline or a button's
# top and bottom, not a letter.
_STRIP_VERT_RULE = 18
_STRIP_HORIZ_RULE = 36
_STRIP_INK_MARGIN = 28


def match_strip_label(words, label, region=None):
    """Return bounds for an exact, case-sensitive strip label inside ``region``.

    ``words`` is ``[(text, bounds), ...]`` from ``read_strip_words``.
    ``Prey`` is not ``Prev`` and a box below the band is not a hit. These two
    labels have no confusion table: the single-line pass has to read them.
    """
    if not label:
        return None
    for text, bounds in words:
        if text != label:
            continue
        inside = _bounds_inside(bounds, region)
        if inside is not None:
            return inside
    return None


def strip_words_as_rows(words):
    """Adapt ``(text, bounds)`` words to the ``find_hit_counter`` row shape."""
    rows = []
    for text, bounds in words:
        left, top, right, bottom = bounds
        rows.append((
            text, left, top, max(1, right - left), max(1, bottom - top), 0,
        ))
    return rows


def _strip_luma(image):
    """Return ``(median, greyscale, dark_background)`` for one crop."""
    grey = image.convert("L")
    histogram = grey.histogram()
    total = sum(histogram)
    seen = 0
    median = 0
    for value, count in enumerate(histogram):
        seen += count
        if seen * 2 >= total:
            median = value
            break
    return median, grey, median < 128


def _erase_strip_rules(grey, median, dark_background):
    """Paint out ink runs longer than a glyph so 1px borders are not strokes.

    Measured on the dark Find strip: the input's blue edge and a button
    rectangle span most of the 44px search band, while Prev/Next glyphs are
    13px. Erasing those runs is what turns ``9/0`` / ``Hest`` into ``0/0``
    and ``Next``, and what keeps a closed button border from gluing onto
    the counter. Glyph columns are left alone.
    """
    width, height = grey.size
    pixels = grey.load()

    def ink(value):
        if dark_background:
            return value >= median + _STRIP_INK_MARGIN
        return value <= median - _STRIP_INK_MARGIN

    for x in range(width):
        y = 0
        while y < height:
            if not ink(pixels[x, y]):
                y += 1
                continue
            end = y + 1
            while end < height and ink(pixels[x, end]):
                end += 1
            if end - y >= _STRIP_VERT_RULE:
                for row in range(y, end):
                    pixels[x, row] = median
            y = end
    for y in range(height):
        x = 0
        while x < width:
            if not ink(pixels[x, y]):
                x += 1
                continue
            end = x + 1
            while end < width and ink(pixels[end, y]):
                end += 1
            if end - x >= _STRIP_HORIZ_RULE:
                for column in range(x, end):
                    pixels[column, y] = median
            x = end
    return grey


def _prepare_strip_cluster(crop):
    """Greyscale, invert a dark strip, drop rules, LANCZOS x4, autocontrast."""
    try:
        from PIL import Image, ImageOps
    except ImportError as exc:
        raise RuntimeError(
            "Find strip OCR requires Pillow (pip install Pillow==13.0.0; see scripts/local-gui/Dockerfile)"
        ) from exc

    median, grey, dark_background = _strip_luma(crop)
    cleaned = _erase_strip_rules(grey, median, dark_background)
    if dark_background:
        cleaned = ImageOps.invert(cleaned)
    lanczos = getattr(getattr(Image, "Resampling", Image), "LANCZOS")
    scaled = cleaned.resize(
        (cleaned.width * _STRIP_SCALE, cleaned.height * _STRIP_SCALE),
        lanczos,
    )
    scaled = ImageOps.autocontrast(scaled, cutoff=1)
    # After invert, both themes are dark text on a light ground.
    return ImageOps.expand(scaled, border=_STRIP_PAD, fill=255)


def _map_strip_box(word, origin, region):
    """Map one upscaled TSV word back into the source screenshot."""
    _text, left, top, width, height, _confidence = word
    origin_x, origin_y = origin
    x1 = origin_x + (left - _STRIP_PAD) / _STRIP_SCALE
    y1 = origin_y + (top - _STRIP_PAD) / _STRIP_SCALE
    x2 = x1 + width / _STRIP_SCALE
    y2 = y1 + height / _STRIP_SCALE
    rx1, ry1, rx2, ry2 = region
    if not (rx1 <= (x1 + x2) / 2 < rx2 and ry1 <= (y1 + y2) / 2 < ry2):
        return None
    left = max(rx1, min(rx2 - 1, int(round(x1))))
    top = max(ry1, min(ry2 - 1, int(round(y1))))
    right = max(left + 1, min(rx2, int(round(x2))))
    bottom = max(top + 1, min(ry2, int(round(y2))))
    return (left, top, right, bottom)


def _ocr_strip_pass(image, tesseract, psm):
    temporary = Path(tempfile.mkstemp(suffix=".png")[1])
    try:
        image.save(temporary)
        tsv = _ocr_tsv(tesseract, temporary, psm)
    finally:
        temporary.unlink(missing_ok=True)
    return parse_visible_words(tsv)


def _prefer_zero_counter(chosen, labelled):
    """Keep an empty counter as 0/0 when one pass misreads it as 0/9.

    On the bordered dark strip, PSM 7 reads the empty pair as ``0/9`` and
    PSM 11 reads ``0/0``. Use 0/0 only when every labelled pass has
    selected index 0. A hit counter (index 1 or 2), including a real 9
    next to a non-zero index, is left alone — the parser does not rewrite 9.
    """
    readings = []
    for mapped in labelled:
        token = next(
            ((text, bounds) for text, bounds in mapped if "/" in text),
            None,
        )
        if token is None:
            continue
        text, bounds = token
        left, top, right, bottom = bounds
        parsed = find_hit_counter([(
            text, left, top, max(1, right - left), max(1, bottom - top), 0,
        )])
        if parsed is not None:
            readings.append((parsed, text, bounds))
    if not readings or any(parsed[0] != 0 for parsed, _text, _bounds in readings):
        return chosen
    zero = next((item for item in readings if item[0][:2] == (0, 0)), None)
    if zero is None or not any(9 in item[0][:2] for item in readings):
        return chosen
    text, bounds = zero[1], zero[2]
    replaced = []
    swapped = False
    for old, box in chosen:
        if not swapped and "/" in old:
            replaced.append((text, bounds))
            swapped = True
        else:
            replaced.append((old, box))
    if not swapped:
        replaced.insert(0, (text, bounds))
    return replaced


def read_strip_words(shot_path, region):
    """Read one chrome strip. Returns ``[(text, bounds), ...]`` in source pixels.

    The Find strip is a single 13px line. Full-band PSM 6 reads ``Prev`` as
    ``Prey`` and ``Next`` as ``Hest`` because the input outline splits the
    line. This pass crops the right-hand 260px (counter, Prev, Next, close),
    inverts when the median pixel is dark, erases border-length rules, and
    upscales x4 with LANCZOS before ``--psm 7``. PSM 6 and 11 are tried when
    7 did not yield both labels, and also when 7's counter token contains a
    9, so an empty counter read as ``0/9`` can be replaced by PSM 11's
    ``0/0``. Words from a losing pass are not merged in, so a closed strip
    cannot accumulate a false Prev.
    """
    tesseract = shutil.which("tesseract")
    if tesseract is None:
        raise RuntimeError("visible label targeting requires tesseract")
    try:
        from PIL import Image
    except ImportError as exc:
        raise RuntimeError(
            "Find strip OCR requires Pillow (pip install Pillow==13.0.0; see scripts/local-gui/Dockerfile)"
        ) from exc
    image = Image.open(shot_path).convert("RGB")
    width, height = image.size
    x1, y1, x2, y2 = (int(edge) for edge in region)
    x1 = max(0, min(width - 1, x1))
    y1 = max(0, min(height - 1, y1))
    x2 = max(x1 + 1, min(width, x2))
    y2 = max(y1 + 1, min(height, y2))
    clamped = (x1, y1, x2, y2)
    band = image.crop(clamped)
    cluster = min(_STRIP_CLUSTER_PX, band.width)
    offset = band.width - cluster
    prepared = _prepare_strip_cluster(band.crop((offset, 0, band.width, band.height)))
    origin = (x1 + offset, y1)
    chosen = []
    best_hits = -1
    labelled = []
    for psm in ("7", "6", "11"):
        mapped = []
        for word in _ocr_strip_pass(prepared, tesseract, psm):
            bounds = _map_strip_box(word, origin, clamped)
            if bounds is None:
                continue
            mapped.append((word[0], bounds))
        hits = sum(text in ("Prev", "Next") for text, _bounds in mapped)
        if hits > best_hits:
            chosen = mapped
            best_hits = hits
        if hits == 2:
            labelled.append(mapped)
            token = next((text for text, _bounds in mapped if "/" in text), "")
            # A 9 in the counter is the empty-pair misread (0/9). Keep
            # going so PSM 11 can supply 0/0. Any other pair stops here.
            if "9" not in token:
                break
    if labelled:
        chosen = _prefer_zero_counter(labelled[0], labelled)
    return chosen


def find_strip_residue(words, find_band) -> dict:
    """Return bounds of surviving controls and parsed hit counter, or None each."""
    inside = [
        (text, bounds) for text, bounds in words
        if _bounds_inside(bounds, find_band) is not None
    ]
    return {
        "Prev": match_strip_label(inside, "Prev", find_band),
        "Next": match_strip_label(inside, "Next", find_band),
        "counter": find_hit_counter(strip_words_as_rows(inside)),
    }


def name_rows_region(first_row_bounds, row_count, *, row_height=23, name_right=None):
    """Name-column data-row box derived from the located first data row.

    The band does not move when the listing reorders. ``left`` covers the
    file icon inset from the glyph box; ``bottom`` covers ``row_count``
    rows of ``row_height``.
    """
    first_left, first_top = first_row_bounds[0], first_row_bounds[1]
    left = max(0, first_left - 36)
    top = first_top - 6
    right = name_right if name_right is not None else first_left + 270
    bottom = first_top + row_count * row_height + 2
    return (left, top, right, bottom)


def name_order_tops(shot, first_label, second_label, rows_region):
    """Return ``(top_first, top_second)`` only if both boxes are inside the rows.

    Locates both labels with ``region=rows_region``. A miss is ``None``, not
    an exception. A box that is not wholly inside ``rows_region`` — the
    transfer status line, for example — is also ``None``.
    """
    try:
        first = locate_visible_label(
            shot, first_label, region=rows_region, budget=LOCATE_BUDGET_S,
        )
        second = locate_visible_label(
            shot, second_label, region=rows_region, budget=LOCATE_BUDGET_S,
        )
    except RuntimeError:
        return None
    if _bounds_inside(first, rows_region) is None or _bounds_inside(second, rows_region) is None:
        return None
    return (first[1], second[1])


def center(bounds):
    return ((bounds[0] + bounds[2]) // 2, (bounds[1] + bounds[3]) // 2)


def diff(first: Path, second: Path, env: dict[str, str]) -> int:
    result = subprocess.run(
        ["compare", "-metric", "AE", str(first), str(second), "null:"],
        env=env, capture_output=True, text=True, timeout=15,
    )
    if result.returncode not in (0, 1):
        raise RuntimeError(f"ImageMagick screenshot comparison failed: {result.stderr[:200]}")
    # ImageMagick prints large counts in scientific notation ("1.024e+06")
    # and newer builds append "(fraction)"; keep the first token.
    token = result.stderr.strip().split()[0] if result.stderr.strip() else ""
    try:
        return int(float(token))
    except ValueError as exc:
        raise RuntimeError(
            f"ImageMagick screenshot comparison printed no pixel count: {result.stderr[:200]!r}"
        ) from exc


def band_changed_pixels(first: Path, second: Path, box) -> int:
    """Pixels in ``box`` whose RGB differs by more than 24 on any channel.

    ``_read_png`` returns ``(width, height, channels, rows)``. Each row is
    ``width * channels`` bytes (3 for RGB, 4 for RGBA). Alpha is ignored.
    """
    width1, height1, channels1, rows1 = _read_png(first)
    width2, height2, channels2, rows2 = _read_png(second)
    if (width1, height1) != (width2, height2):
        raise RuntimeError(
            f"band pixel compare size mismatch: {first.name} {width1}x{height1} "
            f"vs {second.name} {width2}x{height2}"
        )
    left, top, right, bottom = (int(edge) for edge in box)
    if (
        left < 0 or top < 0 or right > width1 or bottom > height1
        or right <= left or bottom <= top
    ):
        raise RuntimeError(
            f"band pixel compare box {box} is outside {width1}x{height1} "
            f"({first.name} vs {second.name})"
        )
    changed = 0
    for y in range(top, bottom):
        row1 = rows1[y]
        row2 = rows2[y]
        for x in range(left, right):
            i1 = x * channels1
            i2 = x * channels2
            if (
                abs(row1[i1] - row2[i2]) > 24
                or abs(row1[i1 + 1] - row2[i2 + 1]) > 24
                or abs(row1[i1 + 2] - row2[i2 + 2]) > 24
            ):
                changed += 1
    return changed


async def wait_find_strip_closed(
    app, window, open_frame: Path, closed_shot: Path, find_band, env,
    *, label: str = "Escape",
) -> tuple[int, list]:
    """Poll closed_shot until the Find strip is gone from find_band vs open_frame.

    open_frame is an OPEN-strip frame taken just before the closing action.
    Return (changed_pixels, words). Raise RuntimeError(f"{label} did not close
    the Find strip; ...") on deadline (8 * TIME_SCALE s) or app exit.
    """
    started = time.monotonic()
    deadline = started + 8 * TIME_SCALE
    changed_pixels = 0
    words = []
    while True:
        if app.poll() is not None:
            raise RuntimeError(
                f"{label} did not close the Find strip; GUI exited (code {app.returncode})"
            )
        await asyncio.to_thread(screenshot, window, closed_shot, env)
        changed_pixels = await asyncio.to_thread(
            band_changed_pixels, open_frame, closed_shot, find_band,
        )
        # A pre-repaint OCR call can consume the deadline; check pixels first.
        if changed_pixels >= 200:
            words = await asyncio.to_thread(read_strip_words, closed_shot, find_band)
            if all(value is None for value in find_strip_residue(words, find_band).values()):
                return changed_pixels, words
        if time.monotonic() >= deadline:
            # Include the last frame's words even if its band never moved.
            if changed_pixels < 200:
                words = await asyncio.to_thread(read_strip_words, closed_shot, find_band)
            residue = find_strip_residue(words, find_band)
            observed = [text for text, _bounds in words][:40]
            raise RuntimeError(
                f"{label} did not close the Find strip; "
                f"Prev={residue['Prev']} Next={residue['Next']} "
                f"counter={residue['counter']} in {find_band}; "
                f"band delta {changed_pixels} vs {open_frame.name} (need >= 200); "
                f"words={observed}; elapsed {time.monotonic() - started:.1f}s"
            )
        await asyncio.sleep(.3)


async def locate_rendered_label(
    window: str, path: Path, label: str, *, env: dict[str, str], timeout: float = 22,
    region=None,
):
    """Screenshot repeatedly until the label is rendered; return its bounds."""
    timeout *= TIME_SCALE
    # OCR and screenshots block for seconds. The fixture SSH server shares
    # this event loop, so run them off-loop or the GUI's sftp connections
    # time out during banner exchange while we are looking at the screen.
    deadline = time.monotonic() + timeout
    while True:
        await asyncio.to_thread(screenshot, window, path, env)
        try:
            return await asyncio.to_thread(
                locate_visible_label, path, label, region=region,
            )
        except RuntimeError:
            if time.monotonic() >= deadline:
                raise
            await asyncio.sleep(.5)


async def click_label(
    window: str, shot_path: Path, label: str, env: dict[str, str], *,
    region=None, timeout: float = 22, settle: float = .5, clicks: int = 1,
):
    """Screenshot until ``label`` is rendered (optionally inside ``region``),
    click its centre with a native pointer event and return its bounds.

    ``clicks`` is the number of left-button clicks. Session rows open on a
    double click (``clicks=2``); every other caller keeps the single click.
    Screenshot and OCR run off the event loop (the fixture SSH server shares
    it), exactly like ``locate_rendered_label``.
    """
    timeout *= TIME_SCALE
    deadline = time.monotonic() + timeout
    while True:
        await asyncio.to_thread(screenshot, window, shot_path, env)
        try:
            bounds = await asyncio.to_thread(
                locate_visible_label, shot_path, label, region=region,
            )
            break
        except RuntimeError:
            if time.monotonic() >= deadline:
                raise
            await asyncio.sleep(.5)
    click = ["click", "1"] if clicks == 1 else [
        "click", "--repeat", str(clicks), "--delay", "120", "1",
    ]
    command(
        "xdotool", "mousemove", "--window", window,
        *map(str, center(bounds)), *click, env=env,
    )
    await asyncio.sleep(settle)
    return bounds


def window_size(window: str, env: dict[str, str]) -> tuple[int, int]:
    geometry = command("xdotool", "getwindowgeometry", "--shell", window, env=env)
    fields = dict(
        line.split("=", 1) for line in geometry.splitlines() if "=" in line
    )
    return int(fields["WIDTH"]), int(fields["HEIGHT"])


def dock_strip_region(width: int, height: int):
    """Bottom dock tab strip (Session/Transfer/Filer/Explorer) search box.

    The bottom dock occupies roughly the lower 30% of the window; its strip
    sits at y~538..567 for an 800px window. Search a generous band so OCR of
    a tab label never matches the centre toolbar's 'Files' button.
    """
    return (0, int(height * .62), width, int(height * .76))


SIDE_FILE_DOCK_WIDTH = 420


def dock_body_region(
    width: int, height: int, *, right: bool = False, bottom_top: int | None = None,
):
    """Dock body (file rows, Upload/Download, Transfer queue rows).

    Bottom dock: full width, lower third above the status bar. Right dock:
    a file panel moved into a side dock widens it to 420px, so search
    x = width - 420 - 4 .. width, from below the 32px menu bar down to the
    top of the bottom dock (``bottom_top``, from the OCR-located bottom
    strip when available, else 0.66 * height).
    """
    if right:
        lower = bottom_top if bottom_top is not None else int(height * .66)
        return (width - SIDE_FILE_DOCK_WIDTH - 4, 32, width, lower)
    return (0, int(height * .66), width, height - 22)


async def drag_horizontal(
    window: str, env: dict[str, str], start_x: int, end_x: int, y: int, *, steps: int = 6,
) -> None:
    """Native left-button drag: press at start, move in steps, release."""
    command(
        "xdotool", "mousemove", "--window", window, str(start_x), str(y), env=env,
    )
    await asyncio.sleep(.2)
    command("xdotool", "mousedown", "1", env=env)
    await asyncio.sleep(.2)
    for i in range(1, steps + 1):
        x = start_x + (end_x - start_x) * i // steps
        command(
            "xdotool", "mousemove", "--window", window, str(x), str(y), env=env,
        )
        await asyncio.sleep(.08)
    await asyncio.sleep(.2)
    command("xdotool", "mouseup", "1", env=env)
    await asyncio.sleep(.6)


def gear_menu_region(width: int, height: int):
    """Dock gear menu rows (Move to Right / Move to Bottom): right 30%."""
    return (int(width * .70), 0, width, height)


async def wait_for(predicate, label: str, *, timeout: float = 22) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise RuntimeError(f"timeout waiting for {label}")
        await asyncio.sleep(.2)


async def click_filer_refresh(
    window: str, env: dict[str, str], shot_path: Path, *,
    path_bar_y: int, completed, label: str, attempt_timeout: float = 6,
) -> None:
    """Click the Filer path bar's text-less refresh icon until ``completed``.

    Switching between the Explorer and Filer dock tabs does NOT reload the
    remote listing (panels only re-list when going hidden -> visible), so the
    refresh icon must be clicked. Measured at 1280x800 with a bottom dock:
    tab labels y~552, path bar y=567..598; at its right end 'Up' ~(1185,584),
    refresh icon (1222,584) with a ~22x22 hit box, 'Go' (1256,584) spanning
    x 1238..1274. So the icon is at x = width - 58, y = tab-label y + 32. The
    path bar's right end stays at the window's right edge for a right dock
    (strip y~46, so path bar y~78). Retry at x-8 then x+8 before raising.
    """
    width, _ = window_size(window, env)
    base_x = width - 58
    offsets = (0, -8, 8)
    for index, offset in enumerate(offsets):
        command(
            "xdotool", "mousemove", "--window", window,
            str(base_x + offset), str(path_bar_y), "click", "1", env=env,
        )
        await asyncio.sleep(.3)
        screenshot(window, shot_path, env)
        last = index == len(offsets) - 1
        try:
            await wait_for(completed, label, timeout=22 if last else attempt_timeout)
            return
        except RuntimeError:
            if last:
                raise


async def click_transfer(
    *,
    window: str,
    env: dict[str, str],
    row: tuple[int, int],
    action: tuple[int, int],
    completed,
    label: str,
    selected_screenshot: Path | None = None,
) -> None:
    """Select one visibly rendered row and activate one production Iced button."""
    command(
        "xdotool", "mousemove", "--window", window,
        str(row[0]), str(row[1]), "click", "1", env=env,
    )
    await asyncio.sleep(.25)
    if selected_screenshot is not None:
        screenshot(window, selected_screenshot, env)
    command(
        "xdotool", "mousemove", "--window", window,
        str(action[0]), str(action[1]), "click", "1", env=env,
    )
    await wait_for(completed, label)


def reserved_display() -> str:
    for value in range(160, 230):
        if not Path(f"/tmp/.X11-unix/X{value}").exists() and not Path(f"/tmp/.X{value}-lock").exists():
            return f":{value}"
    raise RuntimeError("no disposable Xvfb display number available")


def danger_fill_run(path: Path, y: int, x1: int, x2: int, *, min_width: int = 30):
    """Horizontal extent of the danger button fill on row ``y``.

    The label's dark glyphs interrupt the fill on its text rows, so this is
    the span from the first to the last strongly red pixel between ``x1`` and
    ``x2``. Returns ``(left, right)`` in image coordinates, or ``None`` when
    fewer than ``min_width`` such pixels are on the row.
    """
    width, height, channels, rows = _read_png(path)
    if not 0 <= y < height:
        return None
    row = rows[y]
    red = [
        x for x in range(max(0, x1), min(width, x2))
        if row[x * channels] >= 190
        and row[x * channels + 1] <= 130
        and row[x * channels + 2] <= 130
    ]
    if len(red) < min_width:
        return None
    return (red[0], red[-1] + 1)


def request_window_close(window: str, env: dict[str, str]) -> None:
    """Send the ICCCM close a window manager's close button sends.

    ClientMessage type WM_PROTOCOLS, format 32, data.l[0] = WM_DELETE_WINDOW,
    data.l[1] = CurrentTime, delivered to the window itself with
    XSendEvent(display, window, False, NoEventMask, &event) then XFlush.

    ctypes, not xdotool: the runners' xdotool is 3.20160805 (no windowquit;
    windowclose and windowkill destroy the window instead of asking), and no
    window manager runs under Xvfb so wmctrl cannot deliver this either.
    """
    import ctypes
    import ctypes.util

    try:
        protocols = command("xprop", "-id", window, "WM_PROTOCOLS", env=env)
    except subprocess.CalledProcessError as exc:
        detail = (exc.stderr or exc.stdout or "").strip()
        raise RuntimeError(
            f"xprop WM_PROTOCOLS failed for window {window}: {detail[:300]}"
        ) from exc
    if "WM_DELETE_WINDOW" not in protocols:
        raise RuntimeError(
            f"window {window} does not advertise WM_DELETE_WINDOW: {protocols!r}"
        )
    lib_name = ctypes.util.find_library("X11")
    if not lib_name:
        raise RuntimeError("libX11 not found; cannot send WM_DELETE_WINDOW")
    x11 = ctypes.CDLL(lib_name)

    class _XClientMessageData(ctypes.Union):
        _fields_ = [
            ("b", ctypes.c_char * 20),
            ("s", ctypes.c_short * 10),
            ("l", ctypes.c_long * 5),
        ]

    class _XClientMessageEvent(ctypes.Structure):
        # LP64 layout: int, pad, ulong, int, pad, pointer, ulong, ulong, int,
        # pad, then the 5-long data union. Window and Atom are unsigned long.
        _fields_ = [
            ("type", ctypes.c_int),
            ("serial", ctypes.c_ulong),
            ("send_event", ctypes.c_int),
            ("display", ctypes.c_void_p),
            ("window", ctypes.c_ulong),
            ("message_type", ctypes.c_ulong),
            ("format", ctypes.c_int),
            ("data", _XClientMessageData),
        ]

    class _XEvent(ctypes.Union):
        # XSendEvent copies sizeof(XEvent), which is 24 longs. A bare
        # XClientMessageEvent is shorter; the pad makes the union that size.
        _fields_ = [
            ("xclient", _XClientMessageEvent),
            ("pad", ctypes.c_long * 24),
        ]

    if ctypes.sizeof(_XEvent) != 24 * ctypes.sizeof(ctypes.c_long):
        raise RuntimeError(
            f"XEvent is {ctypes.sizeof(_XEvent)} bytes, expected 24 longs"
        )

    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = ctypes.c_void_p
    x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    x11.XCloseDisplay.restype = ctypes.c_int
    x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XSendEvent.argtypes = [
        ctypes.c_void_p,
        ctypes.c_ulong,
        ctypes.c_int,
        ctypes.c_long,
        ctypes.POINTER(_XEvent),
    ]
    x11.XSendEvent.restype = ctypes.c_int
    x11.XFlush.argtypes = [ctypes.c_void_p]
    x11.XFlush.restype = ctypes.c_int

    display_name = env.get("DISPLAY")
    if not display_name:
        raise RuntimeError("DISPLAY is unset; cannot send WM_DELETE_WINDOW")
    display = x11.XOpenDisplay(display_name.encode())
    if not display:
        raise RuntimeError(f"XOpenDisplay failed for {display_name!r}")
    try:
        wm_protocols = x11.XInternAtom(display, b"WM_PROTOCOLS", False)
        wm_delete = x11.XInternAtom(display, b"WM_DELETE_WINDOW", False)
        if not wm_protocols or not wm_delete:
            raise RuntimeError(
                "failed to intern WM_PROTOCOLS or WM_DELETE_WINDOW"
            )
        # Nested union assignment through a parent writes a temporary and
        # would not reach XSendEvent. Build each value, then assign it whole.
        data = _XClientMessageData()
        data.l[0] = wm_delete
        data.l[1] = 0  # CurrentTime
        client = _XClientMessageEvent()
        client.type = 33  # ClientMessage
        client.window = int(window)
        client.message_type = wm_protocols
        client.format = 32
        client.data = data
        event = _XEvent()
        event.xclient = client
        # NoEventMask, propagate False: the creating client receives it.
        # XSendEvent itself sets send_event, which is what a real WM does.
        sent = x11.XSendEvent(
            display, int(window), False, 0, ctypes.byref(event),
        )
        if sent == 0:
            raise RuntimeError(
                f"XSendEvent WM_DELETE_WINDOW to {window} returned 0"
            )
        x11.XFlush(display)
    finally:
        x11.XCloseDisplay(display)


TOUR = os.environ.get("INSPIRUM_GUI_TOUR") == "1"


def sync_badge_labels_visible(path: Path, tesseract: str, expected: str) -> bool:
    """Require both full labels on one line inside the 640px status badge.

    Full-width OCR treats the 240px badge outline as a glyph. Crop to the
    badge and reject OCR boxes touching its border or outside the badge.
    """
    badge = (60, 455, 300, 480)
    interior = (62, 461, 298, 477)
    for psm, invert in (("6", False), ("11", False), ("6", True)):
        words = region_upscaled_rows(path, tesseract, badge, psm=psm, invert=invert)
        words = [(w[0].upper(), *w[1:]) for w in words if _word_inside(w, interior)]
        count = match_visible_label(words, "2 SELECTED")
        state = match_visible_label(words, expected)
        if count and state and count[2] < state[0] \
                and count[1] < state[3] and state[1] < count[3] \
                and _bounds_inside(count, interior) and _bounds_inside(state, interior):
            return True
    return False


async def run_tour(window: str, tour_dir: Path, env: dict[str, str], events, app_log: Path,
                   *, forward_ports: tuple[int, ...] = ()) -> int:
    """LIGHT-theme screenshot tour (INSPIRUM_GUI_TOUR=1).

    The window, a real SSH session, and the terminal echo probe are required.
    Every other step logs a warning and continues. Coordinates assume the
    1280x800 layout.
    """
    tour_dir.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []

    def log_text() -> str:
        return events.path.read_text(encoding="utf-8")

    def key(*keys: str) -> None:
        command("xdotool", "windowfocus", window, "key", "--clearmodifiers", *keys, env=env)

    def click(x: int, y: int, clicks: int = 1) -> None:
        args = ["xdotool", "mousemove", "--window", window, str(x), str(y), "click"]
        if clicks != 1:
            args.extend(["--repeat", str(clicks), "--delay", "120"])
        args.append("1")
        command(*args, env=env)

    def shot(name: str) -> None:
        path = tour_dir / name
        screenshot(window, path, env)
        written.append(path)
        print(f"tour: wrote {path}", flush=True)

    async def step(label: str, coro_fn) -> None:
        try:
            await coro_fn()
        except Exception as error:  # best-effort tour
            print(f"tour: WARNING step {label!r} failed: {error}", flush=True)

    width, height = 1280, 800
    try:
        width, height = window_size(window, env)
    except Exception as error:
        print(f"tour: WARNING window_size failed ({error}); assuming 1280x800", flush=True)
    strip_y = 552

    await asyncio.sleep(2)

    async def s01():
        shot("tour-01-start.png")
    await step("start", s01)

    async def s02():
        click(30, 16)
        await asyncio.sleep(1)
        shot("tour-02-menu-session.png")
        key("Escape")
        await asyncio.sleep(.5)
    await step("menu-session", s02)

    async def s03():
        # Session(7)=71, Edit(4)=47.2, View starts ~118, width ~47 -> centre ~142.
        click(142, 16)
        await asyncio.sleep(1)
        shot("tour-03-menu-view.png")
        key("Escape")
        await asyncio.sleep(.5)
    await step("menu-view", s03)

    async def tour_label(path: Path, label: str, region=None):
        """Probe a saved frame only; never replace its evidence on an OCR miss."""
        try:
            return await asyncio.to_thread(
                locate_visible_label, path, label, region=region,
            )
        except RuntimeError as error:
            print(f"tour: WARNING {path.name}: {label!r} OCR miss: {error}", flush=True)
            return None

    def centre(box):
        return ((box[0] + box[2]) // 2, (box[1] + box[3]) // 2)

    async def resize_tour(w: int, h: int) -> tuple[int, int]:
        command("xdotool", "windowsize", window, str(w), str(h), env=env)
        await asyncio.sleep(1)
        actual = window_size(window, env)
        print(f"tour: requested window {w}x{h}; actual {actual[0]}x{actual[1]}", flush=True)
        return actual

    async def s04():
        # The 1280x800 fallback is measured from production-new-connection.png:
        # Advanced control [353,549,552,572]. Prefer its live OCR box.
        if window_size(window, env) != (1280, 800):
            raise RuntimeError("tour-04 baseline requires actual 1280x800 window")
        key("ctrl+n")
        try:
            await asyncio.sleep(1.5)
            shot("tour-04-new-connection.png")
            collapsed = tour_dir / "tour-04-new-connection.png"
            box = await tour_label(collapsed, "Advanced SSH options")
            if box:
                click(*centre(box))
            elif window_size(window, env) == (1280, 800):
                click(452, 560)
            else:
                raise RuntimeError("Advanced control not located at non-baseline size")
            await asyncio.sleep(1)
            shot("tour-04a-advanced-ssh-1280x800.png")
        finally:
            key("Escape")
            await asyncio.sleep(.5)
    await step("new-connection", s04)

    async def s04_min():
        try:
            min_width, min_height = await resize_tour(640, 480)
            size_tag = f"{min_width}x{min_height}"
            # iced may clamp the requested size to its configured 960x640.
            # Every artifact name records the *observed* native dimensions.
            key("ctrl+n")
            try:
                await asyncio.sleep(1)
                collapsed = tour_dir / f"tour-04b-connection-{size_tag}.png"
                shot(collapsed.name)
                box = await tour_label(collapsed, "Advanced SSH options")
                # At minimum height the disclosure may be below the fold.
                # Scroll inside the dialog to find it, never use (452,560).
                form_x, form_y = min_width // 2, min_height * 3 // 4
                for _ in range(6):
                    if box:
                        break
                    command("xdotool", "mousemove", "--window", window,
                            str(form_x), str(form_y), "click", "5", env=env)
                    await asyncio.sleep(.3)
                    probe = tour_dir / "tour-04-min-locator.png"
                    await asyncio.to_thread(screenshot, window, probe, env)
                    box = await tour_label(probe, "Advanced SSH options")
                if box:
                    click(*centre(box))
                    await asyncio.sleep(.8)
                else:
                    print("tour: WARNING minimum-size advanced control not found", flush=True)
                # Return to the top so the selected authentication controls
                # remain in the unscrolled expanded frame.
                for _ in range(8):
                    command("xdotool", "mousemove", "--window", window,
                            str(form_x), str(form_y), "click", "4", env=env)
                await asyncio.sleep(.7)
                shot(f"tour-04c-advanced-ssh-{size_tag}.png")
                for _ in range(6):
                    command("xdotool", "mousemove", "--window", window,
                            str(form_x), str(form_y), "click", "5", env=env)
                await asyncio.sleep(.7)
                shot(f"tour-04d-advanced-ssh-scrolled-{size_tag}.png")
                # The six-wheel frame shows proxy controls, not the footer.
                # Drive to the end and keep a separate frame as evidence.
                for _ in range(30):
                    command("xdotool", "mousemove", "--window", window,
                            str(form_x), str(form_y), "click", "5", env=env)
                await asyncio.sleep(1)
                bottom = tour_dir / f"tour-04e-advanced-ssh-bottom-{size_tag}.png"
                shot(bottom.name)
                # Only check dark-on-light footer buttons in the bottom band.
                # A miss is inconclusive; do not assert scroll reachability.
                footer_region = (0, max(0, min_height - 160), min_width, min_height)
                try:
                    tesseract = shutil.which("tesseract")
                    if tesseract is None:
                        raise RuntimeError("tesseract unavailable")
                    footer = None
                    for psm in ("6", "11"):
                        words = await asyncio.to_thread(
                            region_upscaled_rows, bottom, tesseract, footer_region,
                            psm=psm,
                        )
                        for label in ("Cancel", "Save only"):
                            footer = _bounds_inside(
                                match_visible_label(words, label), footer_region,
                            )
                            if footer:
                                break
                        if footer:
                            break
                    if footer:
                        print(f"tour: minimum-size footer label visible at {footer}", flush=True)
                    else:
                        print(f"tour: WARNING minimum-size footer not detected in {bottom.name}", flush=True)
                except Exception as error:  # best-effort visual evidence
                    print(f"tour: WARNING minimum-size footer check failed in {bottom.name}: {error}", flush=True)
            finally:
                key("Escape")
                await asyncio.sleep(.5)
        finally:
            restored = await resize_tour(1280, 800)
            if restored != (1280, 800):
                raise RuntimeError(f"tour baseline not restored: {restored}")
    await step("new-connection-minimum", s04_min)

    # Steps 4-5 are mandatory: a real SSH session must start.
    if "SESSION_STARTED" not in log_text():
        for y in (630, 653, 612, 676, 699):
            click(200, y, clicks=2)
            await asyncio.sleep(1.5)
            if "SESSION_STARTED" in log_text():
                break
            key("Escape")
    if "SESSION_STARTED" not in log_text():
        print(
            "tour: FAILED double-clicking the saved profile never started a real SSH session. "
            "SSH events: " + repr(log_text()[-800:])
            + "; application log tail: " + app_log.read_text(encoding="utf-8")[-1800:],
            flush=True,
        )
        return 1
    await asyncio.sleep(2)
    shot("tour-05-terminal.png")

    async def s05_tunnels():
        # Only target the top-right control if its live label is there.
        before = tour_dir / "tour-05-terminal.png"
        box = await tour_label(before, "Tunnels", region=(width - 180, 0, width, 42))
        if not box:
            raise RuntimeError("top-right Tunnels button not visible")
        click(*centre(box))
        try:
            await asyncio.sleep(1)
            shot("tour-05a-tunnels-open.png")
            # Top Tunnels opens a quick popup; its footer opens the manager.
            popup = (max(0, width - 340), 32, width, min(height, 400))
            settings = await tour_label(
                tour_dir / "tour-05a-tunnels-open.png", "Tunnel Settings...",
                region=popup,
            )
            if not settings:
                raise RuntimeError("Tunnel Settings... not visible in tunnels popup")
            click(*centre(settings))
            await asyncio.sleep(1)
            shot("tour-05b-tunnel-settings.png")
            await tour_label(tour_dir / "tour-05b-tunnel-settings.png", "Tunnel Manager")
        finally:
            key("Escape")
            await asyncio.sleep(.5)
            # Escape closes the manager; focus remains with the connected
            # terminal. The mandatory echo below proves input routing.
    await step("tunnels-manager", s05_tunnels)

    async def s05_tunnels_min() -> None:
        """Populated LIGHT Tunnel Manager at the minimum window size.

        Real saved forwards (INSPIRUM_GUI_POPULATED=1): the popup and the
        manager list must show both labels, and selecting Alpha must put its
        own listen port in the Listen Port row with Bravo's absent. All OCR
        is dark-on-light (light_locate), never the dark-theme pipeline.
        """
        min_w, min_h = await resize_tour(*MIN_WINDOW)
        try:
            tag = require_min_window((min_w, min_h))
            await asyncio.sleep(.8)
            base = tour_dir / f"tour-05c-terminal-{tag}.png"
            shot(base.name)
            box = await asyncio.to_thread(
                light_locate, base, "Tunnels", region=(max(0, min_w - 220), 0, min_w, 42),
            )
            click(*centre(box))
            await asyncio.sleep(1)
            popup_shot = tour_dir / f"tour-05d-tunnels-popup-{tag}.png"
            shot(popup_shot.name)
            popup = (max(0, min_w - 440), 32, min_w, min_h)
            settings = await asyncio.to_thread(
                light_locate, popup_shot, "Tunnel Settings...", region=popup,
            )
            if len(forward_ports) != 2:
                raise RuntimeError(f"populated tour needs two forwards, got {forward_ports}")
            # Each row: its exact first word plus its own listen port in the
            # same row band, above the footer (populated_profile_ssh pairs
            # POPULATED_LABELS[i] with forward_ports[i]).
            for idx, label in enumerate(POPULATED_LABELS):
                row = await asyncio.to_thread(
                    light_popup_row, popup_shot, label.split()[0], forward_ports[idx],
                    forward_ports[1 - idx], region=popup, footer_top=settings[1],
                )
                print(f"tour: {tag} popup row {label} port {forward_ports[idx]} at {row}",
                      flush=True)
            click(*centre(settings))
            await asyncio.sleep(1.2)
            manager = tour_dir / f"tour-05e-tunnel-manager-{tag}.png"
            shot(manager.name)
            heading = await asyncio.to_thread(
                light_locate, manager, "Tunnel Manager", region=(0, 42, min_w, min_h // 2),
            )
            left, _dialog = tunnel_dialog_left(min_w)
            list_card = (left, heading[3], left + TUNNEL_EDITOR_CARD_DX - 6, min_h)
            rows = []
            for label in POPULATED_LABELS:
                rows.append(await asyncio.to_thread(
                    light_locate, manager, label, region=list_card,
                ))
            print(f"tour: {tag} Tunnel Manager heading {heading} rows {rows}", flush=True)
            if len(forward_ports) == 2:
                click(*centre(rows[0]))
                await asyncio.sleep(1)
                selected = tour_dir / f"tour-05f-tunnel-manager-alpha-{tag}.png"
                shot(selected.name)
                port_box = await asyncio.to_thread(
                    require_editor_selection, selected, forward_ports[0], forward_ports[1],
                    editor_form_region(min_w, min_h),
                    row_region=listen_port_row_region(heading, min_w),
                    locate=light_locate,
                )
                print(f"tour: {tag} Alpha listen port {forward_ports[0]} at {port_box}",
                      flush=True)
        finally:
            # One Escape closes the popup or the manager. A second would
            # reach the shell and corrupt the mandatory echo probe line.
            key("Escape")
            await asyncio.sleep(.5)
            restored = await resize_tour(1280, 800)
            if restored != (1280, 800):
                raise RuntimeError(f"tour baseline not restored: {restored}")

    if POPULATED:
        # Required in a populated tour: it exists for this evidence.
        try:
            await s05_tunnels_min()
        except Exception as error:
            print(f"tour: FAILED populated minimum Tunnel Manager: {error}", flush=True)
            return 1
        print(
            f"tour: PASS populated LIGHT Tunnel Manager at minimum size "
            f"{MIN_WINDOW[0]}x{MIN_WINDOW[1]}", flush=True,
        )

    # Required: a missed echo fails the tour. INSPIRUM_GUI_TOUR_WITHHOLD_ECHO=1
    # makes the fixture stay silent so this path can be proven.
    command("xdotool", "windowfocus", window, env=env)
    probe_typed_at = time.time()
    command("xdotool", "type", "--delay", "20", "echo:TOUR_PROBE", env=env)
    key("Return")
    try:
        await wait_for(
            lambda: "NATIVE_ECHO:TOUR_PROBE" in log_text().splitlines(),
            "tour shell answered NATIVE_ECHO:TOUR_PROBE",
        )
    except RuntimeError:
        print(
            "tour: FAILED terminal did not answer echo:TOUR_PROBE "
            + log_text()[-800:],
            flush=True,
        )
        return 1
    await asyncio.sleep(1)
    echo_shot = tour_dir / "tour-06-terminal-output.png"
    if echo_shot.exists():
        echo_shot.unlink()
    shot("tour-06-terminal-output.png")
    if not echo_shot.is_file() or echo_shot.stat().st_mtime <= probe_typed_at:
        print(
            "tour: FAILED tour-06-terminal-output.png missing or not newer than the echo probe",
            flush=True,
        )
        return 1

    async def s07():
        key("ctrl+shift+r")
        await asyncio.sleep(2)
        shot("tour-07-split.png")
    await step("split", s07)

    async def s07b():
        # Best-effort Find strip. Escape in finally so a failed shot cannot
        # leave the strip open for the Filer step.
        key("ctrl+shift+f")
        try:
            await asyncio.sleep(1)
            command(
                "xdotool", "windowfocus", window, "type", "--delay", "20",
                "NATIVE", env=env,
            )
            await asyncio.sleep(1)
            shot("tour-07b-find.png")
        finally:
            key("Escape")
            await asyncio.sleep(.5)
    await step("find", s07b)

    async def s08():
        key("ctrl+shift+o")
        await wait_for(lambda: "SFTP_LIST_READY" in log_text(), "remote SFTP listing")
        await asyncio.sleep(1.5)
        shot("tour-08-filer.png")
    await step("filer", s08)

    # Bottom dock tab centres (Sender 6-88, Session 92-182, Transfer 186-283,
    # Filer ~287-361, Explorer ~365-463).
    for x, name in ((414, "tour-09-explorer.png"), (234, "tour-10-transfer.png"),
                    (47, "tour-11-sender.png"), (137, "tour-12-session.png"),
                    (324, None)):
        async def tab(x=x, name=name):
            click(x, strip_y)
            await asyncio.sleep(1.5)
            if name:
                shot(name)
        await step(name or "filer-again", tab)

    async def s13():
        click(width - 40, strip_y)
        await asyncio.sleep(1)
        shot("tour-13-gear-menu.png")
        key("Escape")
        await asyncio.sleep(.5)
    await step("gear-menu", s13)

    async def s14():
        key("ctrl+comma")
        await asyncio.sleep(1.5)
        shot("tour-14-tools.png")
        key("Escape")
        await asyncio.sleep(.5)
    await step("tools", s14)

    # Optional hard gate: do not put this in step(), which swallows failures.
    if os.environ.get("INSPIRUM_GUI_SYNC_BADGE_PROBE") == "1":
        original_size = window_size(window, env)
        armed = False
        try:
            tesseract = shutil.which("tesseract")
            if tesseract is None:
                raise RuntimeError("sync badge probe requires tesseract")
            if original_size != (1280, 800):
                raise RuntimeError(f"sync badge probe requires 1280x800 setup: {original_size}")
            key("ctrl+shift+r")  # Tour already split once; focus is in pane B.
            await asyncio.sleep(2)
            menu_frame = tour_dir / "tour-sync-workspace-menu.png"
            click(245, 16)  # Window spans x212..275, per menu.rs title widths.
            await asyncio.sleep(.5)
            screenshot(window, menu_frame, env)
            menu_box = await asyncio.to_thread(
                locate_visible_label, menu_frame, "Workspace",
                region=(210, 24, 455, 400),
            )
            if menu_box is None:
                raise RuntimeError("Window menu Workspace entry not visible")
            click(*centre(menu_box))
            await asyncio.sleep(1)
            panel_frame = tour_dir / "tour-sync-workspace-targets.png"
            screenshot(window, panel_frame, env)
            # At 1280x800 the controls are small light text near the bottom of
            # the panel. Whole-window OCR drops them; upscale this panel slice.
            targets_region = (205, 560, 900, 680)
            rows = await asyncio.to_thread(
                region_upscaled_rows, panel_frame, tesseract, targets_region, psm="6",
            )
            header_box = match_visible_label(rows, "Synchronized input targets")
            if header_box is None:
                raise RuntimeError("Workspace synchronized-input controls not visible")
            # The checkboxes precede each profile name. Distinguish the three
            # identical names by their 'pane <id>' suffix and vertical order.
            pane_rows = []
            for line in _group_lines(rows):
                text = " ".join(word[0] for word in line).lower()
                match = re.search(r"\bpane\s*([123])\b", text)
                if match:
                    left = min(word[1] for word in line)
                    y = (min(word[2] for word in line) + max(word[2] + word[4] for word in line)) // 2
                    if y > header_box[3] and left > targets_region[0]:
                        pane_rows.append((y, left, int(match.group(1))))
            pane_rows.sort()
            if len(pane_rows) != 3 or [pane for _, _, pane in pane_rows] != [1, 2, 3]:
                raise RuntimeError(f"expected three visible Workspace pane rows, got {pane_rows}")
            for y, left, _ in pane_rows[:2]:
                click(left - 16, y)  # Iced checkbox before its label.
                await asyncio.sleep(.4)
            # Checkbox changes can redraw the panel; target ARM from fresh pixels.
            arm_frame = tour_dir / "tour-sync-workspace-arm.png"
            screenshot(window, arm_frame, env)
            arm_rows = await asyncio.to_thread(
                region_upscaled_rows, arm_frame, tesseract, targets_region, psm="6",
            )
            arm_box = match_visible_label(arm_rows, "ARM synchronized input")
            if arm_box is None or arm_box[1] <= pane_rows[-1][0]:
                raise RuntimeError("ARM synchronized input checkbox not visible below targets")
            click(arm_box[0] - 20, centre(arm_box)[1])
            armed = True
            await asyncio.sleep(.5)
            key("Escape")
            if await resize_tour(640, 480) != (640, 480):
                raise RuntimeError("sync badge probe was not rendered at actual 640x480")

            async def badge_frame(name: str, expected: str, x: int) -> None:
                click(x, 160)  # Terminal canvas, clear of toolbar and footer.
                await asyncio.sleep(.8)
                path = tour_dir / name
                shot(name)
                crop = tour_dir / name.replace(".png", "-status.png")
                await asyncio.to_thread(_region_crop_upscaled, path, (0, 440, 640, 480), 3, crop)
                written.append(crop)
                if not await asyncio.to_thread(sync_badge_labels_visible, path, tesseract, expected):
                    raise RuntimeError(
                        f"{name}: badge missing full 2 SELECTED / {expected} "
                        "inside 640px status badge"
                    )
                print(f"tour: {name}: visible 2 SELECTED and {expected}", flush=True)

            # Newly split C occupies the right edge; A occupies the left half
            # of the terminal area after the session sidebar. OCR is the gate
            # for both focused-source states, never an app-state substitute.
            await badge_frame("tour-sync-badge-unselected-640x480.png", "THIS PANE ONLY", 600)
            await badge_frame("tour-sync-badge-selected-640x480.png", "MIRRORING", 290)
        except Exception as error:
            print(f"tour: FAILED sync badge probe: {error}", flush=True)
            return 1
        finally:
            if armed:
                try:
                    # The same Mode > Sync Input command toggles an armed group off.
                    key("Escape")
                    click(207, 16)
                    await asyncio.sleep(.3)
                    mode_frame = tour_dir / "tour-sync-mode-menu.png"
                    screenshot(window, mode_frame, env)
                    disarm_box = await asyncio.to_thread(
                        locate_visible_label, mode_frame, "Sync Input",
                        region=(170, 24, 450, 220),
                    )
                    if disarm_box:
                        click(*centre(disarm_box))
                except Exception as error:
                    print(f"tour: WARNING sync badge cleanup: {error}", flush=True)
            restored = await resize_tour(*original_size)
            if restored != original_size:
                raise RuntimeError(f"tour size not restored after sync probe: {restored}")

    print(f"tour: {len(written)} screenshots written:", flush=True)
    for path in written:
        print(f"  {path}", flush=True)
    return 0


async def main() -> int:
    if sys.platform != "linux":
        raise RuntimeError("Linux-only native GUI fixture")
    if len(sys.argv) != 3:
        raise SystemExit("usage: test-iced-connected-gui.py BINARY ARTIFACT_DIRECTORY")

    binary = Path(sys.argv[1]).resolve(strict=True)
    destination = Path(sys.argv[2]).resolve()
    destination.mkdir(parents=True, exist_ok=True)
    root_base = Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir())).resolve()
    root_base.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="iced-connected-fixture-", dir=root_base) as temp:
        root = Path(temp)
        events = fixture.EventLog(root / "ssh-events.log")
        events.path.write_text("", encoding="utf-8")
        probe_state: dict[str, str | None] = {"phase": None}
        host = fixture.generate_key(root, "host")
        client = fixture.generate_key(root, "client")
        trusted = asyncssh.read_public_key(str(client) + ".pub")
        remote_root = root / "remote-sftp"
        remote_root.mkdir()
        download_name = "444_GUI_DOWNLOAD_FIXTURE.bin"
        download_bytes = b"GUI_DOWNLOAD_FIXTURE\x00\xff\r\n" + bytes(range(256)) * 8
        (remote_root / download_name).write_bytes(download_bytes)
        local_root = root / "local-files"
        local_root.mkdir()
        upload_name = "999_GUI_UPLOAD_FIXTURE.bin"
        upload_bytes = b"GUI_UPLOAD_FIXTURE\x00\xfe\n" + bytes(reversed(range(256))) * 8
        (local_root / upload_name).write_bytes(upload_bytes)
        echo_port = None
        forward_ports: tuple[int, ...] = ()
        if POPULATED:
            echo_server = await asyncio.start_server(fixture.echo_service, "127.0.0.1", 0)
            echo_port = int(echo_server.sockets[0].getsockname()[1])
            first_port = free_loopback_port({echo_port})
            forward_ports = (first_port, free_loopback_port({echo_port, first_port}))
        server = await asyncssh.create_server(
            lambda: MonitoredServer(trusted, events, echo_port=echo_port),
            "127.0.0.1", 0, server_host_keys=[str(host)], encoding="utf-8",
            # A real remote program receives cursor-key escape sequences from
            # its PTY. AsyncSSH's server-side line editor consumes those keys
            # itself, so disable it in this transport fixture and observe the
            # exact bytes delivered by the production Iced terminal backend.
            line_editor=False,
            sftp_factory=lambda chan: isolated_sftp_server(chan, remote_root, events),
            session_factory=lambda stdin, stdout, stderr: monitored_shell(
                stdin, stdout, stderr, events, probe_state
            ),
        )
        port = server.get_port()
        key_type, key_data = fixture.public_fields(Path(str(host) + ".pub"))
        known = root / "known_hosts"
        known.write_text(f"[127.0.0.1]:{port} {key_type} {key_data}\n", encoding="utf-8")
        global_known = root / "global_known_hosts"
        global_known.write_text("", encoding="utf-8")
        ssh_config = root / "ssh_config"
        fixture.write_config(
            ssh_config, port=port, identity=client,
            known_hosts=known, global_known_hosts=global_known,
        )
        profiles = root / "profiles.json"
        profiles.write_text(
            json.dumps(fixture_profiles(forward_ports, echo_port)), encoding="utf-8",
        )
        extedit_editor = root / "fake-editor.py"
        extedit_argv_log = root / "fake-editor-argv.log"
        if EXTEDIT:
            # The app reads ui-state.json BESIDE the profiles file.
            extedit_editor.write_text(
                fake_editor_script(sys.executable, extedit_argv_log, EXTEDIT_MARKER),
                encoding="utf-8",
            )
            extedit_editor.chmod(0o755)
            (root / "ui-state.json").write_text(
                json.dumps(external_editor_ui_state(extedit_editor)), encoding="utf-8",
            )

        runtime = root / "xdg-runtime"
        runtime.mkdir(mode=0o700)
        env = dict(
            os.environ,
            DISPLAY=reserved_display(),
            XDG_RUNTIME_DIR=str(runtime),
            INSPIRUM_CI_REMOTE_KEY_TRACE="1",
            # The WindTerm-style chrome defaults to a LIGHT theme. The OCR
            # band/threshold helpers (_visible_bands, _text_threshold) assume
            # light text on a dark background, so force the dark chrome.
            # The tour (INSPIRUM_GUI_TOUR=1) captures the default LIGHT chrome.
            INSPIRUM_THEME="light" if TOUR else "dark",
        )
        xvfb = subprocess.Popen(
            ["Xvfb", env["DISPLAY"], "-screen", "0", "1440x1000x24", "-nolisten", "tcp"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env,
        )
        app = None
        try:
            await wait_for(lambda: subprocess.run(
                ["xdpyinfo", "-display", env["DISPLAY"]],
                env=env, capture_output=True, timeout=3,
            ).returncode == 0, "isolated Xvfb display")
            app_log = root / "app.log"
            with app_log.open("w", encoding="utf-8") as output:
                app = subprocess.Popen(
                    [str(binary), "--ui", "iced",
                     "--profiles", str(profiles), "--ssh-config", str(ssh_config)],
                    stdout=output, stderr=subprocess.STDOUT, env=env, cwd=local_root,
                )
                window = ""
                for _ in range(100):
                    if app.poll() is not None:
                        raise RuntimeError(
                            "production Iced executable exited before its window appeared:\n"
                            + app_log.read_text(encoding="utf-8")[-1200:]
                        )
                    result = subprocess.run(
                        ["xdotool", "search", "--onlyvisible", "--name", "^Inspirum Terminal$"],
                        env=env, capture_output=True, text=True, timeout=3,
                    )
                    if result.returncode == 0 and result.stdout.strip():
                        window = result.stdout.splitlines()[0]
                        break
                    await asyncio.sleep(.25)
                if not window:
                    raise RuntimeError("production connected Iced window never appeared")

                command("xdotool", "windowfocus", window, env=env)
                if TOUR:
                    # Screenshot tour instead of the acceptance sequence; the
                    # surrounding finally block performs the usual teardown.
                    return await run_tour(
                        window, destination.parent / "tour", env, events, app_log,
                        forward_ports=forward_ports,
                    )
                # The native graphics stack may still be initializing after
                # the X11 window first appears, especially on shared runners.
                # A fixed 2 s wait is still blank on a loaded host; screenshot()
                # rejects that as "empty GUI screenshot". Retry only that miss.
                first_frame_deadline = time.monotonic() + 20 * TIME_SCALE
                await asyncio.sleep(2)
                first_frame = destination / "connected-before-open.png"
                while True:
                    try:
                        screenshot(window, first_frame, env)
                        break
                    except RuntimeError as error:
                        if "empty GUI screenshot" not in str(error):
                            raise
                        if first_frame_deadline - time.monotonic() < 1:
                            raise
                        await asyncio.sleep(1)

                # Saved sessions are listed in the bottom Session dock panel
                # (Filter row, 'Sessions' group row, then one flat 23px row per
                # profile). Locate the profile name by OCR and double-click
                # it; fall back to bounded points in the session list rows,
                # never destructive dialogs. Authentication, rather than a
                # screenshot delta alone, proves the click worked.
                # A single click selects the saved profile and must not open
                # it. The product's double-click window is 400 ms; wait longer
                # so the double click below cannot combine with this one.
                # The tour path stays coordinate-driven and is not changed.
                # count() is defined later in this function, so read the log.
                # connected-session-list.png is taken inside click_label before
                # the click, with the pointer not on the row. Assumption: the
                # pointer starts wherever Xvfb put it; park it in the terminal
                # pane so the before-shot is not a hover fill.
                command(
                    "xdotool", "mousemove", "--window", window, "640", "280", env=env,
                )
                await asyncio.sleep(0.3)
                try:
                    session_row = await click_label(
                        window, destination / "connected-session-list.png",
                        "CI connected fixture", env, timeout=8, settle=0,
                        clicks=1,
                    )
                except RuntimeError as exc:
                    raise RuntimeError(
                        "single click could not find session row "
                        "'CI connected fixture' before the opening double click"
                    ) from exc
                await asyncio.sleep(1.2 * TIME_SCALE)
                started_before_open = events.path.read_text(encoding="utf-8").splitlines().count(
                    "SESSION_STARTED"
                )
                if started_before_open != 0:
                    raise RuntimeError(
                        "single click on 'CI connected fixture' opened a session "
                        f"({started_before_open} SESSION_STARTED) before the double click"
                    )
                # Hover fill also changes pixels. Move off the row without
                # clicking, then measure the selected-row fill in the row band.
                command(
                    "xdotool", "mousemove", "--window", window, "640", "280", env=env,
                )
                await asyncio.sleep(0.5 * TIME_SCALE)
                list_shot = destination / "connected-session-list.png"
                selected_shot = destination / "connected-session-selected.png"
                await asyncio.to_thread(screenshot, window, selected_shot, env)
                win_w, _ = window_size(window, env)
                band = (
                    8,
                    session_row[1] - 4,
                    min(win_w, session_row[2] + 120),
                    session_row[3] + 4,
                )
                highlight = await asyncio.to_thread(
                    band_changed_pixels, list_shot, selected_shot, band,
                )
                if highlight < 150:
                    raise RuntimeError(
                        "single click did not highlight the session row "
                        f"({highlight} pixels changed in band {band}; "
                        f"{list_shot.name} vs {selected_shot.name})"
                    )
                print(
                    f"single click highlighted the session row: {highlight} pixels "
                    f"in band {band}",
                    flush=True,
                )
                try:
                    await click_label(
                        window, destination / "connected-session-open.png",
                        "CI connected fixture", env, timeout=8, settle=1.5,
                        clicks=2,
                    )
                except RuntimeError:
                    pass
                if "SESSION_STARTED" not in events.path.read_text(encoding="utf-8"):
                    for y in (630, 653, 612, 676, 699):
                        command(
                            "xdotool", "mousemove", "--window", window, "200", str(y),
                            "click", "--repeat", "2", "--delay", "120", "1", env=env,
                        )
                        await asyncio.sleep(1.5)
                        if "SESSION_STARTED" in events.path.read_text(encoding="utf-8"):
                            break
                        # A coordinate may land on a nonterminal affordance;
                        # dismiss any dialog before trying the next row.
                        command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                if "SESSION_STARTED" not in events.path.read_text(encoding="utf-8"):
                    raise RuntimeError(
                        "double-clicking the saved profile never started a real SSH session. "
                        "SSH events: " + repr(events.path.read_text(encoding="utf-8")[-800:])
                        + "; application log tail: " + app_log.read_text(encoding="utf-8")[-1800:]
                    )
                await asyncio.sleep(1)
                screenshot(window, destination / "connected-terminal.png", env)

                # The session-list click that started SSH does not prove the
                # focused pane owns the keyboard: the dock filter is a text
                # input, and a button click is not the terminal. Click the
                # pane and require an echo before any negative PTY assertion,
                # or a chrome control that never had the keys would pass.
                command(
                    "xdotool", "windowfocus", window, "mousemove", "--window", window,
                    "640", "280", "click", "1", env=env,
                )
                await asyncio.sleep(.3)
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "echo:TERMINAL_OWNED", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "NATIVE_ECHO:TERMINAL_OWNED" in events.path.read_text(encoding="utf-8").splitlines(),
                    "terminal owns keyboard before chrome menu probes",
                )

                # Session menu, Sender editor and bottom-dock gear must own
                # keyboard input locally. Arm the PTY watch only after the
                # opening click settles: that click can resize the PTY, and
                # resize is not keyboard input. Watch raw chunks, not just
                # completed commands, so a stray Escape or unterminated
                # sentinel fails. Do not select a gear/menu row from the
                # keyboard; legitimate menu choices stay mouse-only later.
                command("xdotool", "mousemove", "--window", window,
                        "30", "16", "click", "1", env=env)
                await asyncio.sleep(.3)
                probe_state["phase"] = "MENU"
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "MENU_LOCAL_SENTINEL", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                await asyncio.sleep(.6)
                if "UNEXPECTED_MENU_PTY_INPUT" in events.path.read_text(encoding="utf-8"):
                    raise RuntimeError("Session menu forwarded typing, Return, or Escape to SSH")
                probe_state["phase"] = None
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "echo:AFTER_MENU", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "NATIVE_ECHO:AFTER_MENU" in events.path.read_text(encoding="utf-8").splitlines(),
                    "shell echo after dismissing Session menu",
                )

                # DockActivate on the tab does not focus the text_editor.
                # Click the editor body (below the 28px header) or typed
                # keys still belong to the terminal and the probe is vacuous.
                command("xdotool", "mousemove", "--window", window,
                        "47", "552", "click", "1", env=env)
                await asyncio.sleep(.3)
                command("xdotool", "mousemove", "--window", window,
                        "240", "680", "click", "1", env=env)
                await asyncio.sleep(.3)
                probe_state["phase"] = "SENDER"
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "SENDER_LOCAL_SENTINEL", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                await asyncio.sleep(.6)
                if "UNEXPECTED_SENDER_PTY_INPUT" in events.path.read_text(encoding="utf-8"):
                    raise RuntimeError("Sender dock forwarded typing, Return, or Escape to SSH")
                probe_state["phase"] = None
                # Do not click the terminal: Escape itself must restore focus.
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "echo:AFTER_SENDER_ESCAPE", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "NATIVE_ECHO:AFTER_SENDER_ESCAPE" in events.path.read_text(encoding="utf-8").splitlines(),
                    "shell echo after Sender Escape without terminal click",
                )

                # Bottom-dock gear at the tour coordinate (width-40, strip
                # centre 552 on the 1280x800 window). Open with the mouse,
                # then watch only the following keys. Do not click a row.
                gear_width, _gear_height = window_size(window, env)
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(gear_width - 40), "552", "click", "1", env=env,
                )
                await asyncio.sleep(.3)
                probe_state["phase"] = "GEAR"
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "GEAR_LOCAL_SENTINEL", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                await asyncio.sleep(.6)
                if "UNEXPECTED_GEAR_PTY_INPUT" in events.path.read_text(encoding="utf-8"):
                    raise RuntimeError("dock gear menu forwarded typing, Return, or Escape to SSH")
                probe_state["phase"] = None
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "echo:AFTER_GEAR", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "NATIVE_ECHO:AFTER_GEAR" in events.path.read_text(encoding="utf-8").splitlines(),
                    "shell echo after dismissing dock gear menu",
                )
                if any(
                    marker in events.path.read_text(encoding="utf-8")
                    for marker in (
                        "UNEXPECTED_MENU_PTY_INPUT",
                        "UNEXPECTED_SENDER_PTY_INPUT",
                        "UNEXPECTED_GEAR_PTY_INPUT",
                    )
                ):
                    raise RuntimeError("menu, Sender or dock gear input leaked into SSH PTY")

                # Ctrl+Shift+J toggles an auxiliary shell: a second independent
                # SSH pane for the same profile. With a menu open the chord must
                # not close or reopen it; Escape alone must restore the chord
                # (do not click the terminal before the closing press). Leave
                # probe_state["phase"] as None: the auxiliary session's own
                # startup output is not keyboard leakage. Fails while a menu-open
                # chord still disconnects the shell.
                def count(marker: str) -> int:
                    return sum(
                        1
                        for line in events.path.read_text(encoding="utf-8").splitlines()
                        if line == marker
                    )

                def press_ctrl_shift_j() -> None:
                    command(
                        "xdotool", "windowfocus", window, "key",
                        "--clearmodifiers", "ctrl+shift+j", env=env,
                    )

                command(
                    "xdotool", "windowfocus", window, "mousemove", "--window", window,
                    "640", "280", "click", "1", env=env,
                )
                await asyncio.sleep(.3)
                started0 = count("SESSION_STARTED")
                closed0 = count("SESSION_CLOSED:clean")
                press_ctrl_shift_j()
                await wait_for(
                    lambda: count("SESSION_STARTED") == started0 + 1,
                    "auxiliary shell opened by Ctrl+Shift+J",
                )
                await asyncio.sleep(1)
                screenshot(window, destination / "connected-aux-shell-open.png", env)

                command(
                    "xdotool", "mousemove", "--window", window,
                    "30", "16", "click", "1", env=env,
                )
                await asyncio.sleep(.4)
                press_ctrl_shift_j()
                await asyncio.sleep(min(8, 2 * TIME_SCALE))
                if (
                    count("SESSION_CLOSED:clean") != closed0
                    or count("SESSION_STARTED") != started0 + 1
                ):
                    raise RuntimeError(
                        "Ctrl+Shift+J under the Session menu closed or reopened the auxiliary shell"
                    )
                screenshot(
                    window, destination / "connected-aux-shell-session-menu.png", env,
                )
                command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                await asyncio.sleep(.4)

                # Opening the auxiliary shell splits the workspace, but the
                # bottom dock is unchanged: the gear probe's point still hits.
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(gear_width - 40), "552", "click", "1", env=env,
                )
                await asyncio.sleep(.4)
                press_ctrl_shift_j()
                await asyncio.sleep(min(8, 2 * TIME_SCALE))
                if (
                    count("SESSION_CLOSED:clean") != closed0
                    or count("SESSION_STARTED") != started0 + 1
                ):
                    raise RuntimeError(
                        "Ctrl+Shift+J under the dock gear menu closed or reopened the auxiliary shell"
                    )
                screenshot(
                    window, destination / "connected-aux-shell-gear-menu.png", env,
                )
                command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                await asyncio.sleep(.4)

                # No terminal click: the shortcut must work again after Escape.
                # Both shells are still up. An exit already logged here must
                # fail now: recording it after the close would hide it inside
                # the baseline the rest of the run compares against.
                ended_before_aux_close = count("SHELL_HANDLER_ENDED")
                eof_before_aux_close = count("SHELL_STREAM_EOF")
                if ended_before_aux_close != 0 or eof_before_aux_close != 0:
                    raise RuntimeError(
                        "SSH shell ended before the intentional auxiliary close: "
                        f"SHELL_HANDLER_ENDED={ended_before_aux_close} "
                        f"SHELL_STREAM_EOF={eof_before_aux_close} (expected 0 and 0)"
                    )
                press_ctrl_shift_j()
                await wait_for(
                    lambda: count("SESSION_CLOSED:clean") == closed0 + 1,
                    "auxiliary shell closed by Ctrl+Shift+J after the menu was dismissed",
                )
                # One monitored_shell ends once. Empty stdin.read writes exactly
                # one SHELL_STREAM_EOF and returns; finally then writes exactly
                # one SHELL_HANDLER_ENDED (and SESSION_CLOSED:clean). The "exit"
                # line and the exception path omit EOF, but Ctrl+Shift+J closes
                # the channel — it does not send "exit". One intentional close
                # is therefore exactly one of each, not whatever has accumulated.
                def auxiliary_close_ended_once() -> bool:
                    ended = count("SHELL_HANDLER_ENDED")
                    eof = count("SHELL_STREAM_EOF")
                    if ended > 1 or eof > 1:
                        raise RuntimeError(
                            "intentional auxiliary close ended more than one shell: "
                            f"SHELL_HANDLER_ENDED={ended} "
                            f"SHELL_STREAM_EOF={eof} (expected 1 and 1)"
                        )
                    return ended == 1 and eof == 1

                try:
                    await wait_for(
                        auxiliary_close_ended_once,
                        "exactly one SHELL_HANDLER_ENDED and one SHELL_STREAM_EOF "
                        "after the intentional auxiliary close",
                    )
                except RuntimeError as exc:
                    ended = count("SHELL_HANDLER_ENDED")
                    eof = count("SHELL_STREAM_EOF")
                    if (ended == 1 and eof == 1) or "SHELL_HANDLER_ENDED=" in str(exc):
                        raise
                    raise RuntimeError(
                        "intentional auxiliary close did not end exactly one shell: "
                        f"SHELL_HANDLER_ENDED={ended} "
                        f"SHELL_STREAM_EOF={eof} (expected 1 and 1)"
                    ) from exc
                # Freeze these before the settle sleep. A recount after the
                # sleep would adopt any extra exit as the accepted baseline.
                shells_ended_before_split = count("SHELL_HANDLER_ENDED")
                shell_eof_before_split = count("SHELL_STREAM_EOF")
                if shells_ended_before_split != 1 or shell_eof_before_split != 1:
                    raise RuntimeError(
                        "shell-end baseline after intentional auxiliary close "
                        "is not exactly one: "
                        f"SHELL_HANDLER_ENDED={shells_ended_before_split} "
                        f"SHELL_STREAM_EOF={shell_eof_before_split} (expected 1 and 1)"
                    )
                await asyncio.sleep(1)

                # These shortcuts are intentionally exercised through native
                # Iced keyboard events, never a direct Rust state transition.
                # The auxiliary shell already added one SESSION_STARTED and one
                # clean close. Shell-end baselines were frozen at exactly one
                # immediately after that close, before this sleep.
                started_before_split = count("SESSION_STARTED")
                command("xdotool", "windowfocus", window, "key", "--clearmodifiers", "ctrl+shift+r", env=env)
                await wait_for(
                    lambda: count("SESSION_STARTED") >= started_before_split + 1,
                    "second real SSH pane via Ctrl+Shift+R",
                )
                await asyncio.sleep(1)
                screenshot(window, destination / "connected-split.png", env)
                split_pixels = diff(
                    destination / "connected-terminal.png",
                    destination / "connected-split.png", env,
                )
                if split_pixels < 3000:
                    raise RuntimeError(f"split screen changed only {split_pixels} pixels")

                command("xdotool", "windowfocus", window, "key", "--clearmodifiers", "ctrl+shift+o", env=env)
                await wait_for(
                    lambda: "SFTP_SUBSYSTEM_STARTED" in events.path.read_text(encoding="utf-8"),
                    "actual SFTP subsystem opened by Iced Files dock",
                )
                await asyncio.sleep(2)
                screenshot(window, destination / "connected-files-dock.png", env)
                files_pixels = diff(
                    destination / "connected-split.png",
                    destination / "connected-files-dock.png", env,
                )
                if files_pixels < 3000:
                    raise RuntimeError(f"files dock changed only {files_pixels} pixels")

                # Stay bottom-docked while exercising the native file controls.
                await wait_for(
                    lambda: "SFTP_LIST_READY" in events.path.read_text(encoding="utf-8"),
                    "completed remote listing before file-row interaction",
                )
                await asyncio.sleep(.7)
                geometry = command(
                    "xdotool", "getwindowgeometry", "--shell", window, env=env,
                )
                if "WIDTH=1280" not in geometry or "HEIGHT=800" not in geometry:
                    raise RuntimeError(
                        "connected bottom-dock coordinates require the production 1280x800 "
                        f"window captured by this fixture; geometry was {geometry!r}"
                    )
                screenshot(window, destination / "connected-files-bottom-dock.png", env)

                width, height = window_size(window, env)
                strip = dock_strip_region(width, height)
                body = dock_body_region(width, height)

                # Toggling Files activates the Filer (REMOTE) tab of the bottom
                # dock. The remote fixture row must be visibly targetable.
                bottom = destination / "connected-files-bottom-dock.png"
                remote_row = await asyncio.to_thread(locate_visible_label, bottom, download_name, region=body,
                )
                if remote_row[3] <= remote_row[1]:
                    raise RuntimeError("bottom-dock remote file row is not visibly targetable")

                # Explorer tab = LOCAL listing with the 'Upload ->' action.
                explorer_shot = destination / "connected-files-explorer.png"
                await click_label(window, explorer_shot, "Explorer", env, region=strip, settle=.8)
                local_row = await locate_rendered_label(
                    window, explorer_shot, upload_name, env=env,
                    region=body, timeout=40,
                )
                upload_action = await asyncio.to_thread(
                    locate_visible_label, explorer_shot, "Upload",
                    region=(0, int(height * .62), width, height - 22),
                )
                if local_row[3] <= local_row[1]:
                    raise RuntimeError("bottom-dock local file row is not visibly targetable")

                # Select the only local fixture row and activate Upload through
                # native Iced pointer events. Filesystem bytes are the transfer
                # integrity oracle; a screenshot delta or SFTP handshake alone
                # is not accepted as proof of a completed GUI transfer.
                uploaded = remote_root / upload_name
                await click_transfer(
                    window=window,
                    env=env,
                    row=center(local_row),
                    action=center(upload_action),
                    completed=lambda: uploaded.is_file()
                    and uploaded.read_bytes() == upload_bytes,
                    label="click-driven SFTP upload into expected remote fixture root",
                )
                await asyncio.sleep(.7)
                screenshot(window, destination / "connected-files-uploaded.png", env)

                # Back to the Filer tab (REMOTE). The remote fixture name sorts
                # first, so the first row is deterministic even after the
                # upload refresh. Activate '<- Download' and verify the exact
                # binary payload landed in the Explorer's local directory.
                filer_shot = destination / "connected-files-filer.png"
                await click_label(window, filer_shot, "Filer", env, region=strip, settle=.8)
                remote_row = await locate_rendered_label(
                    window, filer_shot, download_name, env=env,
                    region=body, timeout=40,
                )
                download_action = await asyncio.to_thread(
                    locate_visible_label, filer_shot, "Download",
                    region=(0, int(height * .62), width, height - 22),
                )
                downloaded = local_root / download_name
                if downloaded.exists():
                    raise RuntimeError(
                        "download destination unexpectedly exists before native click"
                    )
                await click_transfer(
                    window=window,
                    env=env,
                    row=center(remote_row),
                    action=center(download_action),
                    completed=lambda: downloaded.is_file()
                    and downloaded.read_bytes() == download_bytes,
                    label="click-driven SFTP download into expected local fixture root",
                    selected_screenshot=(
                        destination / "connected-files-download-selected.png"
                    ),
                )
                await asyncio.sleep(.7)
                screenshot(window, destination / "connected-files-downloaded.png", env)

                if uploaded.read_bytes() != upload_bytes:
                    raise RuntimeError("click-driven GUI upload failed exact-byte integrity")
                if downloaded.read_bytes() != download_bytes:
                    raise RuntimeError("click-driven GUI download failed exact-byte integrity")

                # Two completed transfers must not consume the listings. Select
                # another visibly rendered, distinct remote file through the
                # production row control; its selection styling is the oracle.
                # The text 'Refresh' button is gone (icon only) and switching
                # Explorer <-> Filer tabs does not reload the listing; activate
                # Filer, then click its path-bar refresh icon (see helper).
                # Prefixes 111 < 444 < 999 are distinct and OCR-safe (no 0/O);
                # the third file sorts first, so it stays visible at 960x640.
                third_name = "111_GUI_THIRD_FIXTURE.bin"
                third_bytes = b"GUI_THIRD_FIXTURE" + bytes(range(32))
                (remote_root / third_name).write_bytes(third_bytes)
                listings_before = events.path.read_text(encoding="utf-8").count("SFTP_LIST_READY")
                filer_tab = await click_label(
                    window, destination / "connected-files-refresh-back.png",
                    "Filer", env, region=strip, settle=.3,
                )
                await click_filer_refresh(
                    window, env, destination / "connected-files-refresh-clicked.png",
                    path_bar_y=center(filer_tab)[1] + 32,
                    completed=lambda: events.path.read_text(encoding="utf-8").count("SFTP_LIST_READY") > listings_before,
                    label="remote listing refresh after creating distinct fixture",
                )
                # The fixture event fires when the server answered the listing;
                # the Iced view repaints afterwards. Poll the rendered window
                # until the distinct filename is actually visible.
                third_row = await locate_rendered_label(
                    window, destination / "connected-files-third-visible.png",
                    third_name, env=env, region=body, timeout=40,
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(third_row)), "click", "1", env=env,
                )
                await asyncio.sleep(.4)
                screenshot(window, destination / "connected-files-third-selected.png", env)
                if diff(
                    destination / "connected-files-third-visible.png",
                    destination / "connected-files-third-selected.png", env,
                ) < 20:
                    raise RuntimeError("distinct post-transfer file row was not visibly selected")

                # Name-header click reverses the remote listing in place.
                # Default is name ascending (directories first, then name; this
                # fixture has no directories), so 111 is above 444. A second
                # click must restore that order: the 960x640 step needs 111
                # first. Header clicks must not start another SFTP listing.
                row_111 = await locate_rendered_label(
                    window, destination / "connected-files-sort-before.png",
                    third_name, env=env, region=body, timeout=20,
                )
                # Name-column data rows, fixed to the first file row. Reordering
                # does not move the band, and the transfer status line is outside
                # it, so a later read of 444 cannot land on "Download 444 …".
                rows_region = name_rows_region(row_111, 3)
                default_order = await asyncio.to_thread(
                    name_order_tops,
                    destination / "connected-files-sort-before.png",
                    third_name,
                    download_name,
                    rows_region,
                )
                if default_order is None or default_order[0] >= default_order[1]:
                    y_111 = None if default_order is None else default_order[0]
                    y_444 = None if default_order is None else default_order[1]
                    raise RuntimeError(
                        "Filer default name order is not ascending: "
                        f"y(111)={y_111} y(444)={y_444} rows_region={rows_region}"
                    )
                # Band derived from the located 111 row, which is the first
                # file row. OCR bounds are the filename glyphs (inset 3px),
                # vertically centred in the 23px row and ~20px right of the
                # header label (14px file icon + 6px gap). The Name header is
                # the 20px table header plus a 1px divider immediately above
                # that row. The strip is therefore just above the glyphs,
                # shifted left to cover the word Name, and not tall enough to
                # include the path bar or the filename. It stays put when the
                # rows reorder, so both clicks use this same band.
                name_header = (
                    max(0, row_111[0] - 36),
                    max(0, row_111[1] - 36),
                    row_111[0] + 48,
                    row_111[1] - 2,
                )
                if name_header[3] <= name_header[1] or name_header[2] <= name_header[0]:
                    raise RuntimeError(
                        f"Name header band collapsed from row bounds {row_111}: {name_header}"
                    )
                listings_at_sort = events.path.read_text(encoding="utf-8").count("SFTP_LIST_READY")

                async def wait_name_order(shot: Path, *, descending: bool) -> tuple[int, int]:
                    deadline = time.monotonic() + 15 * TIME_SCALE
                    seen = None
                    while True:
                        await asyncio.to_thread(screenshot, window, shot, env)
                        seen = await asyncio.to_thread(
                            name_order_tops, shot, third_name, download_name, rows_region,
                        )
                        if seen is not None:
                            matched = seen[0] > seen[1] if descending else seen[0] < seen[1]
                            if matched:
                                return seen
                        if time.monotonic() >= deadline:
                            order = "descending" if descending else "ascending"
                            y_111 = None if seen is None else seen[0]
                            y_444 = None if seen is None else seen[1]
                            raise RuntimeError(
                                f"Name header click did not produce {order} Filer order: "
                                f"y(111)={y_111} y(444)={y_444} rows_region={rows_region}"
                            )
                        await asyncio.sleep(.5)

                await click_label(
                    window, destination / "connected-files-sort-header.png",
                    "Name", env, region=name_header, settle=.4,
                )
                await wait_name_order(
                    destination / "connected-files-sorted-desc.png", descending=True,
                )
                await click_label(
                    window, destination / "connected-files-sort-header-again.png",
                    "Name", env, region=name_header, settle=.4,
                )
                await wait_name_order(
                    destination / "connected-files-sorted-asc.png", descending=False,
                )
                listings_after_sort = events.path.read_text(encoding="utf-8").count("SFTP_LIST_READY")
                if listings_after_sort != listings_at_sort:
                    raise RuntimeError(
                        "Name header click started an SFTP listing: "
                        f"SFTP_LIST_READY {listings_at_sort} -> {listings_after_sort}"
                    )

                # Filer right-click menu on the remote file row. Rows are name
                # ascending again (111, then 444). Do not choose Rename or
                # Delete. The popup is clamped inside the window (200px wide,
                # 6*24+2 px for a remote file), so a click near the bottom
                # opens upward rather than down from the pointer.
                # No-pause gesture is the regression case for a popup anchored
                # at a stale pointer. Park at the top right of the terminal
                # pane, far from the bottom-dock row on the x axis, let that
                # become the last known pointer, then move and right-press in
                # one command with no sleep between motion and the press.
                parked_x, parked_y = 1100, 200
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(parked_x), str(parked_y), env=env,
                )
                await asyncio.sleep(0.6 * TIME_SCALE)
                context_row = await locate_rendered_label(
                    window,
                    destination / "connected-files-context-row.png",
                    download_name,
                    env=env,
                    region=body,
                    timeout=20,
                )
                click_x, click_y = center(context_row)
                width, height = window_size(window, env)
                # Pointer box, clamped to the window. The menu may sit above
                # the click when the clamp pushes the anchor up. Computed
                # before the press: a popup opened at the parked point is
                # about 200px wide and 146px tall, so it cannot put Refresh
                # or New Folder in this box when the parked point is far away.
                menu_region = (
                    max(0, click_x - 10),
                    max(0, click_y - 240),
                    min(width, click_x + 260),
                    min(height, click_y + 240),
                )
                if menu_region[2] <= menu_region[0] or menu_region[3] <= menu_region[1]:
                    raise RuntimeError(
                        "Filer context-menu search box collapsed from click "
                        f"({click_x}, {click_y}) in {width}x{height}: {menu_region}"
                    )
                parked_x_gap = (
                    menu_region[0] - parked_x if parked_x < menu_region[0]
                    else parked_x - menu_region[2] if parked_x > menu_region[2]
                    else 0
                )
                parked_y_gap = (
                    menu_region[1] - parked_y if parked_y < menu_region[1]
                    else parked_y - menu_region[3] if parked_y > menu_region[3]
                    else 0
                )
                if parked_x_gap < 300 and parked_y_gap < 300:
                    raise RuntimeError(
                        "parked pointer "
                        f"({parked_x}, {parked_y}) is not outside menu_region "
                        f"{menu_region} by at least 300 px on either axis "
                        f"(x gap {parked_x_gap}, y gap {parked_y_gap}; "
                        f"click ({click_x}, {click_y}) in {width}x{height})"
                    )
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(click_x), str(click_y), "click", "3", env=env,
                )
                # The click is a synthetic X event with no settle. A locate
                # that captures the pre-redraw frame spends its OCR budget
                # and reports a miss. Wait until the window actually changes
                # (selection fill and/or the popup), then search for labels.
                menu_shot = destination / "connected-files-context-menu.png"
                row_shot = destination / "connected-files-context-row.png"
                paint_deadline = time.monotonic() + 8 * TIME_SCALE
                painted = 0
                while True:
                    await asyncio.to_thread(screenshot, window, menu_shot, env)
                    painted = await asyncio.to_thread(diff, row_shot, menu_shot, env)
                    if painted >= 50:
                        break
                    if time.monotonic() >= paint_deadline:
                        raise RuntimeError(
                            "right-click on "
                            f"{download_name} at ({click_x}, {click_y}) did not "
                            f"repaint the window ({painted} pixels vs {row_shot.name})"
                        )
                    await asyncio.sleep(.3)
                # "Rename" is the first OCR token of action-row "Rename remote",
                # and locate_visible_label cannot require "Rename" not followed
                # by " remote". That button (x 292-384, y ~760) overlaps a
                # 202x147 menu anchored at this pointer, so a y-cut would also
                # drop a Rename row that sits anywhere in the menu. Require
                # Refresh and New Folder in the full pointer box instead:
                # neither is on the action row ("New remote folder").
                open_deadline = time.monotonic() + 20 * TIME_SCALE
                new_folder_item = None
                refresh_item = None
                last_open_error = "menu labels not searched"
                while True:
                    await asyncio.to_thread(screenshot, window, menu_shot, env)
                    saw_refresh = False
                    saw_new_folder = False
                    try:
                        refresh_item = await asyncio.to_thread(
                            locate_visible_label,
                            menu_shot, "Refresh", region=menu_region, budget=4,
                        )
                        saw_refresh = True
                    except RuntimeError as exc:
                        last_open_error = str(exc)
                    if saw_refresh:
                        try:
                            new_folder_item = await asyncio.to_thread(
                                locate_visible_label,
                                menu_shot, "New Folder", region=menu_region, budget=4,
                            )
                            saw_new_folder = True
                        except RuntimeError as exc:
                            last_open_error = str(exc)
                    if saw_refresh and saw_new_folder:
                        break
                    if time.monotonic() >= open_deadline:
                        raise RuntimeError(
                            "Filer context menu did not render Refresh and New Folder "
                            f"after right-click at ({click_x}, {click_y}) on "
                            f"{download_name}: {last_open_error}; "
                            f"region {menu_region}"
                        )
                    await asyncio.sleep(.4)
                if new_folder_item is None or refresh_item is None:
                    raise RuntimeError(
                        "Filer context menu locate returned no bounds after "
                        f"finding Refresh and New Folder "
                        f"(New Folder {new_folder_item}, Refresh {refresh_item}, "
                        f"region {menu_region})"
                    )
                for item_name, item_bounds in (
                    ("Refresh", refresh_item),
                    ("New Folder", new_folder_item),
                ):
                    if _bounds_inside(item_bounds, menu_region) is None:
                        raise RuntimeError(
                            f"Filer context menu {item_name} bounds {item_bounds} "
                            f"are not wholly inside menu_region {menu_region} "
                            f"(Refresh {refresh_item}, New Folder {new_folder_item}, "
                            f"parked ({parked_x}, {parked_y}))"
                        )
                    left, top, right, bottom = item_bounds
                    near_x = (
                        left - parked_x if parked_x < left
                        else parked_x - right if parked_x > right
                        else 0
                    )
                    near_y = (
                        top - parked_y if parked_y < top
                        else parked_y - bottom if parked_y > bottom
                        else 0
                    )
                    parked_gap = (near_x * near_x + near_y * near_y) ** 0.5
                    if parked_gap < 150:
                        raise RuntimeError(
                            f"Filer context menu {item_name} bounds {item_bounds} "
                            f"lie within 150 px of parked pointer "
                            f"({parked_x}, {parked_y}) (gap {parked_gap:.1f}; "
                            f"Refresh {refresh_item}, New Folder {new_folder_item}, "
                            f"region {menu_region})"
                        )
                print(
                    "Filer context menu anchored at click "
                    f"({click_x}, {click_y}); Refresh {refresh_item} "
                    f"New Folder {new_folder_item} inside {menu_region}; "
                    f"parked ({parked_x}, {parked_y}) axis gaps "
                    f"x={parked_x_gap} y={parked_y_gap}",
                    flush=True,
                )

                # Menu owns the keyboard. Arm the watch only after the
                # right-click settles: that click selects the row and must
                # not be scored as typed input. Do not click a menu row.
                probe_state["phase"] = "CONTEXT"
                try:
                    typed_before = count("UNEXPECTED_CONTEXT_PTY_INPUT")
                    command(
                        "xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "CONTEXT_LOCAL_SENTINEL", env=env,
                    )
                    command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                    await asyncio.sleep(.6)
                    typed_after = count("UNEXPECTED_CONTEXT_PTY_INPUT")
                    if typed_after != typed_before:
                        raise RuntimeError(
                            "Filer context menu forwarded typing or Return to SSH "
                            f"(UNEXPECTED_CONTEXT_PTY_INPUT {typed_before} -> {typed_after})"
                        )
                    command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                    # Do not move or click between Escape and these checks: a
                    # backdrop click dismisses the menu and would hide a broken
                    # Escape. Poll until Refresh is gone, the popup pixels are
                    # gone, and the file row is readable again. menu_shot is
                    # still the frame where Refresh and New Folder were found.
                    closed_shot = destination / "connected-files-context-closed.png"
                    gone_deadline = time.monotonic() + 12 * TIME_SCALE
                    still_refresh = "not searched: popup pixels never changed"
                    closed_pixels = 0
                    restored_row = None
                    row_restored = False
                    # Cheap pixel poll first: the frame right after the key
                    # predates the repaint, and one OCR miss on it can outlast
                    # the whole deadline. OCR only frames that changed.
                    while True:
                        await asyncio.to_thread(screenshot, window, closed_shot, env)
                        closed_pixels = await asyncio.to_thread(
                            diff, menu_shot, closed_shot, env,
                        )
                        if closed_pixels >= 2000:
                            try:
                                still_refresh = await asyncio.to_thread(
                                    locate_visible_label,
                                    closed_shot, "Refresh", region=menu_region, budget=1.0,
                                )
                            except RuntimeError:
                                still_refresh = None
                                break
                        if time.monotonic() >= gone_deadline:
                            break
                        await asyncio.sleep(.4)
                    # The row is located once, on the frame that passed both
                    # checks above, with the default budget and outside the
                    # deadline: a miss here walks every fallback stage.
                    if still_refresh is None:
                        try:
                            restored_row = await asyncio.to_thread(
                                locate_visible_label,
                                closed_shot, download_name, region=body,
                            )
                            row_restored = all(
                                abs(restored_row[edge] - context_row[edge]) <= 6
                                for edge in range(4)
                            )
                        except RuntimeError:
                            restored_row = None
                    escaped_after = count("UNEXPECTED_CONTEXT_PTY_INPUT")
                    if still_refresh is not None:
                        raise RuntimeError(
                            "Escape did not close the Filer context menu; "
                            f"Refresh still at {still_refresh} in {menu_region}"
                        )
                    if escaped_after != typed_after:
                        raise RuntimeError(
                            "Escape from the Filer context menu forwarded input to SSH "
                            f"(UNEXPECTED_CONTEXT_PTY_INPUT {typed_after} -> {escaped_after})"
                        )
                    restore_failed = []
                    if closed_pixels < 2000:
                        restore_failed.append(
                            f"menu pixel diff {closed_pixels} < 2000 "
                            f"({menu_shot.name} vs {closed_shot.name})"
                        )
                    if not row_restored:
                        restore_failed.append(
                            f"file row bounds {restored_row} not within 6 px of "
                            f"context_row {context_row} on every edge "
                            f"(region {body})"
                        )
                    if restore_failed:
                        raise RuntimeError(
                            "Escape did not restore the Filer context menu background; "
                            + "; ".join(restore_failed)
                        )
                    print(
                        f"Escape restored the row: {closed_pixels} pixels "
                        f"({menu_shot.name} vs {closed_shot.name}); "
                        f"row {restored_row} vs context_row {context_row}",
                        flush=True,
                    )
                finally:
                    probe_state["phase"] = None
                # Escape clears the menu and does not restore the terminal
                # the way the Session menu does. Click the pane the earlier
                # chrome probes used, then require the shell to answer.
                command(
                    "xdotool", "windowfocus", window, "mousemove", "--window", window,
                    "640", "280", "click", "1", env=env,
                )
                await asyncio.sleep(.3)
                command(
                    "xdotool", "type", "--clearmodifiers", "--delay", "25",
                    "echo:AFTER_CONTEXT_MENU", env=env,
                )
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "NATIVE_ECHO:AFTER_CONTEXT_MENU" in events.path.read_text(encoding="utf-8").splitlines(),
                    "shell echo after dismissing Filer context menu",
                )

                # Find strip: Ctrl+Shift+F opens a 26px bar at the bottom of
                # the centre workspace, directly above the bottom dock tab
                # strip, and focuses its input. Prev, Next, and the hit
                # counter are read by read_strip_words (right-hand cluster,
                # single-line OCR). A label outside find_band does not count.
                # Do not click a pane here. The (640, 280) click for
                # AFTER_CONTEXT_MENU left the right pane focused; Find counts
                # that pane only, not the left pane's older NATIVE_ECHO lines.
                # Dock tab top is the
                # Filer glyph centre minus 13 (STRIP_HEIGHT / 2). `strip` is
                # the OCR search band (0.62*height..0.76*height), not the
                # strip's y — a strip rendered elsewhere fails the band.
                width, height = window_size(window, env)
                dock_tab_top = center(filer_tab)[1] - 13
                find_band = (
                    0,
                    max(0, dock_tab_top - 40),
                    width,
                    min(height, dock_tab_top + 4),
                )
                if find_band[3] - find_band[1] < 30 or find_band[2] <= find_band[0]:
                    raise RuntimeError(
                        "Find strip search band collapsed: "
                        f"dock_tab_top={dock_tab_top} Filer {filer_tab} "
                        f"strip {strip} window {width}x{height} band {find_band}"
                    )
                find_before = destination / "connected-find-before.png"
                find_open_shot = destination / "connected-find-open.png"
                await asyncio.to_thread(screenshot, window, find_before, env)
                command(
                    "xdotool", "windowfocus", window, "key",
                    "--clearmodifiers", "ctrl+shift+f", env=env,
                )
                open_deadline = time.monotonic() + 8 * TIME_SCALE
                open_pixels = 0
                while True:
                    if app.poll() is not None:
                        raise RuntimeError(
                            "GUI exited before the Find strip repainted "
                            f"(code {app.returncode})"
                        )
                    await asyncio.to_thread(screenshot, window, find_open_shot, env)
                    open_pixels = await asyncio.to_thread(
                        diff, find_before, find_open_shot, env,
                    )
                    if open_pixels >= 50:
                        break
                    if time.monotonic() >= open_deadline:
                        raise RuntimeError(
                            "Ctrl+Shift+F did not repaint the window "
                            f"({open_pixels} pixels vs {find_before.name}; "
                            f"band {find_band})"
                        )
                    await asyncio.sleep(.3)
                label_deadline = time.monotonic() + 20 * TIME_SCALE
                prev_bounds = None
                next_bounds = None
                last_find_error = "Find labels not searched"
                while True:
                    await asyncio.to_thread(screenshot, window, find_open_shot, env)
                    strip_words = await asyncio.to_thread(
                        read_strip_words, find_open_shot, find_band,
                    )
                    prev_bounds = match_strip_label(strip_words, "Prev", find_band)
                    next_bounds = match_strip_label(strip_words, "Next", find_band)
                    if prev_bounds is not None and next_bounds is not None:
                        break
                    seen = [text for text, _bounds in strip_words][:40]
                    missing = "Prev" if prev_bounds is None else "Next"
                    last_find_error = (
                        f"visible label not found in screenshot: {missing} "
                        f"(screenshot: {find_open_shot}, region={find_band}) "
                        f"words={seen}"
                    )
                    prev_bounds = None
                    next_bounds = None
                    if time.monotonic() >= label_deadline:
                        raise RuntimeError(
                            "Find strip did not render Prev and Next in the "
                            f"band above the dock tab strip {find_band} "
                            f"(dock_tab_top={dock_tab_top} from Filer "
                            f"{filer_tab}; strip search region {strip}; "
                            f"open pixel diff {open_pixels}): {last_find_error}"
                        )
                    await asyncio.sleep(.4)
                if (
                    _bounds_inside(prev_bounds, find_band) is None
                    or _bounds_inside(next_bounds, find_band) is None
                ):
                    raise RuntimeError(
                        "Find Prev/Next are not wholly inside the strip band "
                        f"{find_band}: Prev {prev_bounds} Next {next_bounds} "
                        f"(dock_tab_top={dock_tab_top} Filer {filer_tab})"
                    )
                print(
                    f"Find strip open: Prev {prev_bounds} Next {next_bounds} "
                    f"in {find_band} (dock_tab_top={dock_tab_top} "
                    f"Filer {filer_tab}); {open_pixels} pixels",
                    flush=True,
                )

                # Input owns the keyboard. Arm the watch only after the strip
                # has painted: the chord can resize the PTY, and resize is not
                # typed input. Enter is next-hit; Escape closes and must not
                # become a PTY 0x1b. Do not click a strip button.
                #
                # Find counts the focused pane only. The right pane is focused
                # (accent) and its history is exactly two lines,
                # NATIVE_SMOKE_READY and NATIVE_ECHO:AFTER_CONTEXT_MENU.
                # Query NATIVE matches each of those lines once, so the
                # counter total is exactly 2 and the selected index is 1 or
                # 2. The left pane's older NATIVE_ECHO lines are not in the
                # count (that pane would be 5; both panes would be 7).
                # NATIVE_ECHO is 1/1 here and is the wrong oracle.
                # echo:AFTER_FIND is typed only after these counts are read,
                # so the lines it adds cannot change them.
                find_typed = destination / "connected-find-typed.png"
                probe_state["phase"] = "FIND"
                try:
                    typed_before = count("UNEXPECTED_FIND_PTY_INPUT")
                    if FIND_INJECT_PTY_BYTE:
                        # Feed the observation gate, never the shell parser.
                        observe_pty_chunk(events, probe_state, " ")
                        print("FIND_INJECT_PTY_BYTE: injected one harmless byte", flush=True)
                    command(
                        "xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "NATIVE", env=env,
                    )
                    await asyncio.sleep(0.8 * TIME_SCALE)
                    typed_after = count("UNEXPECTED_FIND_PTY_INPUT")
                    if typed_after != typed_before:
                        raise RuntimeError(
                            "Find input forwarded typing to SSH "
                            f"(UNEXPECTED_FIND_PTY_INPUT {typed_before} -> {typed_after})"
                        )
                    await asyncio.to_thread(screenshot, window, find_typed, env)
                    typed_pixels = await asyncio.to_thread(
                        band_changed_pixels, find_open_shot, find_typed, find_band,
                    )
                    # Authoritative hit-count check is the OCR counter. The
                    # band delta is the weaker fact that the strip reacted;
                    # 40 px is above a 1px caret blink and below a typed
                    # query. Both fail the run. The error includes the count
                    # and the raw words so a miss can be recalibrated.
                    counter_words = await asyncio.to_thread(
                        read_strip_words, find_typed, find_band,
                    )
                    parsed = find_hit_counter(strip_words_as_rows(counter_words))
                    observed = [text for text, _bounds in counter_words][:40]
                    if (
                        parsed is None
                        or parsed[1] != 2
                        or parsed[0] not in (1, 2)
                    ):
                        got = None if parsed is None else f"{parsed[0]}/{parsed[1]} from {parsed[2]!r}"
                        raise RuntimeError(
                            "Find counter was not exactly total 2 with "
                            "selected index 1 or 2 (focused pane only) in "
                            f"{find_band}; parsed={got}; words={observed}; "
                            f"band delta {typed_pixels} "
                            f"({find_open_shot.name} vs {find_typed.name})"
                        )
                    current, total, raw = parsed
                    if typed_pixels < 40:
                        raise RuntimeError(
                            "Find band changed only "
                            f"{typed_pixels} pixels after typing NATIVE "
                            f"(need >= 40; OCR counter {current}/{total} from "
                            f"{raw!r} is the hit-count oracle) "
                            f"words={observed} band {find_band}"
                        )
                    print(
                        f"Find counter {current}/{total} from {raw!r} "
                        f"(words {observed}); band delta {typed_pixels} "
                        f"(OCR counter is the hit-count oracle; focused pane)",
                        flush=True,
                    )
                    # Return is Next inside the strip, not a newline to the
                    # PTY. Poll until the selected index moves (1->2 or 2->1)
                    # with the total still 2.
                    command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                    next_deadline = time.monotonic() + 6 * TIME_SCALE
                    find_next_shot = destination / "connected-find-next.png"
                    next_parsed = None
                    next_observed = []
                    changed = False
                    while True:
                        if app.poll() is not None:
                            raise RuntimeError(
                                "GUI exited before Find Next updated the "
                                f"counter (code {app.returncode})"
                            )
                        await asyncio.to_thread(screenshot, window, find_next_shot, env)
                        next_words = await asyncio.to_thread(
                            read_strip_words, find_next_shot, find_band,
                        )
                        next_parsed = find_hit_counter(strip_words_as_rows(next_words))
                        next_observed = [text for text, _bounds in next_words][:40]
                        if (
                            next_parsed is not None
                            and next_parsed[1] == 2
                            and next_parsed[0] in (1, 2)
                            and next_parsed[0] != current
                        ):
                            changed = True
                            break
                        if time.monotonic() >= next_deadline:
                            break
                        await asyncio.sleep(.3)
                    advanced_after = count("UNEXPECTED_FIND_PTY_INPUT")
                    if advanced_after != typed_after:
                        raise RuntimeError(
                            "Find Next (Return) forwarded input to SSH "
                            f"(UNEXPECTED_FIND_PTY_INPUT {typed_after} -> {advanced_after})"
                        )
                    if not changed:
                        after_got = (
                            None if next_parsed is None
                            else f"{next_parsed[0]}/{next_parsed[1]} from {next_parsed[2]!r}"
                        )
                        raise RuntimeError(
                            "Find Next (Return) did not change the selected "
                            "index while keeping total 2 "
                            f"in {find_band}; "
                            f"before {current}/{total} from {raw!r} "
                            f"words={observed}; "
                            f"after {after_got} words={next_observed}"
                        )
                    assert next_parsed is not None
                    print(
                        f"Find Next moved {current}/{total} -> "
                        f"{next_parsed[0]}/{next_parsed[1]} from "
                        f"{next_parsed[2]!r} (words {next_observed}); "
                        f"Enter is Next, not PTY input",
                        flush=True,
                    )
                    # No-hit: select-all and replace is fiddly with xdotool,
                    # so append ZZQ. The query becomes NATIVEZZQ, which is
                    # absent from the focused pane, and the counter must read
                    # 0/0. Still no PTY input.
                    command(
                        "xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "ZZQ", env=env,
                    )
                    await asyncio.sleep(0.8 * TIME_SCALE)
                    nohit_after = count("UNEXPECTED_FIND_PTY_INPUT")
                    if nohit_after != advanced_after:
                        raise RuntimeError(
                            "Find no-hit query forwarded typing to SSH "
                            f"(UNEXPECTED_FIND_PTY_INPUT {advanced_after} -> {nohit_after})"
                        )
                    find_nohit = destination / "connected-find-nohit.png"
                    await asyncio.to_thread(screenshot, window, find_nohit, env)
                    nohit_words = await asyncio.to_thread(
                        read_strip_words, find_nohit, find_band,
                    )
                    nohit_parsed = find_hit_counter(strip_words_as_rows(nohit_words))
                    nohit_observed = [text for text, _bounds in nohit_words][:40]
                    if (
                        nohit_parsed is None
                        or nohit_parsed[0] != 0
                        or nohit_parsed[1] != 0
                    ):
                        got = (
                            None if nohit_parsed is None
                            else f"{nohit_parsed[0]}/{nohit_parsed[1]} from {nohit_parsed[2]!r}"
                        )
                        raise RuntimeError(
                            "Find no-hit counter was not 0/0 after appending "
                            f"ZZQ (query NATIVEZZQ) in {find_band}; "
                            f"parsed={got}; words={nohit_observed}"
                        )
                    print(
                        f"Find no-hit counter 0/0 from {nohit_parsed[2]!r} "
                        f"after appending ZZQ (words {nohit_observed})",
                        flush=True,
                    )
                    # Compare against the settled OPEN strip immediately before
                    # Escape; the helper checks pixels before reading controls.
                    command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                    find_closed = destination / "connected-find-closed.png"
                    closed_pixels, closed_words = await wait_find_strip_closed(
                        app, window, find_nohit, find_closed, find_band, env,
                    )
                    escaped_after = count("UNEXPECTED_FIND_PTY_INPUT")
                    if escaped_after != advanced_after:
                        raise RuntimeError(
                            "Escape from the Find strip forwarded input to SSH "
                            f"(UNEXPECTED_FIND_PTY_INPUT {advanced_after} -> {escaped_after})"
                        )
                    print(
                        f"Escape closed Find: band delta {closed_pixels} "
                        f"in {find_band}; Prev, Next and counter absent",
                        flush=True,
                    )
                finally:
                    probe_state["phase"] = None
                # Closing the strip must hand the keyboard back with no extra
                # click. windowfocus is not a click; the pointer stays put.
                command(
                    "xdotool", "windowfocus", window, "type",
                    "--clearmodifiers", "--delay", "25",
                    "echo:AFTER_FIND", env=env,
                )
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "NATIVE_ECHO:AFTER_FIND" in events.path.read_text(encoding="utf-8").splitlines(),
                    "shell echo after closing Find without clicking the terminal",
                )

                # Regression probes: match_strip_label returns (left, top,
                # right, bottom), as used by center(). Each open gets its own
                # screenshot and verifies actual rendered buttons in find_band.
                async def open_find_and_locate(tag):
                    before = destination / f"connected-find-{tag}-before.png"
                    opened = destination / f"connected-find-{tag}-open.png"
                    await asyncio.to_thread(screenshot, window, before, env)
                    command("xdotool", "windowfocus", window, "key",
                            "--clearmodifiers", "ctrl+shift+f", env=env)
                    deadline = time.monotonic() + 20 * TIME_SCALE
                    pixels = 0
                    words = []
                    while True:
                        if app.poll() is not None:
                            raise RuntimeError(f"{tag}: GUI exited opening Find ({app.returncode})")
                        await asyncio.to_thread(screenshot, window, opened, env)
                        pixels = await asyncio.to_thread(diff, before, opened, env)
                        # Do not OCR pre-repaint frames.
                        if pixels >= 50:
                            words = await asyncio.to_thread(read_strip_words, opened, find_band)
                            prev = match_strip_label(words, "Prev", find_band)
                            nxt = match_strip_label(words, "Next", find_band)
                            if prev is not None and nxt is not None:
                                await asyncio.sleep(.5 * TIME_SCALE)
                                await asyncio.to_thread(screenshot, window, opened, env)
                                return opened, prev, nxt
                        if time.monotonic() >= deadline:
                            raise RuntimeError(
                                f"{tag}: Find labels absent: pixels={pixels} "
                                f"band={find_band} words={words[:40]}"
                            )
                        await asyncio.sleep(.4 * TIME_SCALE)

                def find_input_point(prev):
                    x = max(120, prev[0] - 320)
                    y = (prev[1] + prev[3]) // 2
                    if not (100 < x < prev[0] - 150 and find_band[1] <= y < find_band[3]):
                        raise RuntimeError(
                            f"Find input point ({x}, {y}) outside expected input "
                            f"left of Prev {prev} in {find_band}"
                        )
                    return x, y

                def find_pane_click():
                    # The split layout has a divider at width/2; click inside the right pane.
                    x = (width * 3) // 4
                    y = 280
                    if not (0 <= x < width and 0 <= y < height
                            and abs(x - width // 2) >= 100
                            and not (find_band[0] <= x < find_band[2]
                                     and find_band[1] <= y < find_band[3])):
                        raise RuntimeError(
                            f"terminal click ({x}, {y}) invalid: width={width}, "
                            f"find_band={find_band}"
                        )
                    command("xdotool", "windowfocus", window, "mousemove",
                            "--window", window, str(x), str(y), "click", "1", env=env)

                async def find_echo(token):
                    command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                            f"echo:{token}", env=env)
                    command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                    try:
                        await wait_for(
                            lambda: f"NATIVE_ECHO:{token}" in events.path.read_text(
                                encoding="utf-8").splitlines(), f"shell echo {token}",
                        )
                    except RuntimeError:
                        timeout_shot = destination / f"connected-find-echo-{token}-timeout.png"
                        await asyncio.to_thread(screenshot, window, timeout_shot, env)
                        print(timeout_shot, flush=True)
                        raise

                # P1: pane ownership, then click back into unchanged query.
                p1_open, p1_prev, _p1_next = await open_find_and_locate("p1")
                p1_input = find_input_point(p1_prev)
                probe_state["phase"] = "FIND"
                try:
                    p1_initial = count("UNEXPECTED_FIND_PTY_INPUT")
                    command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                            "NATIVE_ECHO", env=env)
                    await asyncio.sleep(.5 * TIME_SCALE)
                    p1_typed = count("UNEXPECTED_FIND_PTY_INPUT")
                    if p1_typed != p1_initial:
                        raise RuntimeError(f"P1 query reached PTY: {p1_initial}->{p1_typed}")
                finally:
                    probe_state["phase"] = None
                find_pane_click()
                await asyncio.sleep(.5 * TIME_SCALE)
                await find_echo("FIND_P1_PANE")
                p1_frame = destination / "connected-find-p1-pane.png"
                await asyncio.sleep(.5 * TIME_SCALE)
                await asyncio.to_thread(screenshot, window, p1_frame, env)
                command("xdotool", "windowfocus", window, "mousemove", "--window",
                        window, str(p1_input[0]), str(p1_input[1]), "click", "1", env=env)
                await asyncio.sleep(.5 * TIME_SCALE)
                # The open frame must reflect the settled strip just before Escape.
                p1_frame = destination / "connected-find-p1-clickback.png"
                await asyncio.to_thread(screenshot, window, p1_frame, env)
                probe_state["phase"] = "FIND"
                try:
                    p1_before_escape = count("UNEXPECTED_FIND_PTY_INPUT")
                    command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                    p1_closed, _p1_words = await wait_find_strip_closed(
                        app, window, p1_frame,
                        destination / "connected-find-p1-closed.png", find_band, env,
                        label="Escape after clicking back into Find",
                    )
                    p1_after_escape = count("UNEXPECTED_FIND_PTY_INPUT")
                    if p1_after_escape != p1_before_escape:
                        raise RuntimeError(
                            f"P1 Escape reached PTY: {p1_before_escape}->{p1_after_escape}; "
                            f"closed pixels={p1_closed}"
                        )
                finally:
                    probe_state["phase"] = None
                await find_echo("FIND_P1_AFTER")
                print(f"P1 click-back: input={p1_input}, closed pixels={p1_closed}, "
                      f"PTY count={p1_initial}->{p1_typed}, "
                      f"Escape count={p1_before_escape}->{p1_after_escape}", flush=True)

                # P2: a navigation button must focus the query, not steal keys.
                p2_open, _p2_prev, p2_next = await open_find_and_locate("p2")
                probe_state["phase"] = "FIND"
                try:
                    p2_initial = count("UNEXPECTED_FIND_PTY_INPUT")
                    command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                            "NATIVE_ECHO", env=env)
                    await asyncio.sleep(.5 * TIME_SCALE)
                    p2_typed = count("UNEXPECTED_FIND_PTY_INPUT")
                    if p2_typed != p2_initial:
                        raise RuntimeError(f"P2 query reached PTY: {p2_initial}->{p2_typed}")
                finally:
                    probe_state["phase"] = None
                find_pane_click()
                await asyncio.sleep(.5 * TIME_SCALE)
                await find_echo("FIND_P2_PANE")
                await asyncio.sleep(.5 * TIME_SCALE)
                p2_relocated = destination / "connected-find-p2-relocated.png"
                await asyncio.to_thread(screenshot, window, p2_relocated, env)
                p2_words = await asyncio.to_thread(read_strip_words, p2_relocated, find_band)
                p2_next = match_strip_label(p2_words, "Next", find_band)
                if p2_next is None:
                    raise RuntimeError(
                        f"P2 Next absent after pane echo: band={find_band}, "
                        f"words={p2_words[:40]}"
                    )
                p2_nav = center(p2_next)
                command("xdotool", "windowfocus", window, "mousemove", "--window",
                        window, str(p2_nav[0]), str(p2_nav[1]), "click", "1", env=env)
                await asyncio.sleep(.5 * TIME_SCALE)
                p2_a = destination / "connected-find-p2-nav-before-type.png"
                await asyncio.to_thread(screenshot, window, p2_a, env)
                probe_state["phase"] = "FIND"
                try:
                    p2_before = count("UNEXPECTED_FIND_PTY_INPUT")
                    command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                            "ZZQ", env=env)
                    await asyncio.sleep(.5 * TIME_SCALE)
                    p2_b = destination / "connected-find-p2-nav-after-type.png"
                    await asyncio.to_thread(screenshot, window, p2_b, env)
                    p2_pixels = await asyncio.to_thread(
                        band_changed_pixels, p2_a, p2_b, find_band,
                    )
                    p2_after = count("UNEXPECTED_FIND_PTY_INPUT")
                    # 30 changed band pixels assumes three query glyphs exceed caret noise.
                    if p2_after != p2_before or p2_pixels < 30:
                        raise RuntimeError(
                            f"P2 Next did not focus query: band pixels={p2_pixels} "
                            f"(need >=30), PTY={p2_before}->{p2_after}, "
                            f"Next={p2_next} band={find_band}"
                        )
                    command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                    p2_closed, _p2_words = await wait_find_strip_closed(
                        app, window, p2_b,
                        destination / "connected-find-p2-closed.png", find_band, env,
                        label="Escape after Next then typing",
                    )
                    p2_escaped = count("UNEXPECTED_FIND_PTY_INPUT")
                    if p2_escaped != p2_before:
                        raise RuntimeError(
                            f"P2 Escape reached PTY: {p2_before}->{p2_escaped}; "
                            f"band pixels={p2_pixels}, close pixels={p2_closed}"
                        )
                finally:
                    probe_state["phase"] = None
                await find_echo("FIND_P2_AFTER")
                print(f"P2 Next then type: Next={p2_next}, band pixels={p2_pixels}, "
                      f"close pixels={p2_closed}, PTY={p2_before}->{p2_after}"
                      f"->{p2_escaped}", flush=True)

                # P3: Find swallows other chords. Edit > Command Palette is a
                # mouse path; dismissing it gives the terminal keyboard ownership
                # until the input is clicked again (src/iced_app/menu.rs:73-79).
                p3_open, p3_prev, _p3_next = await open_find_and_locate("p3")
                p3_input = find_input_point(p3_prev)
                probe_state["phase"] = "FIND"
                try:
                    p3_initial = count("UNEXPECTED_FIND_PTY_INPUT")
                    command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                            "NATIVE_ECHO", env=env)
                    await asyncio.sleep(.5 * TIME_SCALE)
                    p3_typed = count("UNEXPECTED_FIND_PTY_INPUT")
                    if p3_typed != p3_initial:
                        raise RuntimeError(f"P3 query reached PTY: {p3_initial}->{p3_typed}")
                    p3_chord_before = destination / "connected-find-p3-before-chord.png"
                    p3_chord_after = destination / "connected-find-p3-after-chord.png"
                    await asyncio.to_thread(screenshot, window, p3_chord_before, env)
                    command("xdotool", "windowfocus", window, "key",
                            "--clearmodifiers", "ctrl+shift+p", env=env)
                    await asyncio.sleep(1.0 * TIME_SCALE)
                    await asyncio.to_thread(screenshot, window, p3_chord_after, env)
                    p3_chord_pixels = await asyncio.to_thread(
                        diff, p3_chord_before, p3_chord_after, env,
                    )
                    p3_chord_pty = count("UNEXPECTED_FIND_PTY_INPUT")
                    if p3_chord_pixels >= 2000 or p3_chord_pty != p3_initial:
                        raise RuntimeError(
                            f"P3 Find did not swallow Ctrl+Shift+P: whole-window "
                            f"pixels={p3_chord_pixels} (need <2000), "
                            f"PTY={p3_initial}->{p3_chord_pty}"
                        )
                    print(f"P3 swallowed chord: whole-window pixels={p3_chord_pixels}, "
                          f"PTY={p3_initial}->{p3_chord_pty}", flush=True)
                finally:
                    probe_state["phase"] = None

                # Locate the top-menu title and its dropdown item on screen;
                # the strip remains open but a mouse click releases its focus.
                p3_frame = destination / "connected-find-p3-before-dialog.png"
                await asyncio.to_thread(screenshot, window, p3_frame, env)
                await click_label(
                    window, destination / "connected-find-p3-edit-menu.png",
                    "Edit", env, region=(0, 0, width, 32), settle=.5 * TIME_SCALE,
                )
                await click_label(
                    window, destination / "connected-find-p3-palette-item.png",
                    "Palette", env, region=(0, 32, min(width, 360), min(height, 320)),
                    settle=.5 * TIME_SCALE,
                )
                p3_dialog = destination / "connected-find-p3-palette.png"
                p3_dialog_pixels = 0
                p3_deadline = time.monotonic() + 8 * TIME_SCALE
                while True:
                    if app.poll() is not None:
                        raise RuntimeError(f"P3 GUI exited opening Command Palette ({app.returncode})")
                    await asyncio.to_thread(screenshot, window, p3_dialog, env)
                    p3_dialog_pixels = await asyncio.to_thread(diff, p3_frame, p3_dialog, env)
                    if p3_dialog_pixels >= 2000:
                        break
                    if time.monotonic() >= p3_deadline:
                        raise RuntimeError(
                            f"P3 Command Palette did not open by menu: whole-window "
                            f"pixels={p3_dialog_pixels} (need >=2000), "
                            f"chord pixels={p3_chord_pixels}, PTY={p3_initial}->{p3_chord_pty}"
                        )
                    await asyncio.sleep(.3 * TIME_SCALE)

                command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                p3_dismissed = destination / "connected-find-p3-dismissed.png"
                p3_overlay_box = (width // 4, height // 5,
                                  width * 3 // 4, height * 4 // 5)
                p3_dismiss_pixels = 0
                p3_overlay_pixels = 0
                p3_words = []
                p3_deadline = time.monotonic() + 8 * TIME_SCALE
                while True:
                    if app.poll() is not None:
                        raise RuntimeError(f"P3 GUI exited dismissing Command Palette ({app.returncode})")
                    await asyncio.to_thread(screenshot, window, p3_dismissed, env)
                    p3_dismiss_pixels = await asyncio.to_thread(
                        diff, p3_dialog, p3_dismissed, env,
                    )
                    # Compare against the OPEN palette; strip labels also show while open.
                    if p3_dismiss_pixels >= 2000:
                        p3_overlay_pixels = await asyncio.to_thread(
                            band_changed_pixels, p3_dialog, p3_dismissed,
                            p3_overlay_box,
                        )
                        if p3_overlay_pixels >= 2000:
                            break
                    if time.monotonic() >= p3_deadline:
                        raise RuntimeError(
                            f"P3 Command Palette did not dismiss: pixels vs open="
                            f"{p3_dismiss_pixels} (need >=2000), central="
                            f"{p3_overlay_pixels} (need >=2000), "
                            f"band={find_band} words={p3_words[:40]} "
                            f"(not read: overlay did not change)"
                        )
                    await asyncio.sleep(.3 * TIME_SCALE)
                # One OCR read after the overlay has repainted, never in the poll.
                p3_words = await asyncio.to_thread(
                    read_strip_words, p3_dismissed, find_band,
                )
                if (match_strip_label(p3_words, "Prev", find_band) is None
                        or match_strip_label(p3_words, "Next", find_band) is None):
                    raise RuntimeError(
                        f"P3 Find band absent after Palette dismissal: pixels vs open="
                        f"{p3_dismiss_pixels}, central={p3_overlay_pixels}, "
                        f"band={find_band} words={p3_words[:40]}"
                    )
                print(f"P3 Palette dismissed: pixels vs open={p3_dismiss_pixels}, "
                      f"central={p3_overlay_pixels}, band={find_band} "
                      f"words={p3_words[:40]}", flush=True)

                # No terminal click: a dialog dismissal must return keys to SSH.
                await find_echo("FIND_P3_PANE")
                p3_relocated = destination / "connected-find-p3-relocated.png"
                await asyncio.to_thread(screenshot, window, p3_relocated, env)
                p3_words = await asyncio.to_thread(read_strip_words, p3_relocated, find_band)
                p3_prev = match_strip_label(p3_words, "Prev", find_band)
                if p3_prev is None:
                    raise RuntimeError(
                        f"P3 Prev absent after pane echo: band={find_band}, "
                        f"words={p3_words[:40]}"
                    )
                p3_input = find_input_point(p3_prev)
                command("xdotool", "windowfocus", window, "mousemove", "--window",
                        window, str(p3_input[0]), str(p3_input[1]), "click", "1", env=env)
                await asyncio.sleep(.5 * TIME_SCALE)
                p3_a = destination / "connected-find-p3-clickback-before-type.png"
                await asyncio.to_thread(screenshot, window, p3_a, env)
                probe_state["phase"] = "FIND"
                try:
                    p3_before = count("UNEXPECTED_FIND_PTY_INPUT")
                    command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                            "ZZQ", env=env)
                    await asyncio.sleep(.5 * TIME_SCALE)
                    p3_b = destination / "connected-find-p3-clickback-after-type.png"
                    await asyncio.to_thread(screenshot, window, p3_b, env)
                    p3_pixels = await asyncio.to_thread(
                        band_changed_pixels, p3_a, p3_b, find_band,
                    )
                    p3_after = count("UNEXPECTED_FIND_PTY_INPUT")
                    if p3_after != p3_before or p3_pixels < 30:
                        raise RuntimeError(
                            f"P3 click-back did not re-own Find: band pixels="
                            f"{p3_pixels} (need >=30), PTY={p3_before}->{p3_after}, "
                            f"palette pixels={p3_dialog_pixels}, "
                            f"dismiss pixels={p3_dismiss_pixels}"
                        )
                    command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                    p3_closed, _p3_closed_words = await wait_find_strip_closed(
                        app, window, p3_b,
                        destination / "connected-find-p3-closed.png", find_band, env,
                        label="P3 Escape after dialog and click-back",
                    )
                    p3_escaped = count("UNEXPECTED_FIND_PTY_INPUT")
                    if p3_escaped != p3_before:
                        raise RuntimeError(
                            f"P3 Escape reached PTY: {p3_before}->{p3_escaped}; "
                            f"band pixels={p3_pixels}, close pixels={p3_closed}"
                        )
                finally:
                    probe_state["phase"] = None
                await find_echo("FIND_P3_AFTER")
                print(f"P3 dialog hand-back: chord pixels={p3_chord_pixels}, "
                      f"palette pixels={p3_dialog_pixels}, dismiss pixels={p3_dismiss_pixels}, "
                      f"input={p3_input}, band pixels={p3_pixels}, close pixels={p3_closed}, "
                      f"PTY={p3_initial}->{p3_typed}->{p3_chord_pty}"
                      f"->{p3_before}->{p3_after}->{p3_escaped}", flush=True)

                # The populated queue lives in the Transfer tab. Both completed
                # jobs must be listed; scroll its visible interior and retain
                # the resulting production screenshot.
                queue_shot = destination / "connected-files-queue.png"
                await click_label(window, queue_shot, "Transfer", env, region=strip, settle=.8)
                queue_bounds = await locate_rendered_label(
                    window, queue_shot, download_name, env=env,
                    region=body, timeout=40,
                )
                await asyncio.to_thread(
                    locate_visible_label, queue_shot, upload_name, region=body,
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(queue_bounds[0] + 40), str(queue_bounds[3] + 24),
                    "click", "4", env=env,
                )
                await asyncio.sleep(.4)
                screenshot(window, destination / "connected-files-queue-scrolled.png", env)
                await click_label(
                    window, destination / "connected-files-filer-again.png",
                    "Filer", env, region=strip, settle=.8,
                )
                await locate_rendered_label(
                    window, destination / "connected-files-filer-again.png",
                    third_name, env=env, region=body, timeout=40,
                )

                # Minimum-size acceptance is interaction, not a screenshot-only
                # artifact. The same distinct row must remain visibly targetable.
                command("xdotool", "windowsize", window, "960", "640", env=env)
                await asyncio.sleep(.7)
                minimum_geometry = command(
                    "xdotool", "getwindowgeometry", "--shell", window, env=env,
                )
                if "WIDTH=960" not in minimum_geometry or "HEIGHT=640" not in minimum_geometry:
                    raise RuntimeError(
                        "minimum-window evidence requires 960x640 geometry; "
                        f"geometry was {minimum_geometry!r}"
                    )
                screenshot(window, destination / "connected-files-min-window.png", env)
                minimum_row = await asyncio.to_thread(
                    locate_visible_label,
                    destination / "connected-files-min-window.png", third_name,
                    region=dock_body_region(960, 640),
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(minimum_row)), "click", "1", env=env,
                )
                await asyncio.sleep(.4)
                screenshot(window, destination / "connected-files-min-selected.png", env)
                command("xdotool", "windowsize", window, "1280", "800", env=env)
                await asyncio.sleep(.7)
                screenshot(window, destination / "connected-files-restored.png", env)

                # Exercise the dock gear menu bottom -> right -> bottom on the
                # active Filer panel and prove a listing remains targetable.
                # The gear is a text-less icon at (right edge - 40, strip
                # centre); derive its y from the OCR-located Filer tab.
                width, height = window_size(window, env)
                filer_tab = await asyncio.to_thread(
                    locate_visible_label, destination / "connected-files-restored.png",
                    "Filer", region=dock_strip_region(width, height),
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(width - 40), str(center(filer_tab)[1]), "click", "1", env=env,
                )
                await asyncio.sleep(.5)
                await click_label(
                    window, destination / "connected-files-gear-menu.png",
                    "Move to Right", env, settle=.9,
                    region=gear_menu_region(width, height),
                )
                screenshot(window, destination / "connected-files-side-dock.png", env)
                if diff(
                    destination / "connected-files-restored.png",
                    destination / "connected-files-side-dock.png", env,
                ) < 3000:
                    raise RuntimeError("Move to Right did not visibly re-dock Filer")
                # The bottom dock (Sender/Session/Transfer/Explorer) stays
                # below the right dock; its strip top bounds the side dock.
                bottom_top = max(32 + 100, filer_tab[1] - 8)
                side_region = dock_body_region(
                    width, height, right=True, bottom_top=bottom_top,
                )
                side_row = await locate_rendered_label(
                    window, destination / "connected-files-side-dock.png", third_name, env=env,
                    region=side_region, timeout=40,
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(side_row)), "click", "1", env=env,
                )
                await asyncio.sleep(.4)
                screenshot(window, destination / "connected-files-side-selected.png", env)

                # The dock splitter is a 5px grab column centred at
                # x = width - dock_width - 3. Drag it 200px left: the dock
                # widens to ~620px and the file row moves with its edge.
                await drag_horizontal(
                    window, env, width - SIDE_FILE_DOCK_WIDTH - 3, width - 620, 300,
                )
                screenshot(window, destination / "connected-files-side-resized.png", env)
                resized_pixels = diff(
                    destination / "connected-files-side-selected.png",
                    destination / "connected-files-side-resized.png", env,
                )
                if resized_pixels < 3000:
                    raise RuntimeError(
                        "dock splitter dragging did not resize the dock: "
                        f"{resized_pixels} pixels changed after dragging x "
                        f"{width - SIDE_FILE_DOCK_WIDTH - 3} -> {width - 620}"
                    )
                resized_row = await locate_rendered_label(
                    window, destination / "connected-files-side-resized.png",
                    third_name, env=env,
                    region=(width - 660, 32, width, bottom_top), timeout=20,
                )
                if not (width - 640 <= resized_row[0] <= width - 560):
                    raise RuntimeError(
                        "dock splitter drag did not move the side dock edge: "
                        f"file row left edge {resized_row[0]}, expected "
                        f"{width - 640}..{width - 560}"
                    )
                # Drag back so later steps see the 420px side dock.
                await drag_horizontal(
                    window, env, width - 620 - 3, width - SIDE_FILE_DOCK_WIDTH, 300,
                )
                screenshot(window, destination / "connected-files-side-restored.png", env)

                # The right dock's strip sits just under the 32px menu bar;
                # the gear is at (width - 40, strip centre). Take the strip
                # y from the OCR-located Filer tab when found, else 46.
                gear_y = 46
                try:
                    side_tab = await asyncio.to_thread(
                        locate_visible_label,
                        destination / "connected-files-side-restored.png",
                        "Filer", region=(width - 430, 32, width, 62),
                    )
                    gear_y = center(side_tab)[1]
                except RuntimeError:
                    pass
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(width - 40), str(gear_y), "click", "1", env=env,
                )
                await asyncio.sleep(.5)
                await click_label(
                    window, destination / "connected-files-gear-menu-right.png",
                    "Move to Bottom", env, settle=.9,
                    region=gear_menu_region(width, height),
                )
                screenshot(window, destination / "connected-files-redocked-bottom.png", env)
                # Move to Bottom appends Filer as the LAST bottom tab (Sender,
                # Session, Transfer, Explorer, Filer) and activates it; locate
                # the row in the dock body, never by Filer's old tab position.
                redocked_row = await locate_rendered_label(
                    window, destination / "connected-files-redocked-bottom.png",
                    third_name, env=env,
                    region=dock_body_region(width, height), timeout=40,
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(redocked_row)), "click", "1", env=env,
                )
                await asyncio.sleep(.4)
                screenshot(window, destination / "connected-files-redocked-selected.png", env)

                if EXTEDIT:
                    # Filer "Open in External Editor" on the remote 444 row.
                    # Every assertion on 444's remote bytes (download verify,
                    # Transfer queue row, name-order oracle) has already run;
                    # the dock closes right after this step, so the appended
                    # marker cannot reach a later check.
                    extedit_remote = remote_root / download_name
                    extedit_original = extedit_remote.read_bytes()
                    if extedit_original != download_bytes:
                        raise RuntimeError(
                            f"remote {download_name} changed before the external-edit step"
                        )
                    extedit_row_shot = destination / "connected-extedit-row.png"
                    extedit_row = await locate_rendered_label(
                        window, extedit_row_shot, download_name, env=env,
                        region=dock_body_region(width, height), timeout=20,
                    )
                    ex_x, ex_y = center(extedit_row)
                    ex_region = extedit_menu_region(ex_x, ex_y, width, height)
                    # Same no-pause gesture as the context-menu step: move and
                    # right-press in one xdotool command.
                    command(
                        "xdotool", "mousemove", "--window", window,
                        str(ex_x), str(ex_y), "click", "3", env=env,
                    )
                    ex_menu_shot = destination / "connected-extedit-menu.png"
                    paint_deadline = time.monotonic() + 8 * TIME_SCALE
                    while True:
                        await asyncio.to_thread(screenshot, window, ex_menu_shot, env)
                        painted = await asyncio.to_thread(diff, extedit_row_shot, ex_menu_shot, env)
                        if painted >= 50:
                            break
                        if time.monotonic() >= paint_deadline:
                            raise RuntimeError(
                                f"right-click on {download_name} at ({ex_x}, {ex_y}) did not "
                                f"repaint for the external-edit menu ({painted} pixels)"
                            )
                        await asyncio.sleep(.3)
                    try:
                        ex_item = await locate_rendered_label(
                            window, ex_menu_shot, EXTEDIT_MENU_LABEL, env=env,
                            region=ex_region, timeout=20,
                        )
                    except RuntimeError as exc:
                        raise RuntimeError(
                            f"Filer context menu did not render '{EXTEDIT_MENU_LABEL}' "
                            f"inside {ex_region} (click ({ex_x}, {ex_y}); see {ex_menu_shot.name})"
                        ) from exc
                    if _bounds_inside(ex_item, ex_region) is None:
                        raise RuntimeError(
                            f"'{EXTEDIT_MENU_LABEL}' bounds {ex_item} not inside {ex_region}"
                        )
                    command(
                        "xdotool", "mousemove", "--window", window,
                        *map(str, center(ex_item)), "click", "1", env=env,
                    )
                    ex_after_shot = destination / "connected-extedit-after.png"
                    try:
                        await wait_for(
                            extedit_argv_log.exists,
                            "fake external editor argv log (editor never launched)",
                            timeout=20 * TIME_SCALE,
                        )
                        await wait_for(
                            lambda: extedit_remote.read_bytes().endswith(EXTEDIT_MARKER),
                            f"remote {download_name} to end with the fake editor marker "
                            "(working copy never uploaded)",
                            timeout=30 * TIME_SCALE,
                        )
                    except RuntimeError:
                        await asyncio.to_thread(screenshot, window, ex_after_shot, env)
                        raise
                    await asyncio.to_thread(screenshot, window, ex_after_shot, env)
                    ex_argv = [
                        json.loads(line)
                        for line in extedit_argv_log.read_text(encoding="utf-8").splitlines()
                        if line.strip()
                    ]
                    if len(ex_argv) != 1:
                        raise RuntimeError(f"fake editor launched {len(ex_argv)} times: {ex_argv}")
                    if Path(ex_argv[0][-1]).name != download_name:
                        raise RuntimeError(
                            f"fake editor last argument {ex_argv[0][-1]!r} is not a "
                            f"working copy named {download_name}"
                        )
                    verdict = extedit_bytes_verdict(
                        extedit_original, extedit_remote.read_bytes(), EXTEDIT_MARKER,
                    )
                    if verdict is not None:
                        raise RuntimeError(f"external-edit upload: {verdict}")
                    print(
                        f"PASS external edit: editor argv {ex_argv[0]}, remote "
                        f"{download_name} == original + marker", flush=True,
                    )
                    command(
                        "xdotool", "windowfocus", window, "mousemove", "--window", window,
                        "640", "280", "click", "1", env=env,
                    )
                    await asyncio.sleep(.3)
                    command(
                        "xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "echo:AFTER_EXTERNAL_EDIT", env=env,
                    )
                    command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                    await wait_for(
                        lambda: "NATIVE_ECHO:AFTER_EXTERNAL_EDIT" in events.path.read_text(encoding="utf-8").splitlines(),
                        "shell echo after Open in External Editor",
                    )

                # The file browser has completed the real SFTP handshake.
                # End that UI task before testing the independent local-editor
                # workflow; otherwise its remote listing can keep keyboard
                # ownership while focus changes. Closing the dock does not
                # close either underlying authenticated SSH PTY.
                command(
                    "xdotool", "windowfocus", window, "key",
                    "--clearmodifiers", "ctrl+shift+o", env=env,
                )
                await asyncio.sleep(.7)
                screenshot(window, destination / "connected-files-closed.png", env)
                closed_pixels = diff(
                    destination / "connected-files-dock.png",
                    destination / "connected-files-closed.png", env,
                )
                if closed_pixels < 3000:
                    raise RuntimeError(
                        f"Files dock did not close after authenticated SFTP: "
                        f"{closed_pixels} pixels changed"
                    )
                command(
                    "xdotool", "windowfocus", window, "mousemove",
                    "--window", window, "710", "300", "click", "1", env=env,
                )
                await asyncio.sleep(.5)

                # Opening Free Type must focus the new pane-local text_editor:
                # typing goes into the draft, and even Enter is NOT forwarded
                # to the authenticated OpenSSH PTY until an explicit Send.
                screenshot(window, destination / "connected-before-free-type.png", env)
                command(
                    "xdotool", "windowfocus", window, "key", "--clearmodifiers",
                    "ctrl+shift+e", env=env,
                )
                await asyncio.sleep(.9)
                screenshot(window, destination / "connected-free-type-empty.png", env)
                mode_pixels = diff(
                    destination / "connected-before-free-type.png",
                    destination / "connected-free-type-empty.png", env,
                )
                if mode_pixels < 300:
                    raise RuntimeError(
                        f"free-type shortcut did not visibly open editor: "
                        f"{mode_pixels} pixels changed"
                    )
                command(
                    "xdotool", "type", "--clearmodifiers", "--delay", "30",
                    "echo:FREE_TYPE_PROBE", env=env,
                )
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await asyncio.sleep(.9)
                screenshot(window, destination / "connected-free-type-draft.png", env)
                free_type_pixels = diff(
                    destination / "connected-free-type-empty.png",
                    destination / "connected-free-type-draft.png", env,
                )
                if free_type_pixels < 250:
                    raise RuntimeError(
                        f"free-type keyboard text did not visibly populate the local editor: "
                        f"{free_type_pixels} pixels changed"
                    )
                # Free Type's exit shortcut discards its local draft. Validate
                # Iced focus-mode and privacy-mode keyboard entry/exit in the
                # same connected split-pane production process.
                command("xdotool", "windowfocus", window, "key", "--clearmodifiers",
                        "ctrl+shift+e", env=env)
                await asyncio.sleep(.5)
                screenshot(window, destination / "connected-before-focus-mode.png", env)
                command("xdotool", "windowfocus", window, "key", "--clearmodifiers",
                        "alt+Return", env=env)
                await asyncio.sleep(.6)
                screenshot(window, destination / "connected-focus-mode.png", env)
                focus_pixels = diff(
                    destination / "connected-before-focus-mode.png",
                    destination / "connected-focus-mode.png", env,
                )
                if focus_pixels < 3000:
                    raise RuntimeError(
                        f"connected focus-mode shortcut changed only {focus_pixels} pixels"
                    )
                command("xdotool", "windowfocus", window, "key", "--clearmodifiers",
                        "alt+Return", env=env)
                await asyncio.sleep(.5)

                command("xdotool", "windowfocus", window, "key", "--clearmodifiers",
                        "ctrl+shift+l", env=env)
                await asyncio.sleep(.6)
                screenshot(window, destination / "connected-privacy-curtain.png", env)
                privacy_pixels = diff(
                    destination / "connected-before-focus-mode.png",
                    destination / "connected-privacy-curtain.png", env,
                )
                if privacy_pixels < 3000:
                    raise RuntimeError(
                        f"privacy curtain did not visibly obscure SSH panes: "
                        f"{privacy_pixels} pixels changed"
                    )
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "echo:LOCK_PROBE", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await asyncio.sleep(.6)
                command("xdotool", "windowfocus", window, "key", "--clearmodifiers",
                        "ctrl+shift+l", env=env)
                await asyncio.sleep(.4)

                # The remote end emits the same VT alternate-screen enter/
                # leave sequences used by full-screen programs. These bytes
                # travel through a real OpenSSH PTY and production Iced canvas.
                # This is bounded protocol evidence, not a Vim usability claim.
                command("xdotool", "mousemove", "--window", window,
                        "710", "300", "click", "1", env=env)
                await asyncio.sleep(.3)
                screenshot(window, destination / "connected-fullscreen-before.png", env)
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "screen-on", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "SCREEN_ON" in events.path.read_text(encoding="utf-8"),
                    "live SSH alternate-screen enter",
                )
                # SCREEN_ON is a server event too: poll until the alternate
                # screen is painted, or the "active" frame is the normal screen
                # and the restore below diffs to nothing.
                active_deadline = time.monotonic() + 10 * TIME_SCALE
                while True:
                    await asyncio.sleep(.5)
                    screenshot(window, destination / "connected-fullscreen-active.png", env)
                    active_pixels = diff(
                        destination / "connected-fullscreen-before.png",
                        destination / "connected-fullscreen-active.png", env,
                    )
                    if active_pixels >= 1500 or time.monotonic() >= active_deadline:
                        break
                if active_pixels < 1500:
                    raise RuntimeError(
                        f"alternate-screen enter changed only {active_pixels} pixels"
                    )
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "screen-off", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "SCREEN_OFF" in events.path.read_text(encoding="utf-8"),
                    "live SSH alternate-screen leave",
                )
                # SCREEN_OFF is a server event, not a repaint: on a loaded
                # host the canvas can still show the alternate screen half a
                # second later. Poll until the restore is visible.
                restore_deadline = time.monotonic() + 10 * TIME_SCALE
                while True:
                    await asyncio.sleep(.5)
                    screenshot(window, destination / "connected-fullscreen-restored.png", env)
                    fullscreen_pixels = diff(
                        destination / "connected-fullscreen-active.png",
                        destination / "connected-fullscreen-restored.png", env,
                    )
                    if fullscreen_pixels >= 1500 or time.monotonic() >= restore_deadline:
                        break
                if fullscreen_pixels < 1500:
                    raise RuntimeError(
                        f"alternate-screen restore changed only {fullscreen_pixels} pixels"
                    )

                # #64: production Iced keyboard shortcut actually toggles a
                # dedicated remote PTY navigation mode, and h sends a real
                # Left cursor sequence to the authenticated synthetic shell.
                # Exit the modal mode before Return completes the test line.
                command(
                    "xdotool", "windowfocus", window, "key",
                    "--clearmodifiers", "ctrl+shift+m", env=env,
                )
                await asyncio.sleep(.5)
                screenshot(window, destination / "connected-remote-keys.png", env)
                remote_keys_pixels = diff(
                    destination / "connected-fullscreen-restored.png",
                    destination / "connected-remote-keys.png", env,
                )
                if remote_keys_pixels < 300:
                    raise RuntimeError(
                        f"remote PTY navigation mode was not visibly activated: "
                        f"{remote_keys_pixels} pixels changed"
                    )
                command("xdotool", "key", "--clearmodifiers", "h", env=env)
                await asyncio.sleep(.7)
                command(
                    "xdotool", "key", "--clearmodifiers", "ctrl+shift+m", env=env,
                )
                await asyncio.sleep(.5)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                try:
                    await wait_for(
                        lambda: "REMOTE_CURSOR_LEFT" in events.path.read_text(encoding="utf-8"),
                        "native OpenSSH remote cursor Left after Iced modal h key",
                    )
                except RuntimeError as exc:
                    raise RuntimeError(
                        f"{exc}. Synthetic SSH lifecycle: "
                        + repr(events.path.read_text(encoding="utf-8")[-1300:])
                        + "; opt-in key dispatch: "
                        + repr(
                            "\n".join(
                                line for line in app_log.read_text(encoding="utf-8").splitlines()
                                if line.startswith("ci_remote_key:")
                            )[-1200:]
                        )
                    ) from exc

                # Session > Exit confirmation. Placed after the remote-key
                # probe (line above: no menu or dialog is open, focus mode
                # and remote-keys mode are off, the SSH session is still up)
                # and before the event_log snapshot so an accidental confirm
                # also fails the "unexpectedly exited" check. This path clicks
                # Cancel only. The confirming Exit click is the window-manager
                # close step after that snapshot.
                command("xdotool", "mousemove", "--window", window,
                        "30", "16", "click", "1", env=env)
                await asyncio.sleep(.3)
                # Session dropdown hangs from the 32px menu bar at x=0 and is
                # 240px wide. Exit is the last item: 7*24px rows + 4*1px
                # separators put it near y=205..229. Search only that
                # top-left band so the status bar, or a pane-toolbar
                # "Exit focus" / "Exit keys", cannot match.
                await click_label(
                    window, destination / "connected-exit-menu.png",
                    "Exit", env, region=(0, 140, 270, 280), settle=.6,
                )
                width, height = window_size(window, env)
                # Confirmation body is one line in a 600px dialog centered
                # on the window. "Exit and disconnect" matches both the
                # singular and plural wording and avoids the leading session
                # count (0/O).
                confirm_region = (
                    max(0, width // 2 - 340),
                    max(0, height // 2 - 180),
                    min(width, width // 2 + 340),
                    min(height, height // 2 + 180),
                )
                confirm = await locate_rendered_label(
                    window, destination / "connected-exit-confirm.png",
                    "Exit and disconnect", env=env, region=confirm_region, timeout=15,
                )
                # The matched phrase starts further left than "disconnect them"
                # did; the existing -280px left expansion already covers the
                # dialog, so it is not widened. The phrase ends ~5 characters
                # earlier, so Cancel (left of the two right-aligned buttons,
                # bottom right of the 600px dialog, one row below) sits further
                # right of confirm[2]. The old +300px covered an ~80-170px gap;
                # +360px keeps that cover. height-24 keeps the box off the
                # status bar. Both confirmation paths use this same cover.
                def exit_dialog_region(phrase_bounds, win_width, win_height):
                    region = (
                        max(0, phrase_bounds[0] - 280),
                        max(0, phrase_bounds[1] - 56),
                        min(win_width, phrase_bounds[2] + 360),
                        min(win_height - 24, phrase_bounds[3] + 72),
                    )
                    if region[2] <= region[0] or region[3] <= region[1]:
                        raise RuntimeError(
                            "Exit confirmation dialog region collapsed from "
                            f"{phrase_bounds}: {region}"
                        )
                    return region

                dialog_region = exit_dialog_region(confirm, width, height)
                await click_label(
                    window, destination / "connected-exit-cancel.png",
                    "Cancel", env, region=dialog_region, settle=.5,
                )
                try:
                    command("xdotool", "getwindowgeometry", "--shell", window, env=env)
                except subprocess.CalledProcessError as exc:
                    raise RuntimeError("Exit Cancel closed the application window") from exc
                # A backdrop click can dismiss the dialog too, so prove it is
                # gone before the workspace click. Repaint can lag on a
                # loaded host; a miss is slow, so each attempt is a region
                # OCR with a small budget.
                cancel_shot = destination / "connected-after-exit-cancel.png"
                gone_deadline = time.monotonic() + 8 * TIME_SCALE
                while True:
                    await asyncio.to_thread(screenshot, window, cancel_shot, env)
                    try:
                        still = await asyncio.to_thread(
                            locate_visible_label,
                            cancel_shot,
                            "Exit and disconnect",
                            region=confirm_region,
                            budget=1.0,
                        )
                    except RuntimeError:
                        break
                    if time.monotonic() >= gone_deadline:
                        raise RuntimeError(
                            "Exit confirmation still visible after Cancel "
                            f"at {still} in {confirm_region}"
                        )
                    await asyncio.sleep(.4)
                removed_pixels = await asyncio.to_thread(
                    diff,
                    destination / "connected-exit-confirm.png",
                    cancel_shot,
                    env,
                )
                print(
                    f"Exit Cancel dialog removal: {removed_pixels} pixels "
                    f"(threshold 5000)",
                    flush=True,
                )
                if removed_pixels < 5000:
                    raise RuntimeError(
                        "Exit confirmation backdrop still present after Cancel: "
                        f"{removed_pixels} pixels changed (need >= 5000)"
                    )
                # Cancel closes the dialog but, unlike Escape after the
                # Session menu, does not restore terminal focus. Click the
                # pane the alternate-screen probe used so this echo proves
                # the session is still alive, not that focus came back.
                command(
                    "xdotool", "windowfocus", window, "mousemove", "--window", window,
                    "710", "300", "click", "1", env=env,
                )
                await asyncio.sleep(.3)
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "echo:AFTER_EXIT_CANCEL", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "NATIVE_ECHO:AFTER_EXIT_CANCEL" in events.path.read_text(encoding="utf-8").splitlines(),
                    "shell echo after cancelling Session Exit",
                )

                # Mandatory Tunnels probe: top-right Tunnels control opens a
                # right-hand popup offering "Tunnel Settings...", which opens
                # the Tunnel Manager; Escape closes it and the terminal must
                # still own the keyboard (no terminal re-click). Saved
                # forwards are not guaranteed by the fixture, so no toggles.
                tun_width, tun_height = window_size(window, env)
                tunnels_bounds = await click_label(
                    window, destination / "connected-tunnels-button.png", "Tunnels", env,
                    region=(max(0, tun_width - 220), 0, tun_width, 42), timeout=15,
                )
                popup_region = (
                    max(0, tun_width - 440),
                    tunnels_bounds[1],
                    tun_width,
                    min(tun_height, tunnels_bounds[3] + 420),
                )
                settings_bounds = await locate_rendered_label(
                    window, destination / "connected-tunnels-popup.png",
                    "Tunnel Settings...", env=env, region=popup_region, timeout=15,
                )
                if not _bounds_inside(settings_bounds, popup_region):
                    raise RuntimeError(
                        f"Tunnel Settings... {settings_bounds} outside right-hand "
                        f"Tunnels popup region {popup_region}"
                    )
                if POPULATED:
                    # Both saved forwards render as distinct rows above the
                    # footer, nothing running yet (TCP-checked, not OCR).
                    row_boxes = []
                    for label in POPULATED_LABELS:
                        box = await locate_rendered_label(
                            window, destination / "connected-tunnels-popup-idle.png",
                            label, env=env, region=popup_region, timeout=15,
                        )
                        if not (_bounds_inside(box, popup_region) and box[3] <= settings_bounds[1]):
                            raise RuntimeError(f"{label} row {box} not inside popup list")
                        row_boxes.append(box)
                    if abs(row_boxes[0][1] - row_boxes[1][1]) < 20:
                        raise RuntimeError(f"tunnel rows not distinct: {row_boxes}")
                    for port in forward_ports:
                        await forward_refused(port, "before any toggle")
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(settings_bounds)), "click", "1", env=env,
                )
                await asyncio.sleep(.6)
                heading_bounds = await locate_rendered_label(
                    window, destination / "connected-tunnels-settings.png",
                    "Tunnel Manager", env=env, timeout=15,
                )
                if POPULATED:
                    # Select each saved tunnel in the editor list. The list
                    # label renders regardless of selection, so proof is the
                    # selected forward's unique listen port in the right-hand
                    # form with the sibling's port absent (no-op click fails).
                    ed_w, ed_h = window_size(window, env)
                    form_region = editor_form_region(ed_w, ed_h)
                    listen_row = listen_port_row_region(heading_bounds, ed_w)
                    list_region = (0, 42, ed_w // 2, ed_h)
                    for idx, (label, name) in enumerate(
                        zip(POPULATED_LABELS, ("alpha", "bravo"))
                    ):
                        await click_label(
                            window, destination / f"connected-tunnels-editor-pre-{name}.png",
                            label, env, region=list_region, timeout=15, settle=.8,
                        )
                        shot = destination / f"connected-tunnels-editor-selected-{name}.png"
                        deadline = time.monotonic() + 15 * TIME_SCALE
                        while True:
                            await asyncio.to_thread(screenshot, window, shot, env)
                            try:
                                await asyncio.to_thread(
                                    require_editor_selection, shot,
                                    forward_ports[idx], forward_ports[1 - idx], form_region,
                                    row_region=listen_row,
                                )
                                break
                            except RuntimeError:
                                if time.monotonic() >= deadline:
                                    raise
                                await asyncio.sleep(.5)
                command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                await asyncio.sleep(.6)
                tunnels_closed = destination / "connected-tunnels-closed.png"
                await asyncio.to_thread(screenshot, window, tunnels_closed, env)
                try:
                    await asyncio.to_thread(
                        locate_visible_label, tunnels_closed, "Tunnel Manager",
                    )
                except RuntimeError:
                    pass
                else:
                    raise RuntimeError("Escape did not close the Tunnel Manager")
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "echo:AFTER_TUNNEL_SETTINGS", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "NATIVE_ECHO:AFTER_TUNNEL_SETTINGS" in events.path.read_text(encoding="utf-8").splitlines(),
                    "shell echo after closing Tunnel Manager without terminal click",
                )

                if POPULATED:
                    # Functional per-forward toggles. Truth is the loopback
                    # listener (echo through the fixture SSH server, or a
                    # real refusal), never the screenshot.
                    port_a, port_b = forward_ports
                    label_a, label_b = POPULATED_LABELS

                    async def popup_row(label: str, shot: Path):
                        for attempt in range(2):
                            try:
                                return await locate_rendered_label(
                                    window, shot, label, env=env,
                                    region=popup_region, timeout=8,
                                )
                            except RuntimeError:
                                if attempt:
                                    raise
                                await click_label(
                                    window, destination / "connected-tunnels-reopen.png",
                                    "Tunnels", env, timeout=10,
                                    region=(max(0, tun_width - 220), 0, tun_width, 42),
                                )

                    async def toggle(label: str, name: str) -> None:
                        shot = destination / f"connected-tunnels-{name}-before.png"
                        box = await popup_row(label, shot)
                        (x, y), source = await asyncio.to_thread(
                            toggler_target, shot, box, tun_width,
                        )
                        print(f"tunnel toggle {name}: {label} row {box} -> ({x},{y}) via {source}",
                              flush=True)
                        command("xdotool", "mousemove", "--window", window,
                                str(x), str(y), "click", "1", env=env)
                        await asyncio.sleep(1)

                    async def open_manager(tag: str):
                        """Tunnels -> Tunnel Settings...; return the heading box."""
                        await click_label(
                            window, destination / f"connected-tunnels-{tag}-button.png",
                            "Tunnels", env, region=(max(0, tun_width - 220), 0, tun_width, 42),
                            timeout=15,
                        )
                        footer = await locate_rendered_label(
                            window, destination / f"connected-tunnels-{tag}-popup.png",
                            "Tunnel Settings...", env=env, region=popup_region, timeout=15,
                        )
                        command("xdotool", "mousemove", "--window", window,
                                *map(str, center(footer)), "click", "1", env=env)
                        await asyncio.sleep(.6)
                        return await locate_rendered_label(
                            window, destination / f"connected-tunnels-{tag}-manager.png",
                            "Tunnel Manager", env=env, timeout=15,
                        )

                    async def require_form(port, sibling, heading, tag):
                        w, h = window_size(window, env)
                        shot = destination / f"connected-tunnels-{tag}.png"
                        deadline = time.monotonic() + 15 * TIME_SCALE
                        while True:
                            await asyncio.to_thread(screenshot, window, shot, env)
                            try:
                                return await asyncio.to_thread(
                                    require_editor_selection, shot, port, sibling,
                                    editor_form_region(w, h),
                                    row_region=listen_port_row_region(heading, w),
                                )
                            except RuntimeError:
                                if time.monotonic() >= deadline:
                                    raise
                                await asyncio.sleep(.5)

                    async def select_row(label, port, sibling, heading, tag):
                        """Click a list row; prove it by the Listen Port row."""
                        w, h = window_size(window, env)
                        await click_label(
                            window, destination / f"connected-tunnels-{tag}-pre.png",
                            label, env, region=(0, 42, w // 2, h), timeout=15, settle=.8,
                        )
                        return await require_form(port, sibling, heading, tag)

                    async def type_listen_port(heading, value: int) -> None:
                        w, _h = window_size(window, env)
                        x, y = listen_input_centre(heading, w)
                        command("xdotool", "mousemove", "--window", window,
                                str(x), str(y), "click", "1", env=env)
                        await asyncio.sleep(.3)
                        command("xdotool", "key", "--clearmodifiers", "ctrl+a", env=env)
                        command("xdotool", "type", "--clearmodifiers", "--delay", "40",
                                str(value), env=env)
                        await asyncio.sleep(.5)

                    async def press_save(heading, tag: str):
                        """Click the primary Save fill; return its box."""
                        w, _h = window_size(window, env)
                        shot = destination / f"connected-tunnels-{tag}-save.png"
                        await asyncio.to_thread(screenshot, window, shot, env)
                        found = await asyncio.to_thread(tunnel_save_button, shot, heading, w)
                        if found is None:
                            raise RuntimeError(f"Tunnel Manager Save fill not found in {shot}")
                        (x, y), box = found
                        print(f"tunnel {tag}: Save fill {box} -> ({x},{y})", flush=True)
                        command("xdotool", "mousemove", "--window", window,
                                str(x), str(y), "click", "1", env=env)
                        await asyncio.sleep(.8)
                        return box

                    async def press_remove(heading, tag: str) -> None:
                        w, _h = window_size(window, env)
                        x, y = tunnel_remove_centre(heading, w)
                        print(f"tunnel {tag}: remove button -> ({x},{y})", flush=True)
                        command("xdotool", "mousemove", "--window", window,
                                str(x), str(y), "click", "1", env=env)
                        await asyncio.sleep(.8)

                    async def require_notice(word: str, save_box, tag: str) -> None:
                        """Positive signal the click landed: the footer notice.
                        Each expected word is unique to its outcome."""
                        w, _h = window_size(window, env)
                        await locate_rendered_label(
                            window, destination / f"connected-tunnels-{tag}-notice.png",
                            word, env=env, region=tunnel_notice_region(save_box, w),
                            timeout=12,
                        )

                    def disk_forwards() -> list[str]:
                        return saved_local_forwards(profiles, "CI tunnel fixture")

                    def spec(port: int) -> str:
                        return f"127.0.0.1:{port}:127.0.0.1:{echo_port}"

                    async def running_edit_delete_probe() -> None:
                        """Edit/delete of RUNNING Alpha is refused and its real
                        listener keeps echoing; idle Bravo stays editable:
                        edit + save + reopen + remove. Truth is the loopback
                        listener and the profile file the GUI persists."""
                        original = [spec(port_a), spec(port_b)]
                        if sorted(disk_forwards()) != sorted(original):
                            raise RuntimeError(f"unexpected saved forwards {disk_forwards()}")
                        await click_label(
                            window, destination / "connected-tunnels-edit-start-button.png",
                            "Tunnels", env, region=(max(0, tun_width - 220), 0, tun_width, 42),
                            timeout=15,
                        )
                        await toggle(label_a, "edit-start-a")
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_EDIT_0")
                        await forward_refused(port_b, "bravo idle before edit probe")
                        command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                        await asyncio.sleep(.6)
                        heading = await open_manager("edit")

                        # 1. Running Alpha: change its listen port -> refused.
                        new_a = free_loopback_port({echo_port, port_a, port_b})
                        await select_row(label_a, port_a, port_b, heading, "edit-alpha-selected")
                        await type_listen_port(heading, new_a)
                        # The draft really changed (typing reached the input).
                        await require_form(new_a, port_a, heading, "edit-alpha-draft")
                        save_box = await press_save(heading, "edit-alpha")
                        await require_notice("changing", save_box, "edit-alpha")
                        if sorted(disk_forwards()) != sorted(original):
                            raise RuntimeError(f"refused Alpha edit persisted: {disk_forwards()}")
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_EDIT_1")
                        await forward_refused(new_a, "rejected Alpha listen port")
                        await forward_refused(port_b, "bravo during Alpha edit refusal")

                        # 2. Running Alpha: delete -> refused. Reselecting
                        # reverts the draft and clears the notice, so the
                        # "deleting" word below is fresh evidence.
                        await select_row(label_a, port_a, new_a, heading, "delete-alpha-selected")
                        await press_remove(heading, "delete-alpha")
                        await require_notice("deleting", save_box, "delete-alpha")
                        if sorted(disk_forwards()) != sorted(original):
                            raise RuntimeError(f"refused Alpha delete persisted: {disk_forwards()}")
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_DELETE")
                        await forward_refused(port_b, "bravo during Alpha delete refusal")
                        await require_form(port_a, port_b, heading, "delete-alpha-kept")

                        # 3. Idle Bravo: edit + save allowed, Alpha keeps running.
                        new_b = free_loopback_port({echo_port, port_a, port_b, new_a})
                        await select_row(label_b, port_b, port_a, heading, "edit-bravo-selected")
                        await type_listen_port(heading, new_b)
                        await require_form(new_b, port_b, heading, "edit-bravo-draft")
                        save_box = await press_save(heading, "edit-bravo")
                        await require_notice("saved", save_box, "edit-bravo")
                        saved = disk_forwards()
                        if sorted(saved) != sorted([spec(port_a), spec(new_b)]):
                            raise RuntimeError(f"Bravo edit not persisted exactly: {saved}")
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_AFTER_BRAVO_EDIT")
                        await forward_refused(new_b, "edited Bravo must stay idle")
                        await forward_refused(port_b, "old Bravo port after edit")

                        # 4. Reopen: the manager shows the persisted edit.
                        command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                        await asyncio.sleep(.6)
                        heading = await open_manager("reopen")
                        await select_row(label_b, new_b, port_b, heading, "reopen-bravo-selected")
                        await require_form(new_b, port_a, heading, "reopen-bravo-not-alpha")

                        # 5. Remove idle Bravo; Alpha survives and still echoes.
                        w, _h = window_size(window, env)
                        found = await asyncio.to_thread(
                            tunnel_save_button,
                            destination / "connected-tunnels-reopen-bravo-not-alpha.png",
                            heading, w,
                        )
                        if found is None:
                            raise RuntimeError("Save fill not found for the remove notice row")
                        await press_remove(heading, "remove-bravo")
                        await require_notice("deleted", found[1], "remove-bravo")
                        if disk_forwards() != [spec(port_a)]:
                            raise RuntimeError(f"Bravo remove not persisted exactly: {disk_forwards()}")
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_AFTER_BRAVO_REMOVE")
                        await forward_refused(new_b, "removed Bravo")

                        # Leave nothing running: stop Alpha from the popup.
                        command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                        await asyncio.sleep(.6)
                        await click_label(
                            window, destination / "connected-tunnels-edit-stop-button.png",
                            "Tunnels", env, region=(max(0, tun_width - 220), 0, tun_width, 42),
                            timeout=15,
                        )
                        await toggle(label_a, "edit-stop-a")
                        await forward_refused(port_a, "alpha after edit probe stop")
                        command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                        await asyncio.sleep(.6)
                        print("PASS populated tunnels: running Alpha edit+delete refused "
                              "(listener kept echoing, file unchanged, Bravo idle); idle "
                              "Bravo edit/save/reopen/remove persisted while Alpha ran",
                              flush=True)

                    async def profile_edit_probe() -> None:
                        """Sessions pane -> right-click 'CI tunnel fixture' ->
                        Edit -> Edit SSH connection -> Save changes, while
                        Alpha runs. Renaming the owner or changing Alpha's
                        active spec is refused before any disk write (file
                        byte-identical, inline error, Alpha keeps echoing);
                        adding/editing an idle sibling (Bravo) is allowed;
                        finally the ORIGINAL popup switch stops Alpha.
                        Runs after running_edit_delete_probe, which leaves
                        exactly [Alpha] saved and nothing running."""
                        owner = "CI tunnel fixture"
                        if disk_forwards() != [spec(port_a)]:
                            raise RuntimeError(f"profile probe start state: {disk_forwards()}")
                        await click_label(
                            window, destination / "connected-profile-start-button.png",
                            "Tunnels", env, region=(max(0, tun_width - 220), 0, tun_width, 42),
                            timeout=15,
                        )
                        await toggle(label_a, "profile-start-a")
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_PROFILE_0")
                        command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                        await asyncio.sleep(.6)

                        async def session_row():
                            """Bounds of the owner row in the Sessions pane.
                            View > Session toggles: use it only on a miss."""
                            w, h = window_size(window, env)
                            for attempt in range(3):
                                try:
                                    return await locate_rendered_label(
                                        window, destination / f"connected-profile-row-{attempt}.png",
                                        owner, env=env, region=(0, 32, w, h - 22), timeout=6,
                                    )
                                except RuntimeError:
                                    if attempt == 2:
                                        raise
                                await click_label(
                                    window, destination / f"connected-profile-view-{attempt}.png",
                                    "View", env, region=(0, 0, 500, 32), timeout=10, settle=.4,
                                )
                                await click_label(
                                    window, destination / f"connected-profile-view-session-{attempt}.png",
                                    "Session", env, region=(0, 34, 520, 420), timeout=10, settle=.8,
                                )

                        async def open_edit(tag: str):
                            """Real Sessions-pane path; returns the dialog region."""
                            row = await session_row()
                            x, y = center(row)
                            command("xdotool", "mousemove", "--window", window,
                                    str(x), str(y), "click", "3", env=env)
                            await asyncio.sleep(.6)
                            w, h = window_size(window, env)
                            await click_label(
                                window, destination / f"connected-profile-{tag}-context.png",
                                "Edit", env, timeout=10, settle=.8,
                                region=(max(0, x - 260), max(32, y - 160),
                                        min(w, x + 260), min(h - 22, y + 160)),
                            )
                            dialog = connection_dialog_region(w, h)
                            await locate_rendered_label(
                                window, destination / f"connected-profile-{tag}-dialog.png",
                                CONNECTION_DIALOG_TITLE, env=env, timeout=12,
                                region=(dialog[0], dialog[1], dialog[2], dialog[1] + 80),
                            )
                            return dialog

                        def scroll_bottom(dialog) -> None:
                            command("xdotool", "mousemove", "--window", window,
                                    str((dialog[0] + dialog[2]) // 2), str((dialog[1] + dialog[3]) // 2),
                                    "click", "--repeat", "30", "--delay", "15", "5", env=env)

                        async def bottom_shot(dialog, tag: str) -> Path:
                            scroll_bottom(dialog)
                            await asyncio.sleep(.8)
                            shot = destination / f"connected-profile-{tag}.png"
                            await asyncio.to_thread(screenshot, window, shot, env)
                            return shot

                        async def title_visible(dialog, tag: str) -> bool:
                            shot = destination / f"connected-profile-{tag}-title.png"
                            await asyncio.to_thread(screenshot, window, shot, env)
                            try:
                                await asyncio.to_thread(
                                    locate_visible_label, shot, CONNECTION_DIALOG_TITLE,
                                    region=(dialog[0], dialog[1], dialog[2], dialog[1] + 80),
                                )
                            except RuntimeError:
                                return False
                            return True

                        async def submit(dialog, tag: str):
                            """Scroll to the buttons, click the primary
                            "Save changes" fill; return the pre-click
                            danger-pixel count (refusal control)."""
                            w, _h = window_size(window, env)
                            before = await bottom_shot(dialog, f"{tag}-before-submit")
                            save_only = await asyncio.to_thread(
                                locate_visible_label, before, "Save only", region=dialog,
                            )
                            found = await asyncio.to_thread(
                                connection_submit_button, before, save_only, w,
                            )
                            if found is None:
                                raise RuntimeError(f"Save changes fill not found in {before}")
                            (x, y), box = found
                            red = await asyncio.to_thread(danger_text_pixels, before, dialog)
                            print(f"profile {tag}: Save changes {box} -> ({x},{y}); "
                                  f"danger pixels before {red}", flush=True)
                            command("xdotool", "mousemove", "--window", window,
                                    str(x), str(y), "click", "1", env=env)
                            await asyncio.sleep(1.2)
                            return red

                        async def require_refused(dialog, tag, file_before, red_before):
                            after = await bottom_shot(dialog, f"{tag}-after-submit")
                            red_after = await asyncio.to_thread(danger_text_pixels, after, dialog)
                            print(f"profile {tag}: danger pixels after {red_after}", flush=True)
                            profile_edit_refused(
                                file_before=file_before, file_after=profiles.read_bytes(),
                                title_visible=await title_visible(dialog, f"{tag}-after"),
                                red_before=red_before, red_after=red_after, context=tag,
                            )

                        async def cancel(dialog, tag: str) -> None:
                            shot = await bottom_shot(dialog, f"{tag}-cancel")
                            save_only = await asyncio.to_thread(
                                locate_visible_label, shot, "Save only", region=dialog,
                            )
                            await click_label(
                                window, destination / f"connected-profile-{tag}-cancel-click.png",
                                "Cancel", env, timeout=10, settle=.8,
                                region=(dialog[0], save_only[1] - 12, save_only[0], save_only[3] + 12),
                            )
                            if await title_visible(dialog, f"{tag}-cancelled"):
                                raise RuntimeError(f"{tag}: Cancel left the connection dialog open")

                        async def set_local_forwards(dialog, value: str, tag: str) -> None:
                            w, _h = window_size(window, env)
                            shot = await bottom_shot(dialog, f"{tag}-forwards")
                            label = await asyncio.to_thread(
                                locate_visible_label, shot, "Local forward", region=dialog,
                            )
                            x, y = local_forward_input_centre(label, w)
                            print(f"profile {tag}: Local forward label {label} -> ({x},{y})", flush=True)
                            command("xdotool", "mousemove", "--window", window,
                                    str(x), str(y), "click", "1", env=env)
                            await asyncio.sleep(.3)
                            command("xdotool", "key", "--clearmodifiers", "ctrl+a", env=env)
                            command("xdotool", "type", "--clearmodifiers", "--delay", "20",
                                    value, env=env)
                            await asyncio.sleep(.5)

                        async def stop_alpha(tag: str) -> None:
                            await click_label(
                                window, destination / f"connected-{tag}-button.png",
                                "Tunnels", env, region=(max(0, tun_width - 220), 0, tun_width, 42),
                                timeout=15,
                            )
                            await toggle(label_a, tag)
                            await forward_refused(port_a, f"Alpha after popup stop ({tag})")
                            command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                            await asyncio.sleep(.6)

                        # 1. Rename the running owner -> refused before disk.
                        dialog = await open_edit("rename")
                        file_before = profiles.read_bytes()
                        # EditProfile focuses the Session name input.
                        command("xdotool", "key", "--clearmodifiers", "ctrl+a", env=env)
                        command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                                PROFILE_RENAME_PROBE, env=env)
                        await asyncio.sleep(.4)
                        red = await submit(dialog, "rename")
                        await require_refused(dialog, "rename", file_before, red)
                        names = saved_profile_names(profiles)
                        if owner not in names or PROFILE_RENAME_PROBE in names:
                            raise RuntimeError(f"rename refusal left profiles {names}")
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_PROFILE_RENAME")
                        await cancel(dialog, "rename")
                        print("PASS profile edit: rename of running owner refused before disk "
                              "write; Alpha kept echoing", flush=True)

                        # 2. Change Alpha's ACTIVE spec -> refused before disk.
                        new_a = free_loopback_port({echo_port, port_a, port_b})
                        dialog = await open_edit("spec")
                        file_before = profiles.read_bytes()
                        await set_local_forwards(dialog, spec(new_a), "spec")
                        red = await submit(dialog, "spec")
                        await require_refused(dialog, "spec", file_before, red)
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_PROFILE_SPEC")
                        await forward_refused(new_a, "refused Alpha spec change")
                        await cancel(dialog, "spec")
                        print("PASS profile edit: running Alpha spec change refused before "
                              "disk write; Alpha kept echoing", flush=True)

                        # 3. The ORIGINAL popup switch still owns Alpha after
                        # both refusals: it stops the real listener.
                        await stop_alpha("profile-refused-stop")
                        print("PASS profile edit: original popup switch stopped Alpha after "
                              "refused rename/spec edits", flush=True)
                        await click_label(
                            window, destination / "connected-profile-restart-button.png",
                            "Tunnels", env, region=(max(0, tun_width - 220), 0, tun_width, 42),
                            timeout=15,
                        )
                        await toggle(label_a, "profile-restart-a")
                        await forward_echo(port_a, b"GUI_FORWARD_ALPHA_PROFILE_RESTART")
                        command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                        await asyncio.sleep(.6)

                        # 4. Idle sibling: add Bravo, then edit it -> allowed.
                        new_b = free_loopback_port({echo_port, port_a, port_b, new_a})
                        new_b2 = free_loopback_port({echo_port, port_a, port_b, new_a, new_b})
                        for tag, sibling in (("bravo-add", new_b), ("bravo-edit", new_b2)):
                            dialog = await open_edit(tag)
                            await set_local_forwards(dialog, f"{spec(port_a)}; {spec(sibling)}", tag)
                            await submit(dialog, tag)
                            if await title_visible(dialog, f"{tag}-after"):
                                raise RuntimeError(f"{tag}: idle sibling Save changes was refused")
                            if disk_forwards() != [spec(port_a), spec(sibling)]:
                                raise RuntimeError(f"{tag}: not persisted exactly: {disk_forwards()}")
                            await forward_echo(port_a, f"GUI_FORWARD_ALPHA_{tag}".encode())
                            await forward_refused(sibling, f"{tag}: sibling must stay idle")
                        await forward_refused(new_b, "Bravo old port after sibling edit")
                        print("PASS profile edit: idle sibling Bravo add+edit persisted while "
                              "Alpha ran", flush=True)

                        # 5. Leave nothing running: the switch stops Alpha
                        # again after the accepted sibling edits.
                        await stop_alpha("profile-final-stop")
                        await forward_refused(new_b2, "edited Bravo after final stop")
                        print("PASS profile edit: popup switch stopped Alpha after idle "
                              "sibling edits", flush=True)

                    allow_before = events.path.read_text(encoding="utf-8").count("LOCAL_FORWARD_ALLOW")
                    await click_label(
                        window, destination / "connected-tunnels-button-2.png", "Tunnels", env,
                        region=(max(0, tun_width - 220), 0, tun_width, 42), timeout=15,
                    )
                    await toggle(label_a, "start-a")
                    await forward_echo(port_a, b"GUI_FORWARD_ALPHA_1")
                    if events.path.read_text(encoding="utf-8").count("LOCAL_FORWARD_ALLOW") <= allow_before:
                        raise RuntimeError("alpha echo did not traverse the fixture SSH forward")
                    await forward_refused(port_b, "bravo after starting only alpha")
                    await popup_row(label_a, destination / "connected-tunnels-popup-one-running.png")
                    await toggle(label_b, "start-b")
                    await forward_echo(port_b, b"GUI_FORWARD_BRAVO_1")
                    await toggle(label_a, "stop-a")
                    await forward_refused(port_a, "alpha after its own stop")
                    await forward_echo(port_b, b"GUI_FORWARD_BRAVO_2")
                    await toggle(label_b, "stop-b")
                    await forward_refused(port_b, "bravo after its own stop")
                    await forward_refused(port_a, "alpha still stopped")
                    await asyncio.to_thread(
                        screenshot, window, destination / "connected-tunnels-popup-stopped.png", env,
                    )
                    command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                    await asyncio.sleep(.6)
                    # The split layout has its vertical divider at width/2
                    # (x=640 at 1280 wide), so refocus inside the right
                    # terminal pane, well clear of the divider.
                    toggle_width, toggle_height = window_size(window, env)
                    term_x, term_y = (toggle_width * 3) // 4, 280
                    if not (0 <= term_x < toggle_width and 0 <= term_y < toggle_height
                            and abs(term_x - toggle_width // 2) >= 100):
                        raise RuntimeError(
                            f"terminal refocus click ({term_x}, {term_y}) invalid for "
                            f"window {toggle_width}x{toggle_height}"
                        )
                    command(
                        "xdotool", "windowfocus", window, "mousemove", "--window", window,
                        str(term_x), str(term_y), "click", "1", env=env,
                    )
                    await asyncio.sleep(.3)
                    command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                            "echo:AFTER_TUNNEL_TOGGLES", env=env)
                    command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                    try:
                        await wait_for(
                            lambda: "NATIVE_ECHO:AFTER_TUNNEL_TOGGLES" in events.path.read_text(encoding="utf-8").splitlines(),
                            "shell echo after per-forward tunnel toggles",
                        )
                    except RuntimeError:
                        await asyncio.to_thread(
                            screenshot, window,
                            destination / "connected-tunnels-toggles-echo-timeout.png", env,
                        )
                        raise
                    print("PASS populated tunnels: independent per-forward start/stop "
                          "(echo + refusal through fixture SSH)", flush=True)
                    await running_edit_delete_probe()
                    await profile_edit_probe()

                event_log = events.path.read_text(encoding="utf-8")
                if "SHELL_PTY_RESIZE" not in event_log:
                    raise RuntimeError("connected SSH fixture never observed a live PTY resize")
                # The auxiliary-shell probe closes one shell on purpose. The
                # baseline is that one close, frozen before the settle sleep,
                # not a later sample. Anything beyond it is an unexpected exit.
                ended_during_rest = sum(
                    line == "SHELL_HANDLER_ENDED" for line in event_log.splitlines()
                )
                eof_during_rest = sum(
                    line == "SHELL_STREAM_EOF" for line in event_log.splitlines()
                )
                if (
                    ended_during_rest > shells_ended_before_split
                    or eof_during_rest > shell_eof_before_split
                ):
                    raise RuntimeError(
                        "SSH terminal unexpectedly exited during split/dock/focus acceptance: "
                        f"SHELL_HANDLER_ENDED={ended_during_rest} "
                        f"(baseline {shells_ended_before_split}) "
                        f"SHELL_STREAM_EOF={eof_during_rest} "
                        f"(baseline {shell_eof_before_split}): "
                        + repr(event_log[-900:])
                    )
                if "UNEXPECTED_LOCKED_PTY_INPUT" in event_log:
                    raise RuntimeError("privacy curtain forwarded synthetic keyboard input to SSH")
                if "UNEXPECTED_FREE_TYPE_PTY_INPUT" in event_log:
                    raise RuntimeError("free-type editing leaked synthetic input to remote SSH PTY")
                if any(
                    marker in event_log
                    for marker in (
                        "UNEXPECTED_MENU_PTY_INPUT",
                        "UNEXPECTED_SENDER_PTY_INPUT",
                        "UNEXPECTED_GEAR_PTY_INPUT",
                    )
                ):
                    raise RuntimeError("Session menu, Sender or dock gear keyboard input leaked to SSH")
                if event_log.count("AUTH_ACCEPT:native-smoke") < 3:
                    raise RuntimeError(
                        "three sessions (main, auxiliary, split) were not authenticated"
                    )
                # Window-manager close. After the event-log snapshot so shells
                # ending on exit do not trip the "unexpectedly exited" checks.
                # Confirming Exit is the last click; the process must exit 0
                # by itself before teardown.
                if app.poll() is not None:
                    raise RuntimeError(
                        "application already exited before window-manager close "
                        f"(code {app.returncode})"
                    )
                await asyncio.to_thread(
                    screenshot,
                    window,
                    destination / "connected-before-wm-close.png",
                    env,
                )
                await asyncio.to_thread(request_window_close, window, env)
                # A locate call can spend its whole OCR budget on one frame,
                # so wait for the repaint the request causes before reading it.
                requested_shot = destination / "connected-close-requested.png"
                repaint_deadline = time.monotonic() + 10 * TIME_SCALE
                while True:
                    await asyncio.sleep(.4)
                    if app.poll() is not None:
                        raise RuntimeError(
                            "window-manager close exited without confirmation "
                            f"(code {app.returncode})"
                        )
                    await asyncio.to_thread(screenshot, window, requested_shot, env)
                    close_pixels = await asyncio.to_thread(
                        diff, destination / "connected-before-wm-close.png", requested_shot, env,
                    )
                    if close_pixels >= 5000:
                        break
                    if time.monotonic() >= repaint_deadline:
                        raise RuntimeError(
                            "window-manager close request changed nothing on screen: "
                            f"{close_pixels} pixels (need >= 5000)"
                        )
                print(f"window-manager close request repaint: {close_pixels} pixels", flush=True)
                try:
                    close_confirm = await locate_rendered_label(
                        window,
                        destination / "connected-close-confirm.png",
                        "Exit and disconnect",
                        env=env,
                        region=confirm_region,
                        timeout=15,
                    )
                except RuntimeError:
                    if app.poll() is not None:
                        raise RuntimeError(
                            "window-manager close exited without confirmation "
                            f"(code {app.returncode})"
                        ) from None
                    raise
                if app.poll() is not None:
                    raise RuntimeError(
                        "window-manager close exited without waiting for confirmation "
                        f"(code {app.returncode})"
                    )
                close_dialog = exit_dialog_region(close_confirm, width, height)
                cancel_bounds = await locate_rendered_label(
                    window,
                    destination / "connected-close-cancel.png",
                    "Cancel",
                    env=env,
                    region=close_dialog,
                    timeout=15,
                )
                cancel_top = cancel_bounds[1]
                cancel_right = cancel_bounds[2]
                cancel_bottom = cancel_bounds[3]
                exit_box = (
                    cancel_right + 8,
                    cancel_top - 8,
                    min(width, cancel_right + 200),
                    cancel_bottom + 8,
                )
                if exit_box[2] <= exit_box[0] or exit_box[3] <= exit_box[1]:
                    raise RuntimeError(
                        "confirming Exit button box collapsed from "
                        f"Cancel {cancel_bounds}: {exit_box}"
                    )
                # The confirming button is dark text on the danger fill, which
                # the light-text OCR cannot read. Find the fill itself on
                # Cancel's row, right of Cancel, and click its centre.
                exit_shot = destination / "connected-close-exit.png"
                await asyncio.to_thread(screenshot, window, exit_shot, env)
                exit_run = danger_fill_run(
                    exit_shot, (cancel_top + cancel_bottom) // 2, exit_box[0], exit_box[2],
                )
                if exit_run is None:
                    raise RuntimeError(
                        "confirming Exit button fill not found right of Cancel "
                        f"{cancel_bounds} in {exit_box}"
                    )
                exit_x = (exit_run[0] + exit_run[1]) // 2
                exit_y = (cancel_top + cancel_bottom) // 2
                print(f"confirming Exit button fill x={exit_run} click=({exit_x}, {exit_y})", flush=True)
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(exit_x), str(exit_y), "click", "1", env=env,
                )
                close_started = time.monotonic()
                close_deadline = close_started + 20 * TIME_SCALE
                while app.poll() is None and time.monotonic() < close_deadline:
                    await asyncio.sleep(.2)
                close_elapsed = time.monotonic() - close_started
                close_code = app.poll()
                print(
                    f"window-manager close: process "
                    f"{'exited' if close_code is not None else 'still running'} "
                    f"after {close_elapsed:.1f}s"
                    + (f" code {close_code}" if close_code is not None else ""),
                    flush=True,
                )
                if close_code is None:
                    raise RuntimeError(
                        "window-manager close: application still running "
                        f"after {close_elapsed:.1f}s"
                    )
                if close_code != 0:
                    raise RuntimeError(
                        f"window-manager close: application exit code {close_code}"
                    )
                find_echo_tokens = (
                    "FIND_P1_PANE", "FIND_P1_AFTER", "FIND_P2_PANE",
                    "FIND_P2_AFTER", "FIND_P3_PANE", "FIND_P3_AFTER",
                )
                logged_lines = set(events.path.read_text(encoding="utf-8").splitlines())
                missing_find_echoes = [
                    token for token in find_echo_tokens
                    if f"NATIVE_ECHO:{token}" not in logged_lines
                ]
                if missing_find_echoes:
                    raise RuntimeError(
                        f"Find ownership probes missing SSH echo events: {missing_find_echoes}"
                    )
                (destination / "connected-acceptance.txt").write_text(
                    "PASS: two authenticated independent OpenSSH PTYs through production Iced\n"
                    "PASS: native Files dock established actual isolated SSH SFTP subsystem\n"
                    "PASS: PTY resize events preserved both SSH shell sessions through the test\n"
                    "PASS: click-driven SFTP upload preserved exact binary bytes in the native-smoke chroot\n"
                    "PASS: click-driven SFTP download preserved exact binary bytes in the visible local directory\n"
                    "PASS: Filer Name header click reversed remote name order and the second click restored it without a new SFTP listing\n"
                    "PASS: Filer no-pause right-click anchored at the new pointer with the previous pointer parked away rendered Refresh and New Folder, consumed typing, Return and Escape restored the row, and the shell answered AFTER_CONTEXT_MENU\n"
                    "PASS: exact payloads appeared at the expected disposable remote and local paths\n"
                    f"Connected split screenshot change: {split_pixels} pixels\n"
                    f"Opened utility dock screenshot change: {files_pixels} pixels\n"
                    f"Closed SFTP utility dock screenshot change: {closed_pixels} pixels\n"
                    f"Free-type mode screenshot change: {mode_pixels} pixels\n"
                    f"Free-type draft edit screenshot change: {free_type_pixels} pixels\n"
                    f"Focus-mode screenshot change: {focus_pixels} pixels\n"
                    f"Privacy-curtain screenshot change: {privacy_pixels} pixels\n"
                    f"Alternate-screen screenshot change: {fullscreen_pixels} pixels\n"
                    f"Remote-key mode screenshot change: {remote_keys_pixels} pixels\n"
                    "PASS: real remote-key Left arrow reached the isolated SFTP-capable SSH shell\n"
                    "PASS: a single click on the saved session selected it (row highlight measured) without SESSION_STARTED; the double click still opened it\n"
                    "PASS: terminal owned the keyboard before chrome menu probes\n"
                    "PASS: Session menu, Sender and dock gear consumed typing, Return and Escape without SSH input\n"
                    "PASS: shell answered AFTER_MENU, AFTER_SENDER_ESCAPE and AFTER_GEAR without terminal re-click\n"
                    "PASS: Find ownership probes: click-back then Escape closed Find; Next then typing stayed in Find; Command Palette opened by mouse and dismissed released the keyboard to the terminal and a click re-owned Find; shell answered FIND_P1_AFTER, FIND_P2_AFTER and FIND_P3_AFTER without a terminal click\n"
                    "PASS: Ctrl+Shift+J under the Session and dock gear menus left the auxiliary SSH shell connected; it closed it after Escape\n"
                    "PASS: privacy-locked and free-type synthetic text never reached SSH\n"
                    "PASS: Session Exit asked for confirmation and Cancel left the shell answering AFTER_EXIT_CANCEL\n"
                    "PASS: Exit Cancel removed the confirmation before the workspace was clicked\n"
                    "PASS: top-right Tunnels popup showed Tunnel Settings..., which opened Tunnel Manager; Escape closed it and the shell answered AFTER_TUNNEL_SETTINGS without a terminal click\n"
                    + ("PASS: populated Tunnel Manager started/stopped each forward independently and the shell answered AFTER_TUNNEL_TOGGLES after a terminal click\n"
                       if POPULATED else "") +
                    "PASS: window-manager close request asked for confirmation; confirming Exit ended the process with exit code 0\n"
                    f"Exit Cancel dialog removal: {removed_pixels} pixels\n"
                    f"Window-manager close exit: {close_elapsed:.1f}s code {close_code}\n"
                    "All keys generated in isolated temporary fixture; no real host or credential.\n",
                    encoding="utf-8",
                )
                print("PASS production Iced GUI: connected split, click-driven SFTP upload/download, draft, focus/privacy, alternate-screen and PTY isolation", flush=True)
        finally:
            if app is not None and app.poll() is None:
                app.terminate()
                try:
                    await asyncio.wait_for(asyncio.to_thread(app.wait), 8)
                except asyncio.TimeoutError:
                    app.kill()
                    await asyncio.to_thread(app.wait)
            xvfb.terminate()
            try:
                await asyncio.wait_for(asyncio.to_thread(xvfb.wait), 5)
            except asyncio.TimeoutError:
                xvfb.kill()
                await asyncio.to_thread(xvfb.wait)
            server.close()
            # Python >= 3.12.1 asyncio servers wait for every open connection;
            # a tunnel ssh child that outlives the app would hang this forever
            # and swallow the real failure until the job timeout.
            try:
                await asyncio.wait_for(server.wait_closed(), 10)
            except asyncio.TimeoutError:
                print("fixture SSH server still had open connections at exit", flush=True)
            # Always retain only synthetic lifecycle markers. This is critical
            # when a PTY appears then unexpectedly exits before GUI assertions.
            (destination / "ssh-fixture-events.txt").write_text(
                events.path.read_text(encoding="utf-8"), encoding="utf-8"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
