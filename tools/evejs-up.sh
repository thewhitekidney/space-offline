#!/usr/bin/env bash
# Bring up a containerised EvEJS for black-box protocol observation.
#
# Why this exists and not `docker compose up`: the host has docker-compose
# v2.11.2 standalone, which speaks Docker API 1.42 while the daemon requires
# >= 1.44, and installing the compose plugin would break the project's
# no-local-installs rule. So this is their compose.yaml translated to plain
# `docker run`. Keep the two in sync by hand; `compose.yaml.orig` next to the
# extracted archive is the reference.
#
# This script is OURS and deliberately lives in our tree, not inside the
# extracted archive (untrusted data stays in its own directory).
#
# See docs/evejs/evejs-run-safely.md for the security assessment and the four
# hardening changes, three of which are baked into the extracted Dockerfile
# and compose.yaml already.
set -euo pipefail

ROOT=${EVEJS_ROOT:-$HOME/evejs-run}
IMAGE=${EVEJS_IMAGE:-evejs-local}
NET=evejs-net
# Where the game ports are published. 127.0.0.1 is right for a host-side probe;
# set EVEJS_BIND=10.137.0.1 to expose them to the jailed client, whose own
# 127.0.0.1 is its namespace loopback and reaches nothing here. The jail must
# already be up when binding to 10.137.0.1 -- docker cannot publish on an
# address that does not exist yet:
#   sudo EVE_JAIL_PORTS=26000,26001,26002 tools/jail-setup.sh
# (port 53 is allowed by the jail separately, for the stub resolver).
BIND=${EVEJS_BIND:-127.0.0.1}
# buildx/compose state in ~/.docker is root-owned on this host (a 2022 sudo
# accident), so keep docker's config somewhere writable instead of chowning
# the user's home.
export DOCKER_CONFIG=${DOCKER_CONFIG:-$ROOT/.dockercfg}
mkdir -p "$DOCKER_CONFIG"

[[ -d $ROOT ]] || { echo "no such directory: $ROOT" >&2; exit 1; }

say() { printf '\n== %s\n' "$*"; }

say "network and volumes"
docker network inspect "$NET"      >/dev/null 2>&1 || docker network create "$NET"
docker volume  inspect evejs-data  >/dev/null 2>&1 || docker volume create evejs-data
docker volume  inspect evejs-logs  >/dev/null 2>&1 || docker volume create evejs-logs

# `init` downloads the official SDE for build 3396210 from
# developers.eveonline.com and builds the game database into evejs-data. First
# run only; it is idempotent and exits 0 when the database is already there.
say "init (downloads the SDE on first run; several minutes)"
docker rm -f evejs-init >/dev/null 2>&1 || true
docker run --name evejs-init --network "$NET" \
  -v evejs-data:/var/lib/evejs \
  "$IMAGE" init

# The market daemon refuses to start without a seeded market.sqlite and
# crash-loops with "Choose a seed engine explicitly before startup". compose
# leaves this to a human (market-tools is a `profiles: ["tools"]` service), so
# the translation has to do it. Three engines: v1 is synthetic and local, v2
# downloads an EVE Ref snapshot, v3 downloads a TQ snapshot and builds a Jita
# index. We take v1 -- nothing here is testing the market, and it is the only
# one that needs no further download.
say "market seed (first run only)"
if docker run --rm -v evejs-data:/var/lib/evejs --entrypoint sh "$IMAGE" \
     -c '[ -f /var/lib/evejs/market/market.sqlite ]'; then
  echo "market.sqlite already present, skipping seed"
else
  docker run --rm --name evejs-seed -v evejs-data:/var/lib/evejs \
    --entrypoint /usr/bin/tini "$IMAGE" \
    -g -- /app/docker/entrypoint.sh market-tools rebuild v1 --preset jita_new_caldari
fi

say "market daemon"
docker rm -f evejs-market >/dev/null 2>&1 || true
docker run -d --name evejs-market --network "$NET" --network-alias market \
  -v evejs-data:/var/lib/evejs \
  -p "127.0.0.1:40110:40110" \
  -p "127.0.0.1:40111:40111" \
  --restart unless-stopped \
  "$IMAGE" market

# Wait for the market health endpoint the compose healthcheck used; the game
# server's own startup depends on it.
say "waiting for market health"
for i in $(seq 1 60); do
  if docker exec evejs-market curl -fsS http://127.0.0.1:40110/health >/dev/null 2>&1; then
    echo "market healthy after ${i}0s"; break
  fi
  sleep 10
  [[ $i -eq 60 ]] && { echo "market never became healthy" >&2; exit 1; }
done

say "game server (machoNet on ${BIND}:26000)"
docker rm -f evejs-server >/dev/null 2>&1 || true
docker run -d --name evejs-server --network "$NET" --network-alias evejs-server \
  -e EVEJS_GAMESTORE_DATA_DIR=/var/lib/evejs/gameStore/data \
  -e EVEJS_GAME_SERVER_BIND_HOST=0.0.0.0 \
  -e EVEJS_SERVER_PORT=26000 \
  -e EVEJS_IMAGE_SERVER_BIND_HOST=0.0.0.0 \
  -e EVEJS_IMAGE_SERVER_URL=http://127.0.0.1:26001/ \
  -e EVEJS_MARKET_DAEMON_HOST=market \
  -e EVEJS_MARKET_DAEMON_PORT=40111 \
  -e EVEJS_MICROSERVICES_BIND_HOST=0.0.0.0 \
  -e EVEJS_MICROSERVICES_PORT=26002 \
  -e EVEJS_MICROSERVICES_PUBLIC_URL=http://127.0.0.1:26002/ \
  -e EVEJS_PROXY_LOCAL_INTERCEPT=1 \
  -e EVEJS_PROXY_LOOPBACK_CDN_LISTEN_PORT=26003 \
  -e EVEJS_REDSHIFT_MONITOR_HOST=0.0.0.0 \
  -e EVEJS_REDSHIFT_MONITOR_PORT=26400 \
  -e EVEJS_XMPP_CONNECT_HOST=localhost \
  -e EVEJS_XMPP_SERVER_BIND_HOST=0.0.0.0 \
  -e EVEJS_WEB_GATEWAY_TOKEN= \
  -v evejs-data:/var/lib/evejs \
  -v evejs-logs:/var/lib/evejs/logs \
  -v "$ROOT/config:/app/config" \
  -v "$ROOT/_local/certs:/var/lib/evejs/certs" \
  -v "$ROOT/server/certs:/app/server/certs:ro" \
  -v "$ROOT/server/src/_secondary/express/certs:/app/server/src/_secondary/express/certs:ro" \
  -p "${BIND}:26000:26000" \
  -p "${BIND}:26001:26001" \
  -p "${BIND}:26002:26002" \
  -p "${BIND}:26400:26400" \
  --restart unless-stopped \
  "$IMAGE" server

# Deliberately NOT published: 443->26003 (loopback CDN, needs their CA in the
# Proton prefix) and 5222 (XMPP). Phase 2 of docs/evejs/evejs-run-safely.md is
# machoNet only -- an absent gateway costs tracebacks and blocks nothing.

say "up"
docker ps --filter name=evejs- --format '{{.Names}}\t{{.Status}}\t{{.Ports}}'
cat <<EOF

logs:    docker logs -f evejs-server
stop:    docker rm -f evejs-server evejs-market
re-init: docker volume rm evejs-data   (destroys the universe; init rebuilds)

Game port: ${BIND}:26000
EOF
