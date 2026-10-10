#!/usr/bin/env python3
"""Disposable Linux MIT-Kerberos + OpenSSH GSSAPI acceptance.

No external realm, production principal, ambient ccache or user keytab is used.
The temporary realm, principals, credentials and SSH host keys are deleted.
"""
from __future__ import annotations

import os
from pathlib import Path
import pwd
import shutil
import socket
import subprocess
import tempfile
import time

REALM = "INSPIRUM.TEST"


def run(args: list[str], env: dict[str, str], *, timeout: int = 45) -> str:
    process = subprocess.run(
        args, env=env, capture_output=True, text=True, timeout=timeout
    )
    if process.returncode != 0:
        # Never print KDC passwords, keytabs, caches or kadmin output.
        # Only the isolated Rust acceptance test may expose a bounded
        # failure summary, so misconfigured delegation is diagnosable.
        if args[0] == "cargo":
            print(process.stdout[-2500:], flush=True)
            print(process.stderr[-600:], flush=True)
        raise RuntimeError(f"disposable fixture command failed: {args[0]} (status {process.returncode})")
    return process.stdout


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def wait_tcp(port: int, process: subprocess.Popen[bytes]) -> None:
    for _ in range(120):
        if process.poll() is not None:
            raise RuntimeError(f"isolated daemon exited before binding port {port}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.15):
                return
        except OSError:
            time.sleep(0.1)
    raise RuntimeError(f"isolated daemon did not listen on {port}")


def write_config(root: Path, user: str, kdc_port: int, ssh_port: int) -> None:
    (root / "krb5.conf").write_text(
        f"""[libdefaults]
 default_realm = {REALM}
 dns_lookup_kdc = false
 dns_lookup_realm = false
 rdns = false
 udp_preference_limit = 1
 default_ccache_name = FILE:{root / 'ticket-cache'}
[realms]
 {REALM} = {{
  kdc = 127.0.0.1:{kdc_port}
 }}
[domain_realm]
 localhost = {REALM}
 .localhost = {REALM}
""", encoding="utf-8"
    )
    (root / "kdc.conf").write_text(
        f"""[kdcdefaults]
 kdc_ports = {kdc_port}
 kdc_tcp_ports = {kdc_port}
[realms]
 {REALM} = {{
  database_name = {root / 'principal'}
  key_stash_file = {root / 'stash'}
  acl_file = {root / 'kadm5.acl'}
  max_life = 1h
  max_renewable_life = 2h
 }}
""", encoding="utf-8"
    )
    (root / "kadm5.acl").write_text(f"*/admin@{REALM} *\n", encoding="utf-8")
    (root / "sshd_config").write_text(
        f"""Port {ssh_port}
ListenAddress 127.0.0.1
HostKey {root / 'sshd_host'}
PidFile {root / 'sshd.pid'}
AuthorizedKeysFile none
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication no
GSSAPIAuthentication yes
GSSAPICleanupCredentials no
GSSAPIStrictAcceptorCheck no
UsePAM no
PermitRootLogin no
AllowUsers {user}
LogLevel VERBOSE
""", encoding="utf-8"
    )
    public = (root / "sshd_host.pub").read_text(encoding="utf-8").split()
    (root / "known_hosts").write_text(
        f"[localhost]:{ssh_port} {public[0]} {public[1]}\n", encoding="utf-8"
    )
    (root / "ssh_config").write_text(
        f"""Host krb-probe
 HostName localhost
 Port {ssh_port}
 User {user}
 StrictHostKeyChecking yes
 UserKnownHostsFile {root / 'known_hosts'}
 GlobalKnownHostsFile {root / 'empty_known_hosts'}
 GSSAPIAuthentication yes
 GSSAPIDelegateCredentials no
 GSSAPITrustDNS no
 PreferredAuthentications gssapi-with-mic
 PubkeyAuthentication no
 PasswordAuthentication no
 KbdInteractiveAuthentication no
 IdentityAgent none
 BatchMode yes
 ConnectTimeout 5
 ProxyCommand none
 ControlMaster no
""", encoding="utf-8"
    )
    (root / "empty_known_hosts").write_text("", encoding="utf-8")


def main() -> int:
    if os.name != "posix" or not os.path.exists("/proc"):
        raise RuntimeError("This isolated Kerberos fixture is currently Linux-only")
    username = pwd.getpwuid(os.getuid()).pw_name
    if username == "root":
        raise RuntimeError("run disposable Kerberos tests as the unprivileged CI user")
    for command in ("krb5kdc", "kdb5_util", "kadmin.local", "kinit", "kdestroy", "klist", "ssh", "ssh-keygen"):
        if not shutil.which(command):
            raise RuntimeError(f"required disposable Kerberos tool missing: {command}")

    root = Path(tempfile.mkdtemp(prefix="inspirum-krb-test-"))
    kdc_proc = None
    sshd_proc = None
    log_files = []
    try:
        kdc_port, ssh_port = free_port(), free_port()
        env = os.environ.copy()
        env.update({
            "KRB5_CONFIG": str(root / "krb5.conf"),
            "KRB5_KDC_PROFILE": str(root / "kdc.conf"),
            "KRB5CCNAME": f"FILE:{root / 'ticket-cache'}",
        })
        run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root / "sshd_host")], env)
        write_config(root, username, kdc_port, ssh_port)

        # Fail rather than report a fake success on OpenSSH without GSSAPI.
        run(["ssh", "-G", "-F", str(root / "ssh_config"), "-o", "GSSAPIAuthentication=yes", "krb-probe"], env)
        run(["kdb5_util", "-r", REALM, "create", "-s", "-P", "disposable-fixture-only"], env)
        for principal in (f"{username}@{REALM}", f"host/localhost@{REALM}"):
            run(["kadmin.local", "-r", REALM, "-q", f"addprinc -randkey {principal}"], env)
        # Kerberos clients delegate tickets only to a service explicitly
        # trusted for delegation by the disposable KDC policy.
        run(["kadmin.local", "-r", REALM, "-q",
             f"modprinc +ok_as_delegate host/localhost@{REALM}"], env)
        run(["kadmin.local", "-r", REALM, "-q",
             f"ktadd -k {root / 'client.keytab'} {username}@{REALM}"], env)
        run(["kadmin.local", "-r", REALM, "-q",
             f"ktadd -k {root / 'service.keytab'} host/localhost@{REALM}"], env)

        kdc_log = (root / "kdc.log").open("wb")
        log_files.append(kdc_log)
        kdc_proc = subprocess.Popen(["krb5kdc", "-n"], env=env, stdout=kdc_log, stderr=subprocess.STDOUT)
        wait_tcp(kdc_port, kdc_proc)
        run(["kinit", "-kt", str(root / "client.keytab"), f"{username}@{REALM}"], env)
        run(["klist", "-s"], env)

        sshd_env = env.copy()
        sshd_env.pop("KRB5CCNAME", None)
        sshd_env["KRB5_KTNAME"] = f"FILE:{root / 'service.keytab'}"
        run(["/usr/sbin/sshd", "-t", "-f", str(root / "sshd_config")], sshd_env)
        sshd_log = (root / "sshd.log").open("wb")
        log_files.append(sshd_log)
        sshd_proc = subprocess.Popen(
            ["/usr/sbin/sshd", "-D", "-f", str(root / "sshd_config")],
            env=sshd_env, stdout=sshd_log, stderr=subprocess.STDOUT
        )
        wait_tcp(ssh_port, sshd_proc)

        # Independent control: system OpenSSH must succeed against exactly the
        # same isolated realm and server before Inspirum's adapter is tested.
        baseline = run(
            ["ssh", "-F", str(root / "ssh_config"), "krb-probe", "echo GSSAPI_BASELINE_OK"],
            env, timeout=20
        )
        if "GSSAPI_BASELINE_OK" not in baseline:
            raise RuntimeError("system OpenSSH Kerberos control did not authenticate")

        env["INSPIRUM_GSSAPI_FIXTURE"] = str(root)
        for mode in ("no-delegation", "delegation"):
            env["INSPIRUM_GSSAPI_MODE"] = mode
            run(["cargo", "test", "--locked", "--test", "native_gssapi", "--",
                 "--ignored", "--nocapture", "--test-threads=1"], env, timeout=180)
            print(f"PASS actual Inspirum GSSAPI: {mode}", flush=True)

        run(["kdestroy"], env)
        env["INSPIRUM_GSSAPI_MODE"] = "no-ticket"
        run(["cargo", "test", "--locked", "--test", "native_gssapi", "--",
             "--ignored", "--nocapture", "--test-threads=1"], env, timeout=180)
        print("PASS GSSAPI missing ticket is fail-closed", flush=True)
        return 0
    finally:
        for process in (sshd_proc, kdc_proc):
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        for stream in log_files:
            stream.close()
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
