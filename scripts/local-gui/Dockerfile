# Local GUI acceptance image: mirrors the Linux GUI steps of .github/workflows/iced-design.yml.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8 LC_ALL=C.UTF-8
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      xvfb xdotool x11-utils imagemagick fonts-dejavu-core tesseract-ocr tesseract-ocr-eng xclip \
      python3 python3-pip python3-venv python3-yaml openssh-client ca-certificates \
      libxkbcommon0 libxkbcommon-x11-0 libfontconfig1 libfreetype6 libx11-6 libx11-xcb1 libxcb1 \
      libxcursor1 libxi6 libxrandr2 libgl1 libegl1 libvulkan1 mesa-vulkan-drivers libwayland-client0 \
 && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir --break-system-packages asyncssh==2.24.0 Pillow==13.0.0
# Non-root default user; override the uid at run time with
#   docker run --user $(id -u):$(id -g) -e HOME=/tmp/home ...
# so files written to bind mounts belong to the invoking host user.
RUN useradd -m -u 1500 gui && mkdir -p /work /out /app && chmod 1777 /work /out
# Throwaway test image: let an arbitrary --user uid register itself (OpenSSH needs a passwd entry).
RUN chmod a+w /etc/passwd
USER gui
ENV HOME=/tmp/home
WORKDIR /work
