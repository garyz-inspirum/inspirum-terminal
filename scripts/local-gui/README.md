# Local GUI acceptance harness

Runs the Linux GUI acceptance checks (Xvfb + xdotool + ImageMagick + tesseract driving the real
release binary) in a container on any Linux docker host, instead of spending GitHub Actions minutes.

It mirrors CI; the single source of truth stays `.github/workflows/iced-design.yml` (step
`Smoke-test production Iced window`, whose `run:` block is extracted and executed verbatim) and
`scripts/test-iced-connected-gui.py` (the connected SSH acceptance). Only the dependencies live here.
Pinned Python packages: `asyncssh==2.24.0`, `Pillow==13.0.0`.

    cargo build --release --locked --features iced-ui
    docker build -t inspirum-gui-test -f scripts/local-gui/Dockerfile scripts/local-gui
    mkdir -p gui-out && docker run --rm --user $(id -u):$(id -g) -e HOME=/tmp/home --shm-size=512m \
      -v $PWD:/src:ro -v $PWD/target/release/inspirum-terminal:/app/inspirum-terminal:ro \
      -v $PWD/gui-out:/out inspirum-gui-test bash /src/scripts/local-gui/run-in-container.sh all

Modes: `smoke`, `connected`, `all` (= smoke + connected), `tour`. Artifacts (logs, screenshots) land in the `/out` mount.

CI runs the connected acceptance with `INSPIRUM_GUI_POPULATED=1 INSPIRUM_GUI_AUTOSTART=1
INSPIRUM_GUI_SAVED_SSH_CONFIG=1`: the existing CLI-config auto-start gate remains, followed
by Tools → Profiles Save, process quit, ordinary relaunch without `--ssh-config`, fixture
TCP echo/refusal, Cancel/relaunch and Clear/relaunch. The saved-path gate requires the
production field label `SSH config file for tunnel auto-start`, a text input immediately
below it, Save/Clear/Cancel controls, and `ssh_config_path` in `ui-state.json` beside the
profiles file. It is not a separate CI pass.
`connected` here runs without those flags unless passed: add `-e INSPIRUM_GUI_POPULATED=1
-e INSPIRUM_GUI_AUTOSTART=1 -e INSPIRUM_GUI_SAVED_SSH_CONFIG=1` to `docker run` to mirror
CI. The saved-path mode requires populated auto-start and cannot be combined with `tour`.

`tour` (`it-remote.sh gui tour`) re-runs the connected fixture with `INSPIRUM_GUI_TOUR=1` and the default LIGHT theme,
writing best-effort screenshots `tour-01-start.png` … `tour-14-tools.png` (no `tour-09`: the Explorer tab was merged into Filer) to `/out/tour` (log: `/out/tour.log`).
Only the window and a real SSH session are asserted; other steps warn and continue (fixed 1280x800 coordinates).
