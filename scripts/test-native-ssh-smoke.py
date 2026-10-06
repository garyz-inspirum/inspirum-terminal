#!/usr/bin/env python3
"""Run a disposable native OpenSSH server and the application-path SSH smoke test."""

from __future__ import annotations

import getpass
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time


def command_path(name: str) -> str:
    found = shutil.which(name)
    if found:
        return found
    if os.name == "nt":
        candidate = Path(os.environ.get("WINDIR", r"C:\Windows")) / "System32" / "OpenSSH" / f"{name}.exe"
        if candidate.is_file():
            return str(candidate)
    if name == "sshd" and Path("/usr/sbin/sshd").is_file():
        return "/usr/sbin/sshd"
    raise SystemExit(f"ERROR: required OpenSSH executable not found: {name}")


def q(path: Path) -> str:
    value = path.resolve().as_posix()
    return f'"{value}"'


def reserve_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def wait_ready(process: subprocess.Popen[bytes], port: int) -> None:
    deadline = time.monotonic() + 10
    while True:
        if process.poll() is not None:
            raise RuntimeError("disposable sshd exited during startup")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            if time.monotonic() >= deadline:
                raise
            time.sleep(0.05)


def write_remote_program(root: Path) -> str:
    marker = root / "cleanup.marker"
    if os.name == "nt":
        script = root / "remote.ps1"
        marker_text = marker.resolve().as_posix().replace("'", "''")
        script.write_text(
            f"""$ErrorActionPreference = 'Stop'
$marker = '{marker_text}'
try {{
    [Console]::WriteLine('FIXTURE_AUTHENTICATED')
    while ($true) {{
        $line = [Console]::ReadLine()
        if ($null -eq $line) {{ break }}
        if ($line.StartsWith('echo:')) {{
            [Console]::WriteLine('REMOTE_ECHO:' + $line.Substring(5))
        }} elseif ($line -eq 'size') {{
            [Console]::WriteLine(('REMOTE_SIZE:{{0}} {{1}}' -f [Console]::WindowHeight, [Console]::WindowWidth))
        }} elseif ($line -eq 'exit') {{
            break
        }}
    }}
}} finally {{
    Set-Content -LiteralPath $marker -Value 'CLEANUP'
}}
""",
            encoding="utf-8",
        )
        return f"powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File {q(script)}"

    script = root / "remote.sh"
    marker_text = str(marker.resolve()).replace("'", "'\''")
    script.write_text(
        f"""#!/bin/sh
marker='{marker_text}'
trap 'printf CLEANUP > "$marker"' EXIT HUP TERM
stty -echo
printf 'FIXTURE_AUTHENTICATED\n'
while IFS= read -r line; do
  case "$line" in
    echo:*) printf 'REMOTE_ECHO:%s\n' "${{line#echo:}}" ;;
    size) printf 'REMOTE_SIZE:'; stty size ;;
    exit) exit 0 ;;
  esac
done
""",
        encoding="utf-8",
    )
    script.chmod(0o755)
    return f"/bin/sh {q(script)}"


def main() -> int:
    if not (sys.platform.startswith("win") or sys.platform == "darwin"):
        raise SystemExit("ERROR: native SSH smoke fixture is intended for Windows and macOS CI")

    sshd = command_path("sshd")
    ssh_keygen = command_path("ssh-keygen")
    root_parent = Path(os.environ.get("INSPIRUM_TEST_TMPDIR", tempfile.gettempdir())).resolve()
    root_parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="native-ssh-fixture-", dir=root_parent) as tmp:
        root = Path(tmp)
        port = reserve_port()
        user = getpass.getuser()

        for name in ("host", "client", "wrong-host"):
            subprocess.run(
                [ssh_keygen, "-q", "-t", "ed25519", "-N", "", "-f", str(root / name)],
                check=True,
            )

        (root / "authorized_keys").write_text(
            (root / "client.pub").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        force_command = write_remote_program(root)

        common = [
            "ListenAddress 127.0.0.1",
            f"Port {port}",
            f"HostKey {q(root / 'host')}",
            f"PidFile {q(root / 'sshd.pid')}",
            f"AuthorizedKeysFile {q(root / 'authorized_keys')}",
            "StrictModes no",
            "PasswordAuthentication no",
            "KbdInteractiveAuthentication no",
            "PubkeyAuthentication yes",
            "AuthenticationMethods publickey",
            f"AllowUsers {user}",
            "AllowTcpForwarding no",
            "AllowAgentForwarding no",
            "X11Forwarding no",
            "PermitTunnel no",
            "PermitTTY yes",
            "PrintMotd no",
            f"ForceCommand {force_command}",
            "LogLevel VERBOSE",
        ]
        if os.name != "nt":
            common.insert(6, "UsePAM no")
            common.insert(-2, "PrintLastLog no")
        (root / "sshd_config").write_text("\n".join(common) + "\n", encoding="utf-8")

        host_fields = (root / "host.pub").read_text(encoding="utf-8").split()
        wrong_fields = (root / "wrong-host.pub").read_text(encoding="utf-8").split()
        (root / "known_hosts").write_text(
            f"[127.0.0.1]:{port} {host_fields[0]} {host_fields[1]}\n",
            encoding="utf-8",
        )
        (root / "changed_known_hosts").write_text(
            f"[127.0.0.1]:{port} {wrong_fields[0]} {wrong_fields[1]}\n",
            encoding="utf-8",
        )

        def client_config(known_hosts: Path) -> str:
            return "\n".join(
                [
                    "Host native-fixture",
                    " HostName 127.0.0.1",
                    f" Port {port}",
                    f" User {user}",
                    f" IdentityFile {q(root / 'client')}",
                    " IdentitiesOnly yes",
                    " IdentityAgent none",
                    f" UserKnownHostsFile {q(known_hosts)}",
                    " BatchMode yes",
                    " ConnectTimeout 5",
                    " UpdateHostKeys no",
                ]
            ) + "\n"

        (root / "config").write_text(client_config(root / "known_hosts"), encoding="utf-8")
        (root / "changed-config").write_text(
            client_config(root / "changed_known_hosts"),
            encoding="utf-8",
        )

        subprocess.run([sshd, "-t", "-f", str(root / "sshd_config")], check=True)
        log_path = root / "sshd.log"
        with log_path.open("w+", encoding="utf-8") as log:
            server = subprocess.Popen(
                [sshd, "-D", "-e", "-f", str(root / "sshd_config")],
                stdout=log,
                stderr=log,
            )
            try:
                wait_ready(server, port)
                env = dict(os.environ, INSPIRUM_NATIVE_SSH_FIXTURE=str(root), TERM="xterm-256color")
                command = [
                    "cargo",
                    "test",
                    "--locked",
                    "--test",
                    "native_ssh_smoke",
                    "--",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ]
                print("RUN:", " ".join(command), flush=True)
                print(
                    f"Native isolated sshd: 127.0.0.1:{port}; user={user}; platform={sys.platform}; credentials removed on exit",
                    flush=True,
                )
                subprocess.run(command, env=env, check=True)
            finally:
                if server.poll() is None:
                    server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
                log.flush()
                log.seek(0)
                print("--- native disposable sshd log ---", flush=True)
                print(log.read(), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
