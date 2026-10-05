#!/usr/bin/env bash
# Linux-only unprivileged disposable loopback sshd; no production config touched.
# Runs the SSH integration fixture tests only. Not Windows or macOS native execution.
set -euo pipefail
cd "$(dirname "$0")/.."
# Respect standard Cargo overrides; keep generated credentials outside the checkout.
export TERM=xterm-256color
python3 - <<'PY'
import getpass, os, pathlib, shutil, socket, subprocess, tempfile, time
default_tmp = str(pathlib.Path(os.environ['CARGO_TARGET_DIR']).resolve().parent) if os.environ.get('CARGO_TARGET_DIR') else tempfile.gettempdir()
root=pathlib.Path(os.environ.get('INSPIRUM_TEST_TMPDIR', default_tmp))
root.mkdir(parents=True, exist_ok=True)
sshd=shutil.which('sshd') or '/usr/sbin/sshd'
if not pathlib.Path(sshd).is_file(): raise SystemExit('ERROR: fixture requires sshd')
with tempfile.TemporaryDirectory(prefix='ssh-fixture-',dir=root) as tmp:
 d=pathlib.Path(tmp);os.chmod(d,0o700)
 for key in ('host','client','wrong-host'):
  subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(d/key)],check=True)
 (d/'authorized_keys').write_text((d/'client.pub').read_text())
 (d/'remote.sh').write_text('''#!/bin/sh
stty -echo
printf '%s\\n' "$$" > "'''+str(d)+'''/remote.pid"
printf 'FIXTURE_AUTHENTICATED\\n'
if [ -n "${SSH_ORIGINAL_COMMAND:-}" ]; then
 printf 'REMOTE_COMMAND:%s\\n' "$SSH_ORIGINAL_COMMAND"
 exit 0
fi
while IFS= read -r line; do
 case "$line" in
 echo:*) printf 'REMOTE_ECHO:%s\\n' "${line#echo:}" ;;
 size) printf 'REMOTE_SIZE:'; stty size ;;
 exit) exit 0 ;;
 esac
done
'''.replace('\\n','\n'))
 with socket.socket() as sock, socket.socket() as jump_sock, socket.socket() as sftp_sock:
  sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
  jump_sock.bind(('127.0.0.1',0));jump_port=jump_sock.getsockname()[1]
  sftp_sock.bind(('127.0.0.1',0));sftp_port=sftp_sock.getsockname()[1]
 assert min(port,jump_port,sftp_port)>1024 and len({port,jump_port,sftp_port})==3
 (d/'sshd_config').write_text(f'''ListenAddress 127.0.0.1
Port {port}
HostKey {d}/host
PidFile {d}/sshd.pid
AuthorizedKeysFile {d}/authorized_keys
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
AuthenticationMethods publickey
AllowUsers {getpass.getuser()}
AllowTcpForwarding yes
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
PermitTTY yes
PrintMotd no
PrintLastLog no
ForceCommand /bin/sh {d}/remote.sh
LogLevel VERBOSE
''')
 (d/'jump_sshd_config').write_text(f'''ListenAddress 127.0.0.1
Port {jump_port}
HostKey {d}/host
PidFile {d}/jump_sshd.pid
AuthorizedKeysFile {d}/authorized_keys
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
AuthenticationMethods publickey
AllowUsers {getpass.getuser()}
AllowTcpForwarding yes
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
PermitTTY no
PrintMotd no
PrintLastLog no
LogLevel VERBOSE
''')
 (d/'sftp-root').mkdir()
 (d/'sftp_sshd_config').write_text(f'''ListenAddress 127.0.0.1
Port {sftp_port}
HostKey {d}/host
PidFile {d}/sftp_sshd.pid
AuthorizedKeysFile {d}/authorized_keys
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
AuthenticationMethods publickey
AllowUsers {getpass.getuser()}
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
PermitTTY yes
PrintMotd no
PrintLastLog no
Subsystem sftp internal-sftp
ForceCommand internal-sftp -d {d}/sftp-root
LogLevel VERBOSE
''')
 host_fields=(d/'host.pub').read_text().split()
 wrong_fields=(d/'wrong-host.pub').read_text().split()
 target_host=f'[127.0.0.1]:{port} {host_fields[0]} {host_fields[1]}\n'
 jump_host=f'[127.0.0.1]:{jump_port} {host_fields[0]} {host_fields[1]}\n'
 sftp_host=f'[127.0.0.1]:{sftp_port} {host_fields[0]} {host_fields[1]}\n'
 changed_target=f'[127.0.0.1]:{port} {wrong_fields[0]} {wrong_fields[1]}\n'
 (d/'known_hosts').write_text(target_host+jump_host+sftp_host)
 (d/'changed_known_hosts').write_text(changed_target+jump_host+sftp_host)
 for name,known in [('config','known_hosts'),('changed-config','changed_known_hosts')]:
  (d/name).write_text(f'''Host fixture-jump
 HostName 127.0.0.1
 Port {jump_port}
Host fixture-sftp
 HostName 127.0.0.1
 Port {sftp_port}
Host *
 HostName 127.0.0.1
 Port {port}
 User {getpass.getuser()}
 IdentityFile {d}/client
 IdentitiesOnly yes
 IdentityAgent none
 UserKnownHostsFile {d}/{known}
 GlobalKnownHostsFile /dev/null
 BatchMode yes
 ConnectTimeout 3
 UpdateHostKeys no
 ControlMaster no
 ControlPath none
 ProxyCommand none
''')
 subprocess.run([sshd,'-t','-f',str(d/'sshd_config')],check=True)
 subprocess.run([sshd,'-t','-f',str(d/'jump_sshd_config')],check=True)
 subprocess.run([sshd,'-t','-f',str(d/'sftp_sshd_config')],check=True)
 def wait_ready(server,listen_port,label):
  deadline=time.monotonic()+5
  while True:
   if server.poll() is not None: raise RuntimeError(f'{label} sshd exited at startup')
   try:
    with socket.create_connection(('127.0.0.1',listen_port),timeout=.2): return
   except OSError:
    if time.monotonic()>deadline: raise
    time.sleep(.05)
 with (d/'sshd.log').open('w+') as log, (d/'jump_sshd.log').open('w+') as jump_log, (d/'sftp_sshd.log').open('w+') as sftp_log:
  server=subprocess.Popen([sshd,'-D','-e','-f',str(d/'sshd_config')],stdout=log,stderr=log)
  jump_server=subprocess.Popen([sshd,'-D','-e','-f',str(d/'jump_sshd_config')],stdout=jump_log,stderr=jump_log)
  sftp_server=subprocess.Popen([sshd,'-D','-e','-f',str(d/'sftp_sshd_config')],stdout=sftp_log,stderr=sftp_log)
  try:
   wait_ready(server,port,'target')
   wait_ready(jump_server,jump_port,'jump')
   wait_ready(sftp_server,sftp_port,'sftp')
   env=dict(os.environ,INSPIRUM_SSH_FIXTURE=str(d))
   cmd=['cargo','test','--locked','--test','ssh_integration','--','--ignored','--nocapture','--test-threads=1']
   print('RUN:',' '.join(cmd),flush=True)
   print(f'Isolated target sshd: 127.0.0.1:{port}; jump sshd: 127.0.0.1:{jump_port}; sftp sshd: 127.0.0.1:{sftp_port}; keys removed on exit',flush=True)
   subprocess.run(cmd,env=env,check=True)
  finally:
   for process in (server,jump_server,sftp_server):
    process.terminate()
   for process in (server,jump_server,sftp_server):
    try: process.wait(timeout=5)
    except subprocess.TimeoutExpired: process.kill();process.wait()
   log.seek(0);print('--- disposable target sshd log ---\n'+log.read(),flush=True)
   jump_log.seek(0);print('--- disposable jump sshd log ---\n'+jump_log.read(),flush=True)
   sftp_log.seek(0);print('--- disposable sftp sshd log ---\n'+sftp_log.read(),flush=True)
PY
