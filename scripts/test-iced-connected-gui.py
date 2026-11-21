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



async def monitored_shell(
    stdin: asyncssh.SSHReader[str],
    stdout: asyncssh.SSHWriter[str],
    stderr: asyncssh.SSHWriter[str],
    events: fixture.EventLog,
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
                    stdout.write(f"NATIVE_ECHO:{line[5:]}\r\n")
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
            capture_output=True, text=True, timeout=15,
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


def ocr_rows(path: Path, tesseract: str, region=None):
    source = path
    offset = (0, 0)
    scale = 1
    temporary = None
    if region is not None:
        temporary = path.with_suffix(".crop.png")
        x1, y1, x2, y2 = region
        result = subprocess.run(
            ["convert", str(path), "-crop", f"{x2 - x1}x{y2 - y1}+{x1}+{y1}",
             "+repage", "-resize", "300%", "-colorspace", "Gray",
             "-contrast-stretch", "5%x5%", str(temporary)],
            capture_output=True, text=True, timeout=15,
        )
        if result.returncode != 0:
            raise RuntimeError(f"visible label crop failed: {result.stderr[:200]}")
        source = temporary
        offset = (x1, y1)
        scale = 3
    try:
        tsv = _ocr_tsv(tesseract, source, "6" if region is not None else "11")
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return _scaled_words(tsv, offset, scale)


# One locate attempt must fit well inside the caller's retry window
# (click_label / locate_rendered_label retry with fresh screenshots).
LOCATE_BUDGET_S = 12.0
# Session toolbar + tab bar; the inverted stage never scans above it unless
# the caller passes an explicit region.
TOOLBAR_BOTTOM_Y = 90


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
                if bounds is not None:
                    return bounds, segments
    finally:
        temporary.unlink(missing_ok=True)
    return None, segments


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
    bounds = match_visible_label(ocr_rows(path, tesseract, region), label)
    if bounds is None and time.monotonic() < deadline:
        pixel_rows = visible_pixel_rows(path, tesseract, clusters=False)
        if region is not None:
            x1, y1, x2, y2 = region
            inside = [
                row for row in pixel_rows
                if not (
                    row[1] + row[3] < x1 or row[1] > x2
                    or row[2] + row[4] < y1 or row[2] > y2
                )
            ]
            bounds = match_visible_label(inside, label)
        if bounds is None:
            bounds = match_visible_label(pixel_rows, label)
    if bounds is None and time.monotonic() < deadline:
        bounds = match_visible_label(
            visible_pixel_rows(path, tesseract, clusters=True), label,
        )
    if bounds is None and time.monotonic() < deadline:
        segments = visible_segment_rows(path, tesseract)
        flat = [word for words in segments for word in words]
        bounds = match_visible_label(flat, label)
        if bounds is None:
            bounds = match_visible_segment(segments, label)
    if bounds is None and time.monotonic() < deadline:
        bounds, inverted = _inverted_segment_search(
            path, tesseract, label, region, deadline,
        )
    else:
        inverted = []
    if bounds is None:
        seen = [row[0] for row in ocr_rows(path, tesseract, region)][:40]
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


def center(bounds):
    return ((bounds[0] + bounds[2]) // 2, (bounds[1] + bounds[3]) // 2)


def diff(first: Path, second: Path, env: dict[str, str]) -> int:
    result = subprocess.run(
        ["compare", "-metric", "AE", str(first), str(second), "null:"],
        env=env, capture_output=True, text=True, timeout=15,
    )
    if result.returncode not in (0, 1):
        raise RuntimeError(f"ImageMagick screenshot comparison failed: {result.stderr[:200]}")
    return int(result.stderr.strip().split()[0])


async def locate_rendered_label(
    window: str, path: Path, label: str, *, env: dict[str, str], timeout: float = 22,
    region=None,
):
    """Screenshot repeatedly until the label is rendered; return its bounds."""
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


def dock_body_region(width: int, height: int, *, right: bool = False):
    """Dock body (file rows, Upload/Download, Transfer queue rows).

    Bottom dock: full width, lower third above the status bar. Right dock:
    right 45% of the window below the 32px menu bar.
    """
    if right:
        return (int(width * .55), 32, width, height - 22)
    return (0, int(height * .66), width, height - 22)


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
                stdin, stdout, stderr, events
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
            INSPIRUM_THEME="dark",
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
                # The native graphics stack may still be initializing after
                # the X11 window first appears, especially on shared runners.
                await asyncio.sleep(2)
                screenshot(window, destination / "connected-before-open.png", env)

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

                # These shortcuts are intentionally exercised through native
                # Iced keyboard events, never a direct Rust state transition.
                command("xdotool", "windowfocus", window, "key", "--clearmodifiers", "ctrl+shift+r", env=env)
                await wait_for(
                    lambda: events.path.read_text(encoding="utf-8").count("SESSION_STARTED") >= 2,
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
                side_row = await locate_rendered_label(
                    window, destination / "connected-files-side-dock.png", third_name, env=env,
                    region=dock_body_region(width, height, right=True), timeout=40,
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(side_row)), "click", "1", env=env,
                )
                await asyncio.sleep(.4)
                screenshot(window, destination / "connected-files-side-selected.png", env)
                # The right dock's strip sits just under the 32px menu bar.
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(width - 40), "46", "click", "1", env=env,
                )
                await asyncio.sleep(.5)
                await click_label(
                    window, destination / "connected-files-gear-menu-right.png",
                    "Move to Bottom", env, settle=.9,
                    region=gear_menu_region(width, height),
                )
                screenshot(window, destination / "connected-files-redocked-bottom.png", env)
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
                await asyncio.sleep(.5)
                screenshot(window, destination / "connected-fullscreen-restored.png", env)
                fullscreen_pixels = diff(
                    destination / "connected-fullscreen-active.png",
                    destination / "connected-fullscreen-restored.png", env,
                )
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

                event_log = events.path.read_text(encoding="utf-8")
                if "SHELL_PTY_RESIZE" not in event_log:
                    raise RuntimeError("connected SSH fixture never observed a live PTY resize")
                if "SHELL_HANDLER_ENDED" in event_log or "SHELL_STREAM_EOF" in event_log:
                    raise RuntimeError(
                        "SSH terminal unexpectedly exited during split/dock/focus acceptance: "
                        + repr(event_log[-900:])
                    )
                if "UNEXPECTED_LOCKED_PTY_INPUT" in event_log:
                    raise RuntimeError("privacy curtain forwarded synthetic keyboard input to SSH")
                if "UNEXPECTED_FREE_TYPE_PTY_INPUT" in event_log:
                    raise RuntimeError("free-type editing leaked synthetic input to remote SSH PTY")
                if event_log.count("AUTH_ACCEPT:native-smoke") < 2:
                    raise RuntimeError("two distinct SSH panes were not authenticated")
                (destination / "connected-acceptance.txt").write_text(
                    "PASS: two authenticated independent OpenSSH PTYs through production Iced\n"
                    "PASS: native Files dock established actual isolated SSH SFTP subsystem\n"
                    "PASS: PTY resize events preserved both SSH shell sessions through the test\n"
                    "PASS: click-driven SFTP upload preserved exact binary bytes in the native-smoke chroot\n"
                    "PASS: click-driven SFTP download preserved exact binary bytes in the visible local directory\n"
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
                    "PASS: privacy-locked and free-type synthetic text never reached SSH\n"
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
