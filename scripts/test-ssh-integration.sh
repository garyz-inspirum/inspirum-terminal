#!/usr/bin/env bash
# Linux-only unprivileged disposable loopback sshd; no production config touched.
# Runs the three ignored fixture tests only. Not Windows or macOS native execution.
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
while IFS= read -r line; do
 case "$line" in
 echo:*) printf 'REMOTE_ECHO:%s\\n' "${line#echo:}" ;;
 size) printf 'REMOTE_SIZE:'; stty size ;;
 exit) exit 0 ;;
 esac
done
'''.replace('\\n','\n'))
 with socket.socket() as sock:
  sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
 assert port>1024
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
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
PermitTTY yes
PrintMotd no
PrintLastLog no
ForceCommand /bin/sh {d}/remote.sh
LogLevel VERBOSE
''')
 for name,key in [('known_hosts','host'),('changed_known_hosts','wrong-host')]:
  fields=(d/(key+'.pub')).read_text().split()
  (d/name).write_text(f'[127.0.0.1]:{port} {fields[0]} {fields[1]}\n')
 for name,known in [('config','known_hosts'),('changed-config','changed_known_hosts')]:
  (d/name).write_text(f'''Host *
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
 with (d/'sshd.log').open('w+') as log:
  server=subprocess.Popen([sshd,'-D','-e','-f',str(d/'sshd_config')],stdout=log,stderr=log)
  try:
   deadline=time.monotonic()+5
   while True:
    if server.poll() is not None: raise RuntimeError('isolated sshd exited at startup')
    try:
     with socket.create_connection(('127.0.0.1',port),timeout=.2): break
    except OSError:
     if time.monotonic()>deadline: raise
     time.sleep(.05)
   env=dict(os.environ,INSPIRUM_SSH_FIXTURE=str(d))
   cmd=['cargo','test','--locked','--test','ssh_integration','--','--ignored','--nocapture','--test-threads=1']
   print('RUN:',' '.join(cmd),flush=True)
   print(f'Isolated unprivileged sshd: 127.0.0.1:{port}; keys removed on exit',flush=True)
   subprocess.run(cmd,env=env,check=True)
  finally:
   server.terminate()
   try: server.wait(timeout=5)
   except subprocess.TimeoutExpired: server.kill();server.wait()
   log.seek(0);print('--- disposable sshd log ---\n'+log.read(),flush=True)
PY
