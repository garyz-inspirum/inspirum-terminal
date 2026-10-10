#!/usr/bin/env bash
# Linux-only disposable loopback SSH fixtures; no production SSH config is touched.
# Core fixtures are unprivileged. Password/PAM MFA acceptance uses disposable OS users and
# a root sshd only when passwordless sudo is available (as on the Linux CI runner).
# Runs the SSH integration fixture tests only. Not Windows or macOS native execution.
set -euo pipefail
cd "$(dirname "$0")/.."
# Respect standard Cargo overrides; keep generated credentials outside the checkout.
export TERM=xterm-256color
python3 - <<'PY'
import atexit, getpass, os, pathlib, secrets, shutil, socket, subprocess, tempfile, time
default_tmp = str(pathlib.Path(os.environ['CARGO_TARGET_DIR']).resolve().parent) if os.environ.get('CARGO_TARGET_DIR') else tempfile.gettempdir()
root=pathlib.Path(os.environ.get('INSPIRUM_TEST_TMPDIR', default_tmp))
root.mkdir(parents=True, exist_ok=True)
sshd=shutil.which('sshd') or '/usr/sbin/sshd'
if not pathlib.Path(sshd).is_file(): raise SystemExit('ERROR: fixture requires sshd')
if shutil.which('tmux') is None: raise SystemExit('ERROR: fixture requires tmux')
with tempfile.TemporaryDirectory(prefix='ssh-fixture-',dir=root) as tmp:
 d=pathlib.Path(tmp);os.chmod(d,0o700)
 fixture_password='inspirum-fixture-password'
 privileged_auth=(shutil.which('sudo') is not None and subprocess.run(['sudo','-n','true'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode==0)
 created_users=[]
 auth_public_dir=None
 def cleanup_users():
  for user in reversed(created_users):
   subprocess.run(
    ['sudo','-n','userdel','-f',user],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
    check=False,
   )
  created_users.clear()
 def cleanup_public_assets():
  if auth_public_dir is not None:
   shutil.rmtree(auth_public_dir, ignore_errors=True)
 atexit.register(cleanup_users)
 atexit.register(cleanup_public_assets)
 for key in ('host','client','wrong-host','wrong-client'):
  subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(d/key)],check=True)
 subprocess.run(
  ['ssh-keygen','-q','-t','ed25519','-N','fixture-passphrase','-f',str(d/'encrypted-client')],
  check=True,
 )
 (d/'authorized_keys').write_text(
  (d/'client.pub').read_text()+(d/'encrypted-client.pub').read_text()
 )
 (d/'auth_remote.sh').write_text('''#!/bin/sh
stty -echo
printf 'FIXTURE_AUTHENTICATED\\n'
while IFS= read -r line; do
 case "$line" in
 exit) exit 0 ;;
 esac
done
''')
 os.chmod(d/'auth_remote.sh',0o755)
 (d/'remote.sh').write_text('''#!/bin/sh
stty -echo
printf '%s\\n' "$$" > "'''+str(d)+'''/remote.pid"
printf 'FIXTURE_AUTHENTICATED\\n'
if [ -n "${SSH_ORIGINAL_COMMAND:-}" ]; then
 printf 'REMOTE_COMMAND:%s\\n' "$SSH_ORIGINAL_COMMAND"
 # Allow the PTY reader to observe the marker before the short-lived
 # remote-command session closes (otherwise this test races on CI).
 sleep 1
 exit 0
fi
while IFS= read -r line; do
 case "$line" in
 echo:*) printf 'REMOTE_ECHO:%s\\n' "${line#echo:}" ;;
 x11-probe)
  if [ -z "${DISPLAY:-}" ]; then
   printf 'X11_NO_DISPLAY\\n'
  elif case "$DISPLAY" in localhost:*) true ;; *) false ;; esac && timeout 5 xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
   printf 'X11_FORWARDED\\n'
  else
   printf 'X11_UNUSABLE\\n'
  fi ;;
 agent-probe)
  if [ -z "${SSH_AUTH_SOCK:-}" ]; then
   printf 'AGENT_NO_SOCKET\\n'
  elif [ ! -S "$SSH_AUTH_SOCK" ]; then
   printf 'AGENT_SOCKET_UNAVAILABLE\\n'
  elif ssh-add -l >/dev/null 2>&1; then
   printf 'AGENT_FORWARDED\\n'
  else
   printf 'AGENT_SOCKET_UNUSABLE\\n'
  fi ;;
 size) printf 'REMOTE_SIZE:'; stty size ;;
 exit) exit 0 ;;
 esac
done
'''.replace('\\n','\n'))
 with socket.socket() as sock, socket.socket() as jump_sock, socket.socket() as sftp_sock, socket.socket() as tmux_sock, socket.socket() as password_sock, socket.socket() as mfa_sock:
  sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
  jump_sock.bind(('127.0.0.1',0));jump_port=jump_sock.getsockname()[1]
  sftp_sock.bind(('127.0.0.1',0));sftp_port=sftp_sock.getsockname()[1]
  tmux_sock.bind(('127.0.0.1',0));tmux_port=tmux_sock.getsockname()[1]
  password_sock.bind(('127.0.0.1',0));password_port=password_sock.getsockname()[1]
  mfa_sock.bind(('127.0.0.1',0));mfa_port=mfa_sock.getsockname()[1]
 all_ports={port,jump_port,sftp_port,tmux_port,password_port,mfa_port}
 assert min(all_ports)>1024 and len(all_ports)==6
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
Ciphers aes256-ctr
AllowUsers {getpass.getuser()}
AllowTcpForwarding yes
# Disposable loopback fixture only. Client profile opt-in is verified below.
AllowAgentForwarding yes
X11Forwarding yes
X11UseLocalhost yes
XAuthLocation /usr/bin/xauth
SetEnv XAUTHORITY={d}/remote-xauth
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
 (d/'tmux_sshd_config').write_text(f'''ListenAddress 127.0.0.1
Port {tmux_port}
HostKey {d}/host
PidFile {d}/tmux_sshd.pid
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
LogLevel VERBOSE
''')
 if privileged_auth:
  suffix=str(os.getpid())
  auth_public_dir=pathlib.Path(tempfile.mkdtemp(prefix='inspirum-auth-fixture-',dir='/tmp'))
  os.chmod(auth_public_dir,0o755)
  shutil.copyfile(d/'authorized_keys',auth_public_dir/'authorized_keys')
  os.chmod(auth_public_dir/'authorized_keys',0o644)
  shutil.copyfile(d/'auth_remote.sh',auth_public_dir/'auth_remote.sh')
  os.chmod(auth_public_dir/'auth_remote.sh',0o755)
  password_user=('inspw'+suffix)[-31:]
  mfa_user=('inspmfa'+suffix)[-31:]
  for user in (password_user,mfa_user):
   subprocess.run(['sudo','-n','useradd','-M','-s','/bin/sh',user],check=True)
   created_users.append(user)
   subprocess.run(
    ['sudo','-n','chpasswd'],
    input=f'{user}:{fixture_password}\n',
    text=True,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
    check=True,
   )
  (d/'password_sshd_config').write_text(f'''ListenAddress 127.0.0.1
Port {password_port}
HostKey {d}/host
PidFile {d}/password-sshd.pid
StrictModes no
UsePAM yes
PasswordAuthentication yes
KbdInteractiveAuthentication no
PubkeyAuthentication no
AuthenticationMethods password
AllowUsers {password_user}
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
PermitTTY yes
PrintMotd no
PrintLastLog no
ForceCommand /bin/sh {auth_public_dir}/auth_remote.sh
LogLevel VERBOSE
''')
  (d/'mfa_sshd_config').write_text(f'''ListenAddress 127.0.0.1
Port {mfa_port}
HostKey {d}/host
PidFile {d}/mfa-sshd.pid
AuthorizedKeysFile {auth_public_dir}/authorized_keys
StrictModes no
UsePAM yes
PasswordAuthentication no
KbdInteractiveAuthentication yes
PubkeyAuthentication yes
AuthenticationMethods publickey,keyboard-interactive:pam
AllowUsers {mfa_user}
AllowTcpForwarding no
AllowAgentForwarding no
X11Forwarding no
PermitTunnel no
PermitTTY yes
PrintMotd no
PrintLastLog no
ForceCommand /bin/sh {auth_public_dir}/auth_remote.sh
LogLevel VERBOSE
''')
 host_fields=(d/'host.pub').read_text().split()
 wrong_fields=(d/'wrong-host.pub').read_text().split()
 target_host=f'[127.0.0.1]:{port} {host_fields[0]} {host_fields[1]}\n'
 jump_host=f'[127.0.0.1]:{jump_port} {host_fields[0]} {host_fields[1]}\n'
 sftp_host=f'[127.0.0.1]:{sftp_port} {host_fields[0]} {host_fields[1]}\n'
 tmux_host=f'[127.0.0.1]:{tmux_port} {host_fields[0]} {host_fields[1]}\n'
 password_host=f'[127.0.0.1]:{password_port} {host_fields[0]} {host_fields[1]}\n'
 mfa_host=f'[127.0.0.1]:{mfa_port} {host_fields[0]} {host_fields[1]}\n'
 changed_target=f'[127.0.0.1]:{port} {wrong_fields[0]} {wrong_fields[1]}\n'
 (d/'known_hosts').write_text(target_host+jump_host+sftp_host+tmux_host+password_host+mfa_host)
 (d/'changed_known_hosts').write_text(changed_target+jump_host+sftp_host+tmux_host+password_host+mfa_host)
 for name,known in [('config','known_hosts'),('changed-config','changed_known_hosts')]:
  (d/name).write_text(f'''Host fixture-jump
 HostName 127.0.0.1
 Port {jump_port}
Host fixture-sftp
 HostName 127.0.0.1
 Port {sftp_port}
Host fixture-tmux
 HostName 127.0.0.1
 Port {tmux_port}
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
 # Default fixtures explicitly disable IdentityAgent to keep authentication
 # deterministic. Agent-forwarding acceptance uses an independent fixture
 # config which permits the synthetic, local SSH_AUTH_SOCK for forwarding.
 (d/'agent-forward-config').write_text(
  (d/'config').read_text().replace(' IdentityAgent none\n','')
 )
 (d/'encrypted-config').write_text(f'''Host *
 HostName 127.0.0.1
 Port {port}
 User {getpass.getuser()}
 IdentityFile {d}/encrypted-client
 IdentitiesOnly yes
 IdentityAgent none
 UserKnownHostsFile {d}/known_hosts
 GlobalKnownHostsFile /dev/null
 BatchMode no
 ConnectTimeout 3
 UpdateHostKeys no
 ControlMaster no
 ControlPath none
 ProxyCommand none
''')
 (d/'agent-config').write_text(f'''Host *
 HostName 127.0.0.1
 Port {port}
 User {getpass.getuser()}
 IdentityFile {d}/wrong-client
 IdentitiesOnly no
 UserKnownHostsFile {d}/known_hosts
 GlobalKnownHostsFile /dev/null
 BatchMode yes
 ConnectTimeout 3
 UpdateHostKeys no
 ControlMaster no
 ControlPath none
 ProxyCommand none
''')
 if privileged_auth:
  (d/'password-config').write_text(f'''Host *
 HostName 127.0.0.1
 Port {password_port}
 User {password_user}
 PubkeyAuthentication no
 PasswordAuthentication yes
 KbdInteractiveAuthentication no
 PreferredAuthentications password
 IdentityAgent none
 UserKnownHostsFile {d}/known_hosts
 GlobalKnownHostsFile /dev/null
 BatchMode no
 NumberOfPasswordPrompts 1
 ConnectTimeout 3
 UpdateHostKeys no
 ControlMaster no
 ControlPath none
 ProxyCommand none
''')
  (d/'mfa-config').write_text(f'''Host *
 HostName 127.0.0.1
 Port {mfa_port}
 User {mfa_user}
 IdentityFile {d}/client
 IdentitiesOnly yes
 IdentityAgent none
 PubkeyAuthentication yes
 PasswordAuthentication no
 KbdInteractiveAuthentication yes
 PreferredAuthentications publickey,keyboard-interactive
 UserKnownHostsFile {d}/known_hosts
 GlobalKnownHostsFile /dev/null
 BatchMode no
 NumberOfPasswordPrompts 1
 ConnectTimeout 3
 UpdateHostKeys no
 ControlMaster no
 ControlPath none
 ProxyCommand none
''')
 subprocess.run([sshd,'-t','-f',str(d/'sshd_config')],check=True)
 subprocess.run([sshd,'-t','-f',str(d/'jump_sshd_config')],check=True)
 subprocess.run([sshd,'-t','-f',str(d/'sftp_sshd_config')],check=True)
 subprocess.run([sshd,'-t','-f',str(d/'tmux_sshd_config')],check=True)
 if privileged_auth:
  subprocess.run(['sudo','-n',sshd,'-t','-f',str(d/'password_sshd_config')],check=True)
  subprocess.run(['sudo','-n',sshd,'-t','-f',str(d/'mfa_sshd_config')],check=True)
 def wait_ready(server,listen_port,label):
  deadline=time.monotonic()+5
  while True:
   if server.poll() is not None: raise RuntimeError(f'{label} sshd exited at startup')
   try:
    with socket.create_connection(('127.0.0.1',listen_port),timeout=.2): return
   except OSError:
    if time.monotonic()>deadline: raise
    time.sleep(.05)
 with (d/'sshd.log').open('w+') as log, (d/'jump_sshd.log').open('w+') as jump_log, (d/'sftp_sshd.log').open('w+') as sftp_log, (d/'tmux_sshd.log').open('w+') as tmux_log, (d/'password_sshd.log').open('w+') as password_log, (d/'mfa_sshd.log').open('w+') as mfa_log:
  processes=[]
  server=jump_server=sftp_server=tmux_server=password_server=mfa_server=agent=xvfb=None
  try:
   # A real local X11 display and auth cookie are required to prove that
   # the remote client's proxied DISPLAY works, rather than only checking
   # the OpenSSH CLI arguments. Both auth files remain inside this fixture.
   local_xauth=d/'local-xauth'
   remote_xauth=d/'remote-xauth'
   display_number=next((
    value for value in range(150, 210)
    if not pathlib.Path(f'/tmp/.X11-unix/X{value}').exists()
    and not pathlib.Path(f'/tmp/.X{value}-lock').exists()
   ), None)
   if display_number is None: raise RuntimeError('no free isolated Xvfb display number')
   display=f':{display_number}'
   cookie=secrets.token_hex(16)
   subprocess.run(
    ['xauth','-f',str(local_xauth),'add',display,'.',cookie],
    stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True
   )
   xvfb=subprocess.Popen(
    ['Xvfb',display,'-screen','0','1024x768x24','-nolisten','tcp','-auth',str(local_xauth)],
    stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL
   )
   processes.append(xvfb)
   deadline=time.monotonic()+6
   while not pathlib.Path(f'/tmp/.X11-unix/X{display_number}').exists():
    if xvfb.poll() is not None: raise RuntimeError('isolated Xvfb exited before display ready')
    if time.monotonic()>deadline: raise RuntimeError('isolated Xvfb display not ready')
    time.sleep(.05)
   server=subprocess.Popen([sshd,'-D','-e','-f',str(d/'sshd_config')],stdout=log,stderr=log)
   processes.append(server)
   jump_server=subprocess.Popen([sshd,'-D','-e','-f',str(d/'jump_sshd_config')],stdout=jump_log,stderr=jump_log)
   processes.append(jump_server)
   sftp_server=subprocess.Popen([sshd,'-D','-e','-f',str(d/'sftp_sshd_config')],stdout=sftp_log,stderr=sftp_log)
   processes.append(sftp_server)
   tmux_server=subprocess.Popen([sshd,'-D','-e','-f',str(d/'tmux_sshd_config')],stdout=tmux_log,stderr=tmux_log)
   processes.append(tmux_server)
   if privileged_auth:
    password_server=subprocess.Popen(['sudo','-n',sshd,'-D','-e','-f',str(d/'password_sshd_config')],stdout=password_log,stderr=password_log)
    processes.append(password_server)
    mfa_server=subprocess.Popen(['sudo','-n',sshd,'-D','-e','-f',str(d/'mfa_sshd_config')],stdout=mfa_log,stderr=mfa_log)
    processes.append(mfa_server)
   agent_sock=d/'agent.sock'
   agent=subprocess.Popen(
    ['ssh-agent','-D','-a',str(agent_sock)],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
   )
   processes.append(agent)
   wait_ready(server,port,'target')
   wait_ready(jump_server,jump_port,'jump')
   wait_ready(sftp_server,sftp_port,'sftp')
   wait_ready(tmux_server,tmux_port,'tmux')
   if privileged_auth:
    wait_ready(password_server,password_port,'password')
    wait_ready(mfa_server,mfa_port,'mfa')
   deadline=time.monotonic()+5
   while not agent_sock.exists():
    if agent.poll() is not None: raise RuntimeError('ssh-agent exited at startup')
    if time.monotonic()>deadline: raise RuntimeError('ssh-agent socket did not become ready')
    time.sleep(.05)
   agent_env=dict(os.environ,SSH_AUTH_SOCK=str(agent_sock))
   subprocess.run(
    ['ssh-add',str(d/'client')],
    env=agent_env,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
    check=True,
   )
   env=dict(
    os.environ,
    INSPIRUM_SSH_FIXTURE=str(d),
    SSH_AUTH_SOCK=str(agent_sock),
    DISPLAY=display,
    XAUTHORITY=str(local_xauth),
    INSPIRUM_PRIV_AUTH_FIXTURE='1' if privileged_auth else '0',
    INSPIRUM_FIXTURE_PASSWORD=fixture_password,
   )
   # Independent OpenSSH control with the same synthetic agent and sshd.
   # If it fails, the fixture is broken; do not blame the application adapter.
   baseline=subprocess.run(
    ['ssh','-A','-F',str(d/'agent-forward-config'),'-tt','127.0.0.1'],
    env=env,
    input='agent-probe\\nexit\\n'.replace('\\n','\n'),
    capture_output=True,
    text=True,
    timeout=20,
    check=False,
   )
   if baseline.returncode != 0 or 'AGENT_FORWARDED' not in baseline.stdout:
    marker=next((key for key in ('AGENT_NO_SOCKET','AGENT_SOCKET_UNAVAILABLE','AGENT_SOCKET_UNUSABLE')
                 if key in baseline.stdout), 'NO_MARKER')
    raise RuntimeError(f'isolated system OpenSSH -A baseline failed: {marker}')
   print('PASS isolated system OpenSSH -A forwarding baseline',flush=True)
   x11_control=subprocess.run(
    ['ssh','-X','-F',str(d/'config'),'-tt','127.0.0.1'],
    env=env,
    input='x11-probe\\nexit\\n'.replace('\\n','\n'),
    capture_output=True,text=True,timeout=22,check=False,
   )
   if x11_control.returncode != 0 or 'X11_FORWARDED' not in x11_control.stdout:
    marker=next((key for key in ('X11_NO_DISPLAY','X11_UNUSABLE')
                 if key in x11_control.stdout), 'NO_MARKER')
    raise RuntimeError(f'isolated system OpenSSH -X baseline failed: {marker}')
   print('PASS isolated system OpenSSH -X forwarding baseline',flush=True)
   cmd=['cargo','test','--locked','--test','ssh_integration','--test','sftp_policy','--','--ignored','--nocapture','--test-threads=1']
   print('RUN:',' '.join(cmd),flush=True)
   auth_summary=(f'; password sshd: 127.0.0.1:{password_port}; MFA sshd: 127.0.0.1:{mfa_port}' if privileged_auth else '; password/MFA fixture skipped (passwordless sudo unavailable)')
   print(f'Isolated target sshd: 127.0.0.1:{port}; jump sshd: 127.0.0.1:{jump_port}; sftp sshd: 127.0.0.1:{sftp_port}; tmux sshd: 127.0.0.1:{tmux_port}{auth_summary}; credentials removed on exit',flush=True)
   subprocess.run(cmd,env=env,check=True)
  finally:
   for process in reversed(processes):
    if process.poll() is None:
     process.terminate()
   for process in reversed(processes):
    try: process.wait(timeout=5)
    except subprocess.TimeoutExpired: process.kill();process.wait()
   cleanup_users()
   cleanup_public_assets()
   log.seek(0);print('--- disposable target sshd log ---\n'+log.read(),flush=True)
   jump_log.seek(0);print('--- disposable jump sshd log ---\n'+jump_log.read(),flush=True)
   sftp_log.seek(0);print('--- disposable sftp sshd log ---\n'+sftp_log.read(),flush=True)
   tmux_log.seek(0);print('--- disposable tmux sshd log ---\n'+tmux_log.read(),flush=True)
   if privileged_auth:
    password_log.seek(0);print('--- disposable password sshd log ---\n'+password_log.read(),flush=True)
    mfa_log.seek(0);print('--- disposable MFA sshd log ---\n'+mfa_log.read(),flush=True)
PY
