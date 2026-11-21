# Local GUI acceptance harness

Runs the Linux GUI acceptance checks (Xvfb + xdotool + ImageMagick + tesseract driving the real
release binary) in a container on any Linux docker host, instead of spending GitHub Actions minutes.

It mirrors CI; the single source of truth stays `.github/workflows/iced-design.yml` (step
`Smoke-test production Iced window`, whose `run:` block is extracted and executed verbatim) and
`scripts/test-iced-connected-gui.py` (the connected SSH acceptance). Only the dependencies live here.

    cargo build --release --locked --features iced-ui
    docker build -t inspirum-gui-test -f scripts/local-gui/Dockerfile scripts/local-gui
    mkdir -p gui-out && docker run --rm --user $(id -u):$(id -g) -e HOME=/tmp/home --shm-size=512m \
      -v $PWD:/src:ro -v $PWD/target/release/inspirum-terminal:/app/inspirum-terminal:ro \
      -v $PWD/gui-out:/out inspirum-gui-test bash /src/scripts/local-gui/run-in-container.sh all

Modes: `smoke`, `connected`, `all` (= smoke + connected), `tour`. Artifacts (logs, screenshots) land in the `/out` mount.

`tour` (`it-remote.sh gui tour`) re-runs the connected fixture with `INSPIRUM_GUI_TOUR=1` and the default LIGHT theme,
writing best-effort screenshots `tour-01-start.png` … `tour-14-tools.png` to `/out/tour` (log: `/out/tour.log`).
Only the window and a real SSH session are asserted; other steps warn and continue (fixed 1280x800 coordinates).
