# Build and test image for the Rust server.
#
# This is the ONLY step that touches the network. Everything it installs comes
# from Ubuntu's signed apt archive: the compiler and (later) every Rust crate
# (librust-*-dev). Nothing is fetched from crates.io or GitHub, here or later.
# Code is then compiled by tools/dev.sh with networking disabled.
#
# Toolchain only for now: the marshal and protocol codecs are dependency-free.
# Add librust-*-dev lines (tokio for the server, flate2 for transport
# compression, a database driver for persistence) as the crates that need them
# arrive, and bump server/deps.allow + [workspace.dependencies] to match.
#
# The base image is pinned by digest; bump it deliberately.
FROM ubuntu@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates gcc libc6-dev \
      rustc cargo rust-clippy rustfmt \
 && rm -rf /var/lib/apt/lists/*

# Record exactly which crates the image carries, for review in a diff.
RUN ls /usr/share/cargo/registry | sort > /etc/eve-offline-crates.txt

# Record the toolchain too: apt's rustc/cargo are not version-pinned, so a
# rebuild can move them. The gate's `== toolchain` step prints this (V6 F12).
RUN { rustc -V; cargo -V; } > /etc/eve-offline-toolchain.txt

WORKDIR /repo/server
