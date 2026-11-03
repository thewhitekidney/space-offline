# Analysis toolchain for the EVE client recon work.
#
# Nothing is installed on the host; this image is the only place tooling lives,
# and the only place network access happens (at build time).
#
# Build:  tools/analysis.sh --build
# Run:    tools/analysis.sh <command...>
# The base image is pinned by digest (V6 F12): this is the python:3.11-slim
# the image was already built on (`docker image inspect`, RepoDigests).
# Bump it deliberately.
FROM python@sha256:e88e9763f943ec1834f992a4b51e0f24500486803e8bc534e5767af9ea65f6ce

# Build-time only. Containers run with --network none.
RUN apt-get update && apt-get install -y --no-install-recommends \
        binutils \
        file \
        xxd \
    && rm -rf /var/lib/apt/lists/*

# uncompyle6 runs on Python 3 and decompiles 2.7 bytecode, which is what the
# client ships. xdis/spark-parser are its bytecode and grammar backends.
# Every package, transitive ones included, is pinned with its sha256 in
# analysis.requirements.txt; --require-hashes refuses anything else.
COPY analysis.requirements.txt /tmp/analysis.requirements.txt
RUN pip install --no-cache-dir --require-hashes \
        -r /tmp/analysis.requirements.txt \
 && rm /tmp/analysis.requirements.txt

WORKDIR /work
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
