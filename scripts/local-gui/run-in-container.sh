#!/usr/bin/env bash
# Runs INSIDE the inspirum-gui-test container.
#   /src  = source tree (read-only)   /app/inspirum-terminal = release binary   /out = artifacts
# Usage: run-in-container.sh smoke|connected|all|tour   (all = smoke + connected)
set -euo pipefail
mode=${1:-all}
export HOME=${HOME:-/tmp/home}
[ -w "$HOME" ] 2>/dev/null || export HOME=/tmp/home
mkdir -p "$HOME"
export XDG_RUNTIME_DIR=/tmp/xdg-runtime-$(id -u)
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
# OpenSSH refuses to run for a uid without a passwd entry; give it one if possible.
if ! getent passwd "$(id -u)" >/dev/null && [ -w /etc/passwd ]; then
  echo "agent:x:$(id -u):$(id -g)::$HOME:/bin/bash" >> /etc/passwd
fi

run_connected() {
  local w=/tmp/work-connected; rm -rf "$w"; mkdir -p "$w"; cd "$w"
  # Loaded build host: OCR is several times slower than on GitHub runners.
  export INSPIRUM_GUI_OCR_BUDGET=${INSPIRUM_GUI_OCR_BUDGET:-90}
  export INSPIRUM_GUI_TIME_SCALE=${INSPIRUM_GUI_TIME_SCALE:-4}
  set +e
  python3 -B /src/scripts/test-iced-connected-gui.py /app/inspirum-terminal /out/connected 2>&1 | tee /out/connected.log
  local rc=${PIPESTATUS[0]}
  set -e
  return "$rc"
}

run_tour() {
  local w=/tmp/work-tour; rm -rf "$w"; mkdir -p "$w"; cd "$w"
  export INSPIRUM_GUI_TIME_SCALE=${INSPIRUM_GUI_TIME_SCALE:-4}
  set +e
  INSPIRUM_GUI_TOUR=1 python3 -B /src/scripts/test-iced-connected-gui.py /app/inspirum-terminal /out/connected 2>&1 | tee /out/tour.log
  local rc=${PIPESTATUS[0]}
  set -e
  return "$rc"
}

run_smoke() {
  local w=/tmp/work-smoke; rm -rf "$w"; mkdir -p "$w/target/release" /tmp/runner; cd "$w"
  ln -s /app/inspirum-terminal target/release/inspirum-terminal
  ln -s /out preview-artifacts
  python3 - > /tmp/smoke-step.sh <<'PY'
import sys, yaml
wf = yaml.safe_load(open('/src/.github/workflows/iced-design.yml'))
for job in wf.get('jobs', {}).values():
    for step in job.get('steps', []) or []:
        if step.get('name') == 'Smoke-test production Iced window':
            sys.stdout.write(step['run']); sys.exit(0)
sys.exit("step 'Smoke-test production Iced window' not found")
PY
  set +e
  RUNNER_TEMP=/tmp/runner bash -eo pipefail /tmp/smoke-step.sh 2>&1 | tee /out/smoke.log
  local rc=${PIPESTATUS[0]}
  set -e
  return "$rc"
}

declare -A res
fail=0
case "$mode" in
  smoke|connected|tour) todo=("$mode") ;;
  all) todo=(smoke connected) ;;
  *) echo "usage: $0 smoke|connected|all|tour" >&2; exit 2 ;;
esac
for m in "${todo[@]}"; do
  if "run_$m"; then res[$m]=0; else res[$m]=$?; fail=1; fi
done
for m in "${todo[@]}"; do echo "RESULT $m rc=${res[$m]}"; done
exit "$fail"
