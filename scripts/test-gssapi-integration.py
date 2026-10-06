#!/usr/bin/env python3
"""Run disposable Kerberos/OpenSSH GSSAPI acceptance on Linux CI."""

from __future__ import annotations

import getpass
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time

REALM = "INSPIRUM.TEST"


def tool(name: str) -> str:
    path = shutil.which(name)
    if not path:
        raise SystemExit(f"ERROR: required GSSAPI fixture tool is missing: {name}")
    return path


def reserve_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def wait_tcp(process: subprocess.Popen[bytes], port: int, label: str) -> None:
    deadline = time.monotonic() + 10
    while True:
        if process.poll() is not None:
            raise RuntimeError(f"{label} exited during startup")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            if time.monotonic() >= deadline:
                raise RuntimeError(f"{label} did not listen on 127.0.0.1:{port}")
            time.sleep(0.05)


def run(args: list[str], env: dict[str, str], *, input_text: str | None = None) -> None:
    subprocess.run(
        args,
        env=env,
        input=input_text,
        text=input_text is not None,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        check=True,
    )


def main() -> int:
    if os.name == "nt":
        raise SystemExit("ERROR: GSSAPI fixture is Linux-only")

    sshd = shutil.which("sshd") or "/usr/sbin/sshd"
    if not Path(sshd).is_file():
        raise SystemExit("ERROR: fixture requires sshd")

    kdb5_util = tool("kdb5_util")
    kadmin_local = tool("kadmin.local")
    krb5kdc = tool("krb5kdc")
    kinit = tool("kinit")
    kdestroy = tool("kdestroy")
    klist = tool("klist")
    kvno = tool("kvno")
    ssh_keygen = tool("ssh-keygen")
    ssh = tool("ssh")

    parent = Path(os.environ.get("INSPIRUM_TEST_TMPDIR", tempfile.gettempdir())).resolve()
    parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="gssapi-fixture-", dir=parent) as tmp:
        root = Path(tmp)
        kdc_port = reserve_port()
        ssh_port = reserve_port()
        user = getpass.getuser()
        principal = f"{user}@{REALM}"
        canonical_host = socket.getfqdn().lower().rstrip(".")
        service_principal = f"host/{canonical_host}@{REALM}"
        fixture_password = "inspirum-gssapi-fixture-password"
        master_password = "inspirum-gssapi-master-password"

        krb5_conf = root / "krb5.conf"
        kdc_conf = root / "kdc.conf"
        cache = root / "ccache"
        keytab = root / "sshd.keytab"

        krb5_conf.write_text(
            f"""[libdefaults]
 default_realm = {REALM}
 dns_lookup_kdc = false
 dns_lookup_realm = false
 rdns = false
 dns_canonicalize_hostname = false
 ticket_lifetime = 10m
 forwardable = true

[realms]
 {REALM} = {{
  kdc = 127.0.0.1:{kdc_port}
 }}

[domain_realm]
 localhost = {REALM}
 .localhost = {REALM}
""",
            encoding="utf-8",
        )
        kdc_conf.write_text(
            f"""[kdcdefaults]
 kdc_ports = {kdc_port}
 kdc_tcp_ports = {kdc_port}

[realms]
 {REALM} = {{
  database_name = {root / "principal"}
  key_stash_file = {root / ".k5.stash"}
  acl_file = {root / "kadm5.acl"}
  admin_keytab = {root / "kadm5.keytab"}
  max_life = 10m
  max_renewable_life = 30m
  supported_enctypes = aes256-cts-hmac-sha1-96:normal aes128-cts-hmac-sha1-96:normal
 }}
""",
            encoding="utf-8",
        )
        (root / "kadm5.acl").write_text(f"*/admin@{REALM} *\n", encoding="utf-8")

        env = dict(
            os.environ,
            KRB5_CONFIG=str(krb5_conf),
            KRB5_KDC_PROFILE=str(kdc_conf),
            KRB5CCNAME=f"FILE:{cache}",
        )

        run([kdb5_util, "create", "-s", "-P", master_password, "-r", REALM], env)
        run(
            [kadmin_local, "-r", REALM, "-q", f"addprinc -pw {fixture_password} {principal}"],
            env,
        )
        for spn in (f"host/localhost@{REALM}", service_principal):
            run(
                [kadmin_local, "-r", REALM, "-q", f"addprinc -randkey {spn}"],
                env,
            )
            run(
                [
                    kadmin_local,
                    "-r",
                    REALM,
                    "-q",
                    f"ktadd -k {keytab} {spn}",
                ],
                env,
            )

        kdc_log = (root / "kdc.log").open("w+", encoding="utf-8")
        sshd_log = (root / "sshd.log").open("w+", encoding="utf-8")
        kdc = subprocess.Popen(
            [krb5kdc, "-n", "-r", REALM],
            env=env,
            stdout=kdc_log,
            stderr=kdc_log,
        )
        server = None
        try:
            wait_tcp(kdc, kdc_port, "Kerberos KDC")
            run([kinit, principal], env, input_text=fixture_password + "\n")
            run([klist, "-s"], env)
            service = subprocess.run(
                [kvno, service_principal],
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            if service.returncode != 0:
                raise RuntimeError(
                    "Kerberos service-ticket acquisition failed: "
                    + service.stderr.replace(fixture_password, "[redacted]")
                )
            print(
                "PASS disposable Kerberos client and host service tickets acquired: "
                + service.stdout.strip(),
                flush=True,
            )

            for name in ("host", "wrong-host"):
                run(
                    [ssh_keygen, "-q", "-t", "ed25519", "-N", "", "-f", str(root / name)],
                    env,
                )
            host_fields = (root / "host.pub").read_text(encoding="utf-8").split()
            (root / "known_hosts").write_text(
                f"[127.0.0.1]:{ssh_port} {host_fields[0]} {host_fields[1]}\n",
                encoding="utf-8",
            )

            remote = root / "remote.sh"
            remote.write_text(
                """#!/bin/sh
printf 'FIXTURE_GSSAPI_AUTHENTICATED\n'
if [ -n "${KRB5CCNAME:-}" ] && klist -s 2>/dev/null; then
  printf 'REMOTE_GSSAPI_DELEGATED:yes\n'
else
  printf 'REMOTE_GSSAPI_DELEGATED:no\n'
fi
while IFS= read -r line; do
  [ "$line" = exit ] && exit 0
done
""",
                encoding="utf-8",
            )
            remote.chmod(0o755)

            sshd_config = root / "sshd_config"
            sshd_config.write_text(
                f"""ListenAddress 127.0.0.1
Port {ssh_port}
HostKey {root / "host"}
PidFile {root / "sshd.pid"}
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication no
GSSAPIAuthentication yes
GSSAPICleanupCredentials yes
GSSAPIStrictAcceptorCheck no
AuthenticationMethods gssapi-with-mic
KerberosAuthentication no
AllowUsers {user}
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
PermitTTY yes
PrintMotd no
PrintLastLog no
ForceCommand /bin/sh {remote}
LogLevel DEBUG3
""",
                encoding="utf-8",
            )
            run([sshd, "-t", "-f", str(sshd_config)], env)

            config = root / "config"
            config.write_text(
                f"""Host gssapi-fixture
 HostName 127.0.0.1
 Port {ssh_port}
 User {user}
 GSSAPIAuthentication yes
 GSSAPIDelegateCredentials no
 GSSAPITrustDns no
 GSSAPIServerIdentity host@{canonical_host}
 PreferredAuthentications gssapi-with-mic
 PubkeyAuthentication no
 PasswordAuthentication no
 KbdInteractiveAuthentication no
 UserKnownHostsFile {root / "known_hosts"}
 GlobalKnownHostsFile /dev/null
 StrictHostKeyChecking yes
 BatchMode yes
 ConnectTimeout 5
 UpdateHostKeys no
 ControlMaster no
 ControlPath none
 ProxyCommand none
""",
                encoding="utf-8",
            )

            ssh_env = dict(env, KRB5_KTNAME=str(keytab))
            server = subprocess.Popen(
                [sshd, "-D", "-e", "-f", str(sshd_config)],
                env=ssh_env,
                stdout=sshd_log,
                stderr=sshd_log,
            )
            wait_tcp(server, ssh_port, "GSSAPI sshd")

            baseline = subprocess.run(
                [ssh, "-vvv", "-T", "-F", str(config), "gssapi-fixture"],
                env=env,
                input="exit\n",
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            )
            if (
                baseline.returncode != 0
                or "FIXTURE_GSSAPI_AUTHENTICATED" not in baseline.stdout
            ):
                raise RuntimeError(
                    "system OpenSSH GSSAPI baseline failed: "
                    + baseline.stderr.replace(fixture_password, "[redacted]")
                )
            print("PASS system OpenSSH GSSAPI baseline", flush=True)

            rust_env = dict(
                env,
                INSPIRUM_GSSAPI_FIXTURE=str(root),
                TERM="xterm-256color",
            )
            success = [
                "cargo",
                "test",
                "--locked",
                "--test",
                "gssapi_integration",
                "gssapi_authenticates_with_disposable_ticket_and_no_delegation",
                "--",
                "--ignored",
                "--nocapture",
                "--exact",
            ]
            print("RUN:", " ".join(success), flush=True)
            subprocess.run(success, env=rust_env, check=True)

            run([kdestroy], env)
            missing = [
                "cargo",
                "test",
                "--locked",
                "--test",
                "gssapi_integration",
                "gssapi_missing_ticket_fails_without_auth_fallback",
                "--",
                "--ignored",
                "--nocapture",
                "--exact",
            ]
            print("RUN:", " ".join(missing), flush=True)
            subprocess.run(missing, env=rust_env, check=True)
        finally:
            if server is not None and server.poll() is None:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
            if kdc.poll() is None:
                kdc.terminate()
                try:
                    kdc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    kdc.kill()
                    kdc.wait(timeout=5)
            sshd_log.flush()
            sshd_log.seek(0)
            print("--- disposable GSSAPI sshd log ---", flush=True)
            print(sshd_log.read().replace(fixture_password, "[redacted]"), flush=True)
            kdc_log.close()
            sshd_log.close()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
