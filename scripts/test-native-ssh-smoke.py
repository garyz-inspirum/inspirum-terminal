#!/usr/bin/env python3
"""Cross-platform disposable SSH acceptance fixture.

Runs a loopback AsyncSSH server and then exercises Inspirum's real system
OpenSSH terminal path through the Rust native smoke test. Generated keys,
known_hosts files and logs live under the runner temporary directory only.
"""

from __future__ import annotations

import asyncio
import os
import time
from pathlib import Path
import socket
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
        if line in ("\x1b[D", "\x1bOD", "\x1b[C", "\x1bOC"):
            direction = "LEFT" if line.endswith("D") else "RIGHT"
            self.events.write(f"REMOTE_CURSOR_{direction}")
            self.chan.write(f"NATIVE_REMOTE_CURSOR_{direction}\r\n")
        elif line.startswith("echo:"):
            self.chan.write(f"NATIVE_ECHO:{line[5:]}\r\n")
        elif line == "size":
            self.chan.write(f"NATIVE_SIZE:{self.height} {self.width}\r\n")
        elif line == "agent-probe":
            asyncio.create_task(self.agent_probe())
        elif line == "exit":
            self.events.write("SESSION_EXIT_REQUEST")
            self.chan.exit(0)
            self.chan.close()

    async def agent_probe(self) -> None:
        assert self.chan is not None
        path = self.chan.get_agent_path()
        if not path:
            self.events.write("AGENT_DISABLED")
            self.chan.write("NATIVE_AGENT_DISABLED\r\n")
            return
        try:
            agent = await asyncio.wait_for(asyncssh.connect_agent(path), timeout=5)
            try:
                keys = await asyncio.wait_for(agent.get_keys(), timeout=5)
            finally:
                agent.close()
            if not keys:
                raise RuntimeError("forwarded agent returned no identities")
        except Exception as error:
            self.events.write(f"AGENT_PROBE_ERROR:{type(error).__name__}")
            self.chan.write("NATIVE_AGENT_ERROR\r\n")
        else:
            self.events.write("AGENT_FORWARDED")
            self.chan.write("NATIVE_AGENT_FORWARDED\r\n")

    def connection_lost(self, exc: Exception | None) -> None:
        state = "clean" if exc is None else type(exc).__name__
        self.events.write(f"SESSION_CLOSED:{state}")


class NativeServer(asyncssh.SSHServer):
    def __init__(
        self,
        allowed_key: asyncssh.SSHKey,
        events: EventLog,
        jump_target_port: int | None = None,
        echo_port: int | None = None,
        reverse_port: int | None = None,
    ) -> None:
        self.allowed_key = allowed_key
        self.events = events
        self.jump_target_port = jump_target_port
        self.echo_port = echo_port
        self.reverse_port = reverse_port

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

    def connection_requested(
        self, dest_host: str, dest_port: int, orig_host: str, orig_port: int
    ) -> bool:
        # Only the explicitly configured fixture target may be accessed
        # through the disposable jump server. No arbitrary remote forwarding.
        del orig_host, orig_port
        loopback = dest_host in ("127.0.0.1", "localhost")
        jump = self.jump_target_port is not None and dest_port == self.jump_target_port
        echo = self.echo_port is not None and dest_port == self.echo_port
        allowed = loopback and (jump or echo)
        label = "JUMP" if self.jump_target_port is not None else "LOCAL"
        self.events.write(f"{label}_FORWARD_{'ALLOW' if allowed else 'DENY'}")
        return allowed

    def server_requested(self, listen_host: str, listen_port: int) -> bool:
        allowed = (
            listen_host in ("127.0.0.1", "localhost")
            and self.reverse_port is not None
            and listen_port == self.reverse_port
        )
        self.events.write(f"REMOTE_FORWARD_{'ALLOW' if allowed else 'DENY'}")
        return allowed


