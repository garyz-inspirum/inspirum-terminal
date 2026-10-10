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
    ``000_GUI_DOWNLOAD`` is not a click target. Tokens on the next line
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


def _upscaled_crop(rows, channels, box, scale: int, dest: Path) -> None:
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


def locate_visible_label(
    path: Path,
    label: str,
    *,
    region=None,
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
    bounds = match_visible_label(ocr_rows(path, tesseract, region), label)
    if bounds is None:
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
    if bounds is None:
        bounds = match_visible_label(
            visible_pixel_rows(path, tesseract, clusters=True), label,
        )
    if bounds is None:
        raise RuntimeError(f"visible label not found in screenshot: {label}")
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


async def wait_for(predicate, label: str, *, timeout: float = 22) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise RuntimeError(f"timeout waiting for {label}")
        await asyncio.sleep(.2)


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
        download_name = "000_GUI_DOWNLOAD_FIXTURE.bin"
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

                # First item appears after the session search and 'Ungrouped'
                # label. Try bounded points in the *saved session list*, never
                # the destructive dialogs or global desktop. Authentication,
                # rather than a screenshot delta alone, proves the click worked.
                for y in (235, 255, 275, 295, 315, 335, 355, 375):
                    command("xdotool", "mousemove", "--window", window, "110", str(y), "click", "1", env=env)
                    await asyncio.sleep(1.5)
                    if "SESSION_STARTED" in events.path.read_text(encoding="utf-8"):
                        break
                    # A CI screenshot coordinate may land on a nonterminal
                    # affordance; dismiss its dialog before trying next row.
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

                bottom = destination / "connected-files-bottom-dock.png"
                local_row = locate_visible_label(bottom, upload_name)
                remote_row = locate_visible_label(bottom, download_name)
                upload_action = locate_visible_label(
                    bottom, "Upload", region=(280, 520, 470, 585),
                )
                download_action = locate_visible_label(
                    bottom, "Download", region=(380, 520, 700, 585),
                )
                if local_row[3] <= local_row[1] or remote_row[3] <= remote_row[1]:
                    raise RuntimeError("bottom-dock file rows are not visibly targetable")

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

                # The remote fixture name sorts first, so the first REMOTE row
                # is deterministic even after upload refresh. Activate Download
                # through the production button and verify the exact binary
                # payload landed in the visible local browser directory.
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
                third_name = "111_GUI_THIRD_FIXTURE.bin"
                third_bytes = b"GUI_THIRD_FIXTURE" + bytes(range(32))
                (remote_root / third_name).write_bytes(third_bytes)
                refresh_bounds = locate_visible_label(destination / "connected-files-downloaded.png", "Refresh")
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(refresh_bounds)), "click", "1", env=env,
                )
                await wait_for(
                    lambda: events.path.read_text(encoding="utf-8").count("SFTP_LIST_READY") >= 2,
                    "remote listing refresh after creating distinct fixture",
                )
                screenshot(window, destination / "connected-files-third-visible.png", env)
                third_row = locate_visible_label(
                    destination / "connected-files-third-visible.png", third_name,
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

                # The populated queue is a bounded viewport. Scroll its visible
                # interior and retain the resulting production screenshot.
                queue_bounds = locate_visible_label(
                    destination / "connected-files-third-selected.png", "TRANSFER QUEUE",
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    str(queue_bounds[0] + 40), str(queue_bounds[3] + 24),
                    "click", "4", env=env,
                )
                await asyncio.sleep(.4)
                screenshot(window, destination / "connected-files-queue-scrolled.png", env)

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
                minimum_row = locate_visible_label(
                    destination / "connected-files-min-window.png", third_name,
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

                # Exercise the actual bottom -> right -> bottom controls and
                # prove a listing remains targetable after returning.
                dock_right = locate_visible_label(
                    destination / "connected-files-restored.png", "Dock right",
                    region=(1050, 380, 1270, 450),
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(dock_right)), "click", "1", env=env,
                )
                await asyncio.sleep(.7)
                screenshot(window, destination / "connected-files-side-dock.png", env)
                if diff(
                    destination / "connected-files-restored.png",
                    destination / "connected-files-side-dock.png", env,
                ) < 3000:
                    raise RuntimeError("Dock right did not visibly re-dock Files")
                dock_bottom = locate_visible_label(
                    destination / "connected-files-side-dock.png", "Dock bottom",
                )
                command(
                    "xdotool", "mousemove", "--window", window,
                    *map(str, center(dock_bottom)), "click", "1", env=env,
                )
                await asyncio.sleep(.7)
                screenshot(window, destination / "connected-files-redocked-bottom.png", env)
                redocked_row = locate_visible_label(
                    destination / "connected-files-redocked-bottom.png", third_name,
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
