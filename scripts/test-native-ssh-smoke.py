#!/usr/bin/env python3
"""Cross-platform disposable SSH acceptance fixture.

Runs a loopback AsyncSSH server and then exercises Inspirum's real system
OpenSSH terminal path through the Rust native smoke test. Generated keys,
known_hosts files and logs live under the runner temporary directory only.
"""

from __future__ import annotations

import asyncio
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

import asyncssh


def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, check=True, text=True, **kwargs)


def generate_key(root: Path, name: str) -> Path:
    path = root / name
    run([
        "ssh-keygen",
        "-q",
        "-t",
        "ed25519",
        "-N",
        "",
        "-f",
        str(path),
    ])
    return path


def public_fields(path: Path) -> tuple[str, str]:
    fields = path.read_text(encoding="utf-8").strip().split()
    if len(fields) < 2:
        raise RuntimeError(f"invalid OpenSSH public key: {path}")
    return fields[0], fields[1]


class EventLog:
    def __init__(self, path: Path) -> None:
        self.path = path

    def write(self, event: str) -> None:
        with self.path.open("a", encoding="utf-8") as stream:
            stream.write(event + "\n")
            stream.flush()


class NativeSession(asyncssh.SSHServerSession):
    def __init__(self, events: EventLog) -> None:
        self.events = events
        self.chan: asyncssh.SSHServerChannel | None = None
        self.width = 80
        self.height = 24
        self.buffer = ""

    def connection_made(self, chan: asyncssh.SSHServerChannel) -> None:
        self.chan = chan
        self.events.write("SESSION_OPEN")

    def pty_requested(
        self,
        term_type: str,
        term_size: tuple[int, int, int, int],
        term_modes: dict[int, int],
    ) -> bool:
        del term_type, term_modes
        self.width, self.height = term_size[0], term_size[1]
        self.events.write(f"PTY:{self.width}x{self.height}")
        return True

    def shell_requested(self) -> bool:
        return True

    def session_started(self) -> None:
        assert self.chan is not None
        self.events.write("SESSION_STARTED")
        self.chan.write("NATIVE_SMOKE_READY\r\n")

    def terminal_size_changed(
        self, width: int, height: int, pixwidth: int, pixheight: int
    ) -> None:
        del pixwidth, pixheight
        self.width, self.height = width, height
        self.events.write(f"RESIZE:{width}x{height}")

    def data_received(self, data: str, datatype: asyncssh.DataType) -> None:
        del datatype
        self.buffer += data.replace("\r\n", "\n").replace("\r", "\n")
        while "\n" in self.buffer:
            line, self.buffer = self.buffer.split("\n", 1)
            self.handle_line(line)

    def handle_line(self, line: str) -> None:
        assert self.chan is not None
        if line.startswith("echo:"):
            self.chan.write(f"NATIVE_ECHO:{line[5:]}\r\n")
        elif line == "size":
            self.chan.write(f"NATIVE_SIZE:{self.height} {self.width}\r\n")
        elif line == "exit":
            self.events.write("SESSION_EXIT_REQUEST")
            self.chan.exit(0)
            self.chan.close()

    def connection_lost(self, exc: Exception | None) -> None:
        state = "clean" if exc is None else type(exc).__name__
        self.events.write(f"SESSION_CLOSED:{state}")


class NativeServer(asyncssh.SSHServer):
    def __init__(self, allowed_key: asyncssh.SSHKey, events: EventLog) -> None:
        self.allowed_key = allowed_key
        self.events = events

    def connection_made(self, conn: asyncssh.SSHServerConnection) -> None:
        del conn
        self.events.write("CONNECTION_OPEN")

    def connection_lost(self, exc: Exception | None) -> None:
        state = "clean" if exc is None else type(exc).__name__
        self.events.write(f"CONNECTION_CLOSED:{state}")

    def begin_auth(self, username: str) -> bool:
        self.events.write(f"AUTH_BEGIN:{username}")
        return True

    def public_key_auth_supported(self) -> bool:
        return True

    def validate_public_key(self, username: str, key: asyncssh.SSHKey) -> bool:
        accepted = username == "native-smoke" and key == self.allowed_key
        self.events.write(f"AUTH_{'ACCEPT' if accepted else 'REJECT'}:{username}")
        return accepted

    def session_requested(self) -> NativeSession:
        return NativeSession(self.events)


def write_config(
    path: Path,
    *,
    port: int,
    identity: Path,
    known_hosts: Path,
    global_known_hosts: Path,
) -> None:
    # Keep every file runner-local and avoid platform-specific /dev/null/NUL paths.
    path.write_text(
        "\n".join(
            [
                "Host native-smoke",
                " HostName 127.0.0.1",
                f" Port {port}",
                " User native-smoke",
                f" IdentityFile {identity}",
                " IdentitiesOnly yes",
                " IdentityAgent none",
                f" UserKnownHostsFile {known_hosts}",
                f" GlobalKnownHostsFile {global_known_hosts}",
                " BatchMode yes",
                " ConnectTimeout 3",
                " UpdateHostKeys no",
                " ControlMaster no",
                " ProxyCommand none",
                "",
            ]
        ),
        encoding="utf-8",
    )


async def main() -> int:
    root_base = Path(
        os.environ.get("INSPIRUM_TEST_TMPDIR", tempfile.gettempdir())
    ).resolve()
    root_base.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="native-ssh-smoke-", dir=root_base))

    try:
        events = EventLog(root / "server.log")
        events.path.write_text("", encoding="utf-8")

        host_key = generate_key(root, "host")
        wrong_host_key = generate_key(root, "wrong-host")
        client_key = generate_key(root, "client")
        allowed_key = asyncssh.read_public_key(str(client_key) + ".pub")

        server = await asyncssh.create_server(
            lambda: NativeServer(allowed_key, events),
            "127.0.0.1",
            0,
            server_host_keys=[str(host_key)],
            encoding="utf-8",
        )
        port = server.get_port()
        if port <= 0:
            raise RuntimeError(f"AsyncSSH did not allocate a TCP port: {port}")

        host_type, host_data = public_fields(Path(str(host_key) + ".pub"))
        wrong_type, wrong_data = public_fields(Path(str(wrong_host_key) + ".pub"))
        known_hosts = root / "known_hosts"
        changed_known_hosts = root / "changed_known_hosts"
        global_known_hosts = root / "global_known_hosts"
        global_known_hosts.write_text("", encoding="utf-8")
        known_hosts.write_text(
            f"[127.0.0.1]:{port} {host_type} {host_data}\n", encoding="utf-8"
        )
        changed_known_hosts.write_text(
            f"[127.0.0.1]:{port} {wrong_type} {wrong_data}\n", encoding="utf-8"
        )
        write_config(
            root / "config",
            port=port,
            identity=client_key,
            known_hosts=known_hosts,
            global_known_hosts=global_known_hosts,
        )
        write_config(
            root / "changed-config",
            port=port,
            identity=client_key,
            known_hosts=changed_known_hosts,
            global_known_hosts=global_known_hosts,
        )

        env = dict(os.environ)
        env["INSPIRUM_NATIVE_SSH_FIXTURE"] = str(root)
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
        print(f"Disposable native SSH server: 127.0.0.1:{port}", flush=True)

        try:
            await asyncio.to_thread(run, command, env=env)
        finally:
            server.close()
            await server.wait_closed()

        log = events.path.read_text(encoding="utf-8")
        print("--- disposable native SSH server events ---")
        print(log, end="")
        if "AUTH_ACCEPT:native-smoke" not in log:
            raise RuntimeError("native SSH fixture never authenticated the test key")
        return 0
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