def write_config(
    path: Path,
    *,
    port: int,
    identity: Path,
    known_hosts: Path,
    global_known_hosts: Path,
    identity_agent: Path | None = None,
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
                f" IdentityAgent {identity_agent}" if identity_agent else " IdentityAgent none",
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


def add_jump_host(
    config: Path,
    *,
    jump_port: int,
    identity: Path,
    known_hosts: Path,
    global_known_hosts: Path,
) -> None:
    with config.open("a", encoding="utf-8") as stream:
        stream.write(
            "\n".join([
                "Host native-hop",
                " HostName 127.0.0.1",
                f" Port {jump_port}",
                " User native-smoke",
                f" IdentityFile {identity}",
                " IdentitiesOnly yes",
                " IdentityAgent none",
                f" UserKnownHostsFile {known_hosts}",
                f" GlobalKnownHostsFile {global_known_hosts}",
                " StrictHostKeyChecking yes",
                " BatchMode yes",
                " ConnectTimeout 3",
                " ProxyCommand none",
                "",
            ])
        )


async def echo_service(
    reader: asyncio.StreamReader, writer: asyncio.StreamWriter
) -> None:
    try:
        while chunk := await reader.read(8192):
            writer.write(chunk)
            await writer.drain()
    finally:
        writer.close()
        await writer.wait_closed()


def available_local_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


async def main() -> int:
    root_base = Path(
        os.environ.get("INSPIRUM_TEST_TMPDIR", tempfile.gettempdir())
    ).resolve()
    root_base.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="native-ssh-smoke-", dir=root_base))
    agent_process = None
    agent_dir = None

    try:
        events = EventLog(root / "server.log")
        events.path.write_text("", encoding="utf-8")

        host_key = generate_key(root, "host")
        wrong_host_key = generate_key(root, "wrong-host")
        jump_host_key = generate_key(root, "jump-host")
        client_key = generate_key(root, "client")
        allowed_key = asyncssh.read_public_key(str(client_key) + ".pub")
        echo_server = await asyncio.start_server(echo_service, "127.0.0.1", 0)
        echo_port = int(echo_server.sockets[0].getsockname()[1])
        remote_port = available_local_port()
        (root / "echo-port").write_text(str(echo_port), encoding="utf-8")
        (root / "remote-port").write_text(str(remote_port), encoding="utf-8")

        server = await asyncssh.create_server(
            lambda: NativeServer(
                allowed_key, events, echo_port=echo_port, reverse_port=remote_port
            ),
            "127.0.0.1",
            0,
            server_host_keys=[str(host_key)],
            encoding="utf-8",
            agent_forwarding=True,
        )
        port = server.get_port()
        if port <= 0:
            raise RuntimeError(f"AsyncSSH did not allocate a TCP port: {port}")

        jump_events = EventLog(root / "jump.log")
        jump_events.path.write_text("", encoding="utf-8")
        jump_server = await asyncssh.create_server(
            lambda: NativeServer(allowed_key, jump_events, jump_target_port=port),
            "127.0.0.1",
            0,
            server_host_keys=[str(jump_host_key)],
            encoding="utf-8",
        )
        jump_port = jump_server.get_port()
        if jump_port <= 0:
            raise RuntimeError("AsyncSSH jump server did not bind a loopback port")

        host_type, host_data = public_fields(Path(str(host_key) + ".pub"))
        wrong_type, wrong_data = public_fields(Path(str(wrong_host_key) + ".pub"))
        jump_type, jump_data = public_fields(Path(str(jump_host_key) + ".pub"))
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
        # Separate port-scoped trust records for the destination and jump hop.
        hop_trust = f"[127.0.0.1]:{jump_port} {jump_type} {jump_data}\n"
        with known_hosts.open("a", encoding="utf-8") as file:
            file.write(hop_trust)
        with changed_known_hosts.open("a", encoding="utf-8") as file:
            file.write(hop_trust)
        # The target remains trusted but the jump server presents a different
        # key. Reject at the hop before any target authentication occurs.
        wrong_hop_known_hosts = root / "wrong-hop-known_hosts"
        wrong_hop_known_hosts.write_text(
            f"[127.0.0.1]:{port} {host_type} {host_data}\n"
            f"[127.0.0.1]:{jump_port} {wrong_type} {wrong_data}\n",
            encoding="utf-8",
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

        write_config(
            root / "jump-config",
            port=port,
            identity=client_key,
            known_hosts=known_hosts,
            global_known_hosts=global_known_hosts,
        )
        add_jump_host(
            root / "jump-config",
            jump_port=jump_port,
            identity=client_key,
            known_hosts=known_hosts,
            global_known_hosts=global_known_hosts,
        )
        write_config(
            root / "jump-changed-config",
            port=port,
            identity=client_key,
            known_hosts=changed_known_hosts,
            global_known_hosts=global_known_hosts,
        )
        add_jump_host(
            root / "jump-changed-config",
            jump_port=jump_port,
            identity=client_key,
            known_hosts=changed_known_hosts,
            global_known_hosts=global_known_hosts,
        )
        write_config(
            root / "jump-wrong-hop-config",
            port=port,
            identity=client_key,
            known_hosts=wrong_hop_known_hosts,
            global_known_hosts=global_known_hosts,
        )
        add_jump_host(
            root / "jump-wrong-hop-config",
            jump_port=jump_port,
            identity=client_key,
            known_hosts=wrong_hop_known_hosts,
            global_known_hosts=global_known_hosts,
        )
        write_config(
            root / "jump-broken-config",
            port=port,
            identity=client_key,
            known_hosts=known_hosts,
            global_known_hosts=global_known_hosts,
        )
        # A known destination must not silently succeed when its configured
        # jump is unavailable. TCP port 0 is never a valid connection target.
        add_jump_host(
            root / "jump-broken-config",
            jump_port=1,
            identity=client_key,
            known_hosts=known_hosts,
            global_known_hosts=global_known_hosts,
        )

        # Disposable local UNIX-domain ssh-agent. On Windows, agent forwarding
        # requires separate named-pipe capability acceptance.
        if os.name != "nt":
            agent_dir = Path(tempfile.mkdtemp(prefix="insp-forward-", dir="/tmp"))
            agent_socket = agent_dir / "agent.sock"
            agent_process = subprocess.Popen(
                ["ssh-agent", "-D", "-a", str(agent_socket)],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            deadline = time.monotonic() + 8
            while not agent_socket.exists() and agent_process.poll() is None:
                if time.monotonic() >= deadline:
                    raise RuntimeError("isolated ssh-agent socket did not become ready")
                await asyncio.sleep(.05)
            if not agent_socket.exists():
                raise RuntimeError("isolated ssh-agent exited before creating its socket")
            run(
                ["ssh-add", str(client_key)],
                env=dict(os.environ, SSH_AUTH_SOCK=str(agent_socket)),
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            write_config(
                root / "agent-config",
                port=port, identity=client_key, known_hosts=known_hosts,
                global_known_hosts=global_known_hosts, identity_agent=agent_socket,
            )

        env = dict(os.environ)
        if agent_dir is not None:
            # ForwardAgent=yes resolves the same isolated socket as IdentityAgent.
            env["SSH_AUTH_SOCK"] = str(agent_socket)
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
            jump_server.close()
            server.close()
            echo_server.close()
            await jump_server.wait_closed()
            await server.wait_closed()
            await echo_server.wait_closed()

        log = events.path.read_text(encoding="utf-8")
        print("--- disposable native SSH server events ---")
        print(log, end="")
        if "AUTH_ACCEPT:native-smoke" not in log:
            raise RuntimeError("native SSH fixture never authenticated the test key")
        jump_log = jump_events.path.read_text(encoding="utf-8")
        if "JUMP_FORWARD_ALLOW" not in jump_log:
            raise RuntimeError("the native ProxyJump acceptance never opened its hop")
        return 0
    finally:
        # Teardown also runs when agent startup, ssh-add or fixture validation
        # raises before entering the inner cargo-test cleanup block.
        if agent_process is not None and agent_process.poll() is None:
            agent_process.terminate()
            try:
                agent_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                agent_process.kill()
                agent_process.wait(timeout=5)
        if agent_dir is not None:
            shutil.rmtree(agent_dir, ignore_errors=True)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
