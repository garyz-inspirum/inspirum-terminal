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


class MonitoredSession(fixture.NativeSession):
    """Never log remote input: report only a fixed synthetic test marker."""

    def handle_line(self, line: str) -> None:
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
            if probe_state["phase"] is not None:
                events.write(f"UNEXPECTED_{probe_state['phase']}_PTY_INPUT")
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
                    if line in (
                        "echo:TOUR_PROBE",
                        "echo:TERMINAL_OWNED",
                        "echo:AFTER_MENU",
                        "echo:AFTER_SENDER_ESCAPE",
                        "echo:AFTER_GEAR",
                        "echo:AFTER_EXIT_CANCEL",
                    ):
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
    region=None, timeout: float = 22, settle: float = .5,
):
    """Screenshot until ``label`` is rendered (optionally inside ``region``),
    click its centre with a native pointer event and return its bounds.

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
    command(
        "xdotool", "mousemove", "--window", window,
        *map(str, center(bounds)), "click", "1", env=env,
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


async def run_tour(window: str, tour_dir: Path, env: dict[str, str], events, app_log: Path) -> int:
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

    def click(x: int, y: int) -> None:
        command("xdotool", "mousemove", "--window", window, str(x), str(y), "click", "1", env=env)

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

    async def s04():
        key("ctrl+n")
        await asyncio.sleep(1.5)
        shot("tour-04-new-connection.png")
        key("Escape")
        await asyncio.sleep(.5)
    await step("new-connection", s04)

    # Steps 4-5 are mandatory: a real SSH session must start.
    if "SESSION_STARTED" not in log_text():
        for y in (630, 653, 612, 676, 699):
            click(200, y)
            await asyncio.sleep(1.5)
            if "SESSION_STARTED" in log_text():
                break
            key("Escape")
    if "SESSION_STARTED" not in log_text():
        print(
            "tour: FAILED clicking the saved profile never started a real SSH session. "
            "SSH events: " + repr(log_text()[-800:])
            + "; application log tail: " + app_log.read_text(encoding="utf-8")[-1800:],
            flush=True,
        )
        return 1
    await asyncio.sleep(2)
    shot("tour-05-terminal.png")

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

    async def s08():
        key("ctrl+shift+f")
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
        server = await asyncssh.create_server(
            lambda: MonitoredServer(trusted, events),
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
            json.dumps([{
                "name": "CI connected fixture",
                "host": "native-smoke",
                "user": "native-smoke",
                "port": None,
                "strict": True,
            }]), encoding="utf-8",
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
                # profile). Locate the profile name by OCR and single-click
                # it; fall back to bounded points in the session list rows,
                # never destructive dialogs. Authentication, rather than a
                # screenshot delta alone, proves the click worked.
                try:
                    await click_label(
                        window, destination / "connected-session-list.png",
                        "CI connected fixture", env, timeout=8, settle=1.5,
                    )
                except RuntimeError:
                    pass
                if "SESSION_STARTED" not in events.path.read_text(encoding="utf-8"):
                    for y in (630, 653, 612, 676, 699):
                        command("xdotool", "mousemove", "--window", window, "200", str(y), "click", "1", env=env)
                        await asyncio.sleep(1.5)
                        if "SESSION_STARTED" in events.path.read_text(encoding="utf-8"):
                            break
                        # A coordinate may land on a nonterminal affordance;
                        # dismiss any dialog before trying the next row.
                        command("xdotool", "key", "--clearmodifiers", "Escape", env=env)
                if "SESSION_STARTED" not in events.path.read_text(encoding="utf-8"):
                    raise RuntimeError(
                        "clicking the saved profile never started a real SSH session. "
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

                command("xdotool", "windowfocus", window, "key", "--clearmodifiers", "ctrl+shift+f", env=env)
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

                # The file browser has completed the real SFTP handshake.
                # End that UI task before testing the independent local-editor
                # workflow; otherwise its remote listing can keep keyboard
                # ownership while focus changes. Closing the dock does not
                # close either underlying authenticated SSH PTY.
                command(
                    "xdotool", "windowfocus", window, "key",
                    "--clearmodifiers", "ctrl+shift+f", env=env,
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
                command("xdotool", "type", "--clearmodifiers", "--delay", "25",
                        "screen-on", env=env)
                command("xdotool", "key", "--clearmodifiers", "Return", env=env)
                await wait_for(
                    lambda: "SCREEN_ON" in events.path.read_text(encoding="utf-8"),
                    "live SSH alternate-screen enter",
                )
                await asyncio.sleep(.5)
                screenshot(window, destination / "connected-fullscreen-active.png", env)
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
                await click_label(
                    window,
                    destination / "connected-close-exit.png",
                    "Exit",
                    env,
                    region=exit_box,
                    settle=.3,
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
                (destination / "connected-acceptance.txt").write_text(
                    "PASS: two authenticated independent OpenSSH PTYs through production Iced\n"
                    "PASS: native Files dock established actual isolated SSH SFTP subsystem\n"
                    "PASS: PTY resize events preserved both SSH shell sessions through the test\n"
                    "PASS: click-driven SFTP upload preserved exact binary bytes in the native-smoke chroot\n"
                    "PASS: click-driven SFTP download preserved exact binary bytes in the visible local directory\n"
                    "PASS: Filer Name header click reversed remote name order and the second click restored it without a new SFTP listing\n"
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
                    "PASS: terminal owned the keyboard before chrome menu probes\n"
                    "PASS: Session menu, Sender and dock gear consumed typing, Return and Escape without SSH input\n"
                    "PASS: shell answered AFTER_MENU, AFTER_SENDER_ESCAPE and AFTER_GEAR without terminal re-click\n"
                    "PASS: Ctrl+Shift+J under the Session and dock gear menus left the auxiliary SSH shell connected; it closed it after Escape\n"
                    "PASS: privacy-locked and free-type synthetic text never reached SSH\n"
                    "PASS: Session Exit asked for confirmation and Cancel left the shell answering AFTER_EXIT_CANCEL\n"
                    "PASS: Exit Cancel removed the confirmation before the workspace was clicked\n"
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
            await server.wait_closed()
            # Always retain only synthetic lifecycle markers. This is critical
            # when a PTY appears then unexpectedly exits before GUI assertions.
            (destination / "ssh-fixture-events.txt").write_text(
                events.path.read_text(encoding="utf-8"), encoding="utf-8"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
