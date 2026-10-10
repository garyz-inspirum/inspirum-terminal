#!/usr/bin/env python3
"""Native Iced GUI smoke against an isolated loopback SSH server (Linux only).

Uses the existing AsyncSSH fixture's synthetic identity and real OpenSSH PTYs.
Launches the PRODUCTION Iced binary inside a private Xvfb display, clicks a
saved session, keyboard-splits a second connected pane, and opens the file dock.
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
import subprocess
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


def command(*args: str, env: dict[str, str], timeout: float = 12) -> str:
    result = subprocess.run(
        args, env=env, capture_output=True, text=True,
        check=True, timeout=timeout,
    )
    return result.stdout.strip()


def screenshot(window: str, path: Path, env: dict[str, str]) -> None:
    command("import", "-window", window, str(path), env=env, timeout=15)
    if not path.is_file() or path.stat().st_size < 1000:
        raise RuntimeError(f"empty GUI screenshot: {path.name}")


def diff(first: Path, second: Path, env: dict[str, str]) -> int:
    result = subprocess.run(
        ["compare", "-metric", "AE", str(first), str(second), "null:"],
        env=env, capture_output=True, text=True, timeout=15,
    )
    if result.returncode not in (0, 1):
        raise RuntimeError(f"ImageMagick screenshot comparison failed: {result.stderr[:200]}")
    return int(result.stderr.strip())


async def wait_for(predicate, label: str, *, timeout: float = 22) -> None:
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise RuntimeError(f"timeout waiting for {label}")
        await asyncio.sleep(.2)


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
        server = await asyncssh.create_server(
            lambda: fixture.NativeServer(trusted, events),
            "127.0.0.1", 0, server_host_keys=[str(host)], encoding="utf-8",
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
        env = dict(os.environ, DISPLAY=reserved_display(), XDG_RUNTIME_DIR=str(runtime))
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
                    stdout=output, stderr=subprocess.STDOUT, env=env,
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
                await asyncio.sleep(1)
                screenshot(window, destination / "connected-before-open.png", env)

                # First item appears after the session search and 'Ungrouped'
                # label. Try bounded points in the *saved session list*, never
                # the destructive dialogs or global desktop. Authentication,
                # rather than a screenshot delta alone, proves the click worked.
                for y in (270, 305, 340, 375):
                    command("xdotool", "mousemove", "--window", window, "110", str(y), "click", "1", env=env)
                    await asyncio.sleep(1.8)
                    if "SESSION_STARTED" in events.path.read_text(encoding="utf-8"):
                        break
                if "SESSION_STARTED" not in events.path.read_text(encoding="utf-8"):
                    raise RuntimeError("clicking the saved profile never started a real SSH session")
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
                await asyncio.sleep(2)
                screenshot(window, destination / "connected-files-dock.png", env)
                files_pixels = diff(
                    destination / "connected-split.png",
                    destination / "connected-files-dock.png", env,
                )
                if files_pixels < 3000:
                    raise RuntimeError(f"files dock changed only {files_pixels} pixels")

                event_log = events.path.read_text(encoding="utf-8")
                if event_log.count("AUTH_ACCEPT:native-smoke") < 2:
                    raise RuntimeError("two distinct SSH panes were not authenticated")
                (destination / "connected-acceptance.txt").write_text(
                    "PASS: two authenticated independent OpenSSH PTYs through production Iced\n"
                    f"Connected split screenshot change: {split_pixels} pixels\n"
                    f"Opened utility dock screenshot change: {files_pixels} pixels\n"
                    "All keys generated in isolated temporary fixture; no real host or credential.\n",
                    encoding="utf-8",
                )
                print("PASS production Iced GUI: connected terminal, split, utility file dock", flush=True)
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
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
