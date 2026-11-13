#!/usr/bin/env bash
# Announces a planned Zigbee2MQTT restart to the guard, then recreates it.
#
# Usage: ./restart-z2m.sh [window_seconds]
#
# Why: the MQTT last-will and the container teardown look identical to a real
# outage. Telling the guard first is the only way it can tell them apart
# without Docker socket access.
set -euo pipefail

GUARD_URL="${Z2M_GUARD_URL:-http://172.22.1.51:8089}"
TOKEN="${Z2M_GUARD_HTTP_TOKEN:?set Z2M_GUARD_HTTP_TOKEN}"
WINDOW="${1:-180}"
COMPOSE_FILE="${COMPOSE_FILE:-$HOME/docker/home-assistent/docker-compose.yaml}"

echo "== announcing planned restart (window ${WINDOW}s)"
curl -fsS -X POST "$GUARD_URL/v1/restart-intent" \
  -H "x-guard-token: ${TOKEN}" \
  -H 'content-type: application/json' \
  -d "{\"reason\":\"script\",\"window_s\":${WINDOW}}"
echo

echo "== recreating zigbee2mqtt"
docker compose -f "$COMPOSE_FILE" up -d --force-recreate --no-deps zigbee2mqtt

echo "== done; watch: docker logs -f z2m-guard"