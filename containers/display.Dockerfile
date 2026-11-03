# A virtual display, in a container, for running the EVE client with no monitor.
#
# Why this image exists at all: this host has the Vulkan loader, RADV, lavapipe
# and `wine`, but **no Xvfb and no weston** -- and the project's standing rule
# is that nothing is installed on the host. So the one missing piece, a display
# server with no monitor behind it, comes from here.
#
# Two ways to use it, both via tools/client-headless.sh:
#
#   1. `xvfb` -- the container runs ONLY Xvfb, publishes its socket into the
#      host's /tmp/.X11-unix, and the client is launched on the host by
#      tools/run-client.sh inside the existing network-namespace jail with
#      DISPLAY pointed at it. X11 is a unix socket, so it crosses the jail
#      boundary untouched (docs/jail.md says so explicitly) and the jail is
#      preserved exactly as it is today.
#
#   2. `client` -- the whole client runs inside the container under the host's
#      Proton, with `--network none`. No interface at all is a strictly
#      stronger isolation than the jail's veth, but the client then cannot
#      reach our server either, so this mode is a launch/render smoke test.
#
# Build:  tools/client-headless.sh --build     (the only networked step)
# Run:    everything else runs with --network none.
#
# The base image is pinned by digest (V6 F12): the ubuntu:26.04 the image was
# already built on, the same digest build.Dockerfile uses. Bump it deliberately.
FROM ubuntu@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7

ENV DEBIAN_FRONTEND=noninteractive

# Build-time only. Containers run with --network none.
#
# Three groups:
#   * the display itself and the tools that prove it is there (xdpyinfo,
#     xlsclients, xwd) and that capture from it (imagemagick's `import`);
#   * the graphics stack the client's DX11 -> DXVK -> Vulkan chain lands on:
#     mesa-vulkan-drivers carries both RADV (for the passed-through GPU) and
#     lavapipe (lvp, software), vulkan-tools carries vkcube/vulkaninfo which
#     are our stand-in oracle for "can anything present to this display";
#   * `wine` for step 1 -- proving a *Windows* GUI program can run headless
#     here before 20 GB of client is involved -- and python3 + the libraries
#     Proton's own launcher script needs for step 2.
RUN dpkg --add-architecture i386 \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      xvfb xauth x11-utils x11-apps x11-xserver-utils \
      imagemagick \
      mesa-vulkan-drivers mesa-vulkan-drivers:i386 \
      libvulkan1 libvulkan1:i386 \
      vulkan-tools mesa-utils libgl1-mesa-dri libgl1-mesa-dri:i386 \
      python3 ca-certificates procps \
      libc6:i386 libstdc++6:i386 \
      libfreetype6 libfreetype6:i386 \
      libgnutls30t64 libasound2t64 libpulse0 \
      libxcomposite1 libxrandr2 libxcursor1 libxi6 libxinerama1 \
      libsdl2-2.0-0 libgbm1 \
      fonts-liberation \
 && rm -rf /var/lib/apt/lists/*

# Wine last and on its own layer: it is the biggest thing here, it is only
# needed by the step-1 self-test, and keeping it separate means a failure
# fetching it does not cost the rest of the image.
RUN apt-get update \
 && apt-get install -y --no-install-recommends wine wine64 \
 && rm -rf /var/lib/apt/lists/*

# ImageMagick's stock policy.xml on Ubuntu refuses several formats. We only
# ever write PNG from `import`, which is allowed, but be explicit about not
# needing the rest.
ENV XDG_RUNTIME_DIR=/tmp/xdg
WORKDIR /work
