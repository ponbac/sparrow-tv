#!/usr/bin/env bash

set -euo pipefail
umask 077

if [[ $# -lt 3 || $# -gt 4 ]]; then
  echo "usage: $0 <image@sha256:digest> <revision> <verified-manifest> [environment-file]" >&2
  exit 2
fi

readonly image="$1"
readonly expected_revision="$2"
readonly expected_manifest="$3"
readonly environment_file="${4:-.env.local}"
readonly rehearsal_password="${SPARROW_REHEARSAL_PASSWORD:-}"
readonly container_name="sparrow-private-rehearsal-$$"
readonly rollback_image="docker.io/ponbac/sparrow@sha256:96ac1b8e3fe6f25bc912a62a4b457be4fd553bc9a6e72db6fc6dffde2e8ff30f"

if [[ "$image" != *@sha256:* ]]; then
  echo "the rehearsal requires an immutable image digest" >&2
  exit 2
fi
if [[ ! "$expected_revision" =~ ^[0-9a-f]{40}$ ]]; then
  echo "the expected image revision is invalid" >&2
  exit 2
fi
if [[ ! "$expected_manifest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "the verified manifest is invalid" >&2
  exit 2
fi
readonly image_manifest="${image##*@}"
if [[ "$image_manifest" != "$expected_manifest" ]]; then
  echo "the rehearsal image does not match the reproduced manifest" >&2
  exit 2
fi
if [[ ! -f "$environment_file" ]]; then
  echo "the rehearsal environment file is unavailable" >&2
  exit 2
fi
if [[ -z "$rehearsal_password" ]]; then
  echo "SPARROW_REHEARSAL_PASSWORD is required" >&2
  exit 2
fi
if [[ "$rehearsal_password" == *$'\n'* || "$rehearsal_password" == *$'\r'* ]]; then
  echo "SPARROW_REHEARSAL_PASSWORD cannot contain line breaks" >&2
  exit 2
fi

rehearsal_root="$(mktemp -d)"
readonly rehearsal_root
readonly curl_config="$rehearsal_root/curl.conf"
readonly wrong_curl_config="$rehearsal_root/wrong-curl.conf"
escaped_password="${rehearsal_password//\\/\\\\}"
escaped_password="${escaped_password//\"/\\\"}"
readonly escaped_password
printf 'user = "sparrow:%s"\n' "$escaped_password" > "$curl_config"
printf 'user = "sparrow:wrong-rehearsal-credential"\n' > "$wrong_curl_config"

authenticated_curl() {
  curl --config "$curl_config" "$@"
}

container_created=false
cleanup() {
  if [[ "$container_created" == true ]]; then
    docker rm --force "$container_name" >/dev/null 2>&1 || true
  fi
  rm -rf -- "$rehearsal_root"
}
trap cleanup EXIT

if ! docker image inspect "$image" >/dev/null 2>&1; then
  docker pull "$image" >/dev/null
fi
image_revision="$(
  docker image inspect "$image" \
    --format '{{index .Config.Labels "org.opencontainers.image.revision"}}'
)"
readonly image_revision
if [[ "$image_revision" != "$expected_revision" ]]; then
  echo "the rehearsal image revision does not match the reproduced commit" >&2
  exit 1
fi

PASSWORD="$rehearsal_password" docker run --detach \
  --name "$container_name" \
  --env-file "$environment_file" \
  --env PASSWORD \
  --publish 127.0.0.1::33733 \
  --read-only \
  --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m \
  --cap-drop ALL \
  --security-opt no-new-privileges \
  --pids-limit 256 \
  "$image" >/dev/null
container_created=true

published_address="$(docker port "$container_name" 33733/tcp)"
readonly published_address
readonly host_port="${published_address##*:}"
readonly base_url="http://127.0.0.1:$host_port"
readonly startup_started="$SECONDS"
readonly startup_deadline="$((SECONDS + 720))"

while ! curl --silent --fail --max-time 2 "$base_url/health" >/dev/null; do
  if (( SECONDS >= startup_deadline )); then
    echo "the private candidate did not become healthy before the startup deadline" >&2
    exit 1
  fi
  if [[ "$(docker inspect --format '{{.State.Running}}' "$container_name")" != true ]]; then
    echo "the private candidate stopped before becoming healthy" >&2
    exit 1
  fi
  sleep 2
done

health_body="$(curl --silent --fail --max-time 2 "$base_url/health")"
readonly health_body
if [[ "$health_body" != '{"status":"ok"}' ]]; then
  echo "the health response did not match the public contract" >&2
  exit 1
fi
if ! docker exec "$container_name" /usr/local/bin/sparrow-server --healthcheck; then
  echo "the image self-healthcheck did not accept the live server" >&2
  exit 1
fi
readonly startup_seconds="$((SECONDS - startup_started))"

unauthenticated_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 "$base_url/app/"
)"
readonly unauthenticated_status
wrong_authenticated_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 \
    --config "$wrong_curl_config" \
    "$base_url/app/"
)"
readonly wrong_authenticated_status
authenticated_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 \
    --config "$curl_config" \
    "$base_url/app/"
)"
readonly authenticated_status

if [[ "$unauthenticated_status" != 401 || "$wrong_authenticated_status" != 401 ]]; then
  echo "the hosted UI did not reject missing and wrong authentication" >&2
  exit 1
fi
if [[ "$authenticated_status" != 200 ]]; then
  echo "the authenticated hosted UI did not answer successfully" >&2
  exit 1
fi

unauthenticated_api_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 \
    "$base_url/api/v1/status"
)"
readonly unauthenticated_api_status
wrong_authenticated_api_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 \
    --config "$wrong_curl_config" \
    "$base_url/api/v1/status"
)"
readonly wrong_authenticated_api_status
if [[ "$unauthenticated_api_status" != 401 || "$wrong_authenticated_api_status" != 401 ]]; then
  echo "the hosted API did not reject missing and wrong authentication" >&2
  exit 1
fi

authenticated_curl --silent --fail --max-time 10 \
  --output "$rehearsal_root/status.json" \
  "$base_url/api/v1/status"
jq -e '.generation != null and (.m3u | type == "object")' \
  "$rehearsal_root/status.json" >/dev/null

authenticated_curl --silent --fail --max-time 10 \
  --output "$rehearsal_root/groups.json" \
  "$base_url/api/v1/groups?limit=1"
jq -e '(.items | type == "array") and (.items | length > 0)' \
  "$rehearsal_root/groups.json" >/dev/null

authenticated_curl --silent --fail --max-time 10 \
  --output "$rehearsal_root/channels.json" \
  "$base_url/api/v1/channels?limit=5"
jq -e '(.items | type == "array") and (.items | length > 0)' \
  "$rehearsal_root/channels.json" >/dev/null

authenticated_curl --silent --fail --max-time 30 \
  --output "$rehearsal_root/search.json" \
  "$base_url/api/v1/search?term=sport&channelLimit=5&programmeLimit=5"
jq -e \
  '(.channels.items | type == "array") and (.programmes.items | type == "array") and (.programmes.items | length > 0)' \
  "$rehearsal_root/search.json" >/dev/null

channel_id="$(
  jq -er '.programmes.items[0].channelId // .channels.items[0].id // empty' \
    "$rehearsal_root/search.json" 2>/dev/null \
    || jq -er '.items[0].id' "$rehearsal_root/channels.json"
)"
readonly channel_id
if [[ ! "$channel_id" =~ ^[A-Za-z0-9_-]+$ ]]; then
  echo "the candidate returned an invalid Channel Identifier" >&2
  exit 1
fi

jq -er '[.programmes.items[].channelId, .channels.items[].id] | .[]' \
  "$rehearsal_root/search.json" > "$rehearsal_root/playback-candidates.txt"
jq -er '.items[].id' "$rehearsal_root/channels.json" \
  >> "$rehearsal_root/playback-candidates.txt"
awk '!seen[$0]++ { print; if (++count == 10) exit }' \
  "$rehearsal_root/playback-candidates.txt" \
  > "$rehearsal_root/playback-candidates-unique.txt"
if [[ ! -s "$rehearsal_root/playback-candidates-unique.txt" ]]; then
  echo "the candidate returned no playback candidates" >&2
  exit 1
fi
while IFS= read -r playback_candidate; do
  if [[ ! "$playback_candidate" =~ ^[A-Za-z0-9_-]+$ ]]; then
    echo "the candidate returned an invalid playback Channel Identifier" >&2
    exit 1
  fi
done < "$rehearsal_root/playback-candidates-unique.txt"

authenticated_curl --silent --fail --max-time 30 \
  --output "$rehearsal_root/schedule.json" \
  "$base_url/api/v1/channels/$channel_id/schedule?limit=5"
jq -e '(.items | type == "array") and (.items | length > 0)' \
  "$rehearsal_root/schedule.json" >/dev/null

authenticated_curl --silent --fail --max-time 720 \
  --request POST \
  --header 'X-Sparrow-Request: refresh' \
  --data '' \
  --output "$rehearsal_root/refresh.json" \
  "$base_url/api/v1/refresh"
jq -e '.trigger == "manual" and (.status.generation != null)' \
  "$rehearsal_root/refresh.json" >/dev/null
jq -e '
  def succeeded:
    ._tag == "updated"
    or ._tag == "not-modified"
    or (._tag == "skipped" and .reason == "fresh");
  (.m3u | succeeded) and (.epg == null or (.epg | succeeded))
' "$rehearsal_root/refresh.json" >/dev/null

unauthenticated_playback_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 \
    "$base_url/api/v1/play/$channel_id"
)"
readonly unauthenticated_playback_status
wrong_authenticated_playback_status="$(
  curl --silent --output /dev/null --write-out '%{http_code}' --max-time 5 \
    --config "$wrong_curl_config" \
    "$base_url/api/v1/play/$channel_id"
)"
readonly wrong_authenticated_playback_status
if [[ "$unauthenticated_playback_status" != 401 || "$wrong_authenticated_playback_status" != 401 ]]; then
  echo "the hosted playback route did not reject missing and wrong authentication" >&2
  exit 1
fi

playback_verified=false
while IFS= read -r playback_candidate; do
  rm -f -- "$rehearsal_root/playback.bin"
  set +e
  authenticated_curl --silent --fail --max-time 15 \
    --output "$rehearsal_root/playback.bin" \
    "$base_url/api/v1/play/$playback_candidate"
  playback_exit=$?
  set -e
  if [[ ("$playback_exit" -eq 0 || "$playback_exit" -eq 28) \
    && -s "$rehearsal_root/playback.bin" ]]; then
    playback_verified=true
    break
  fi
done < "$rehearsal_root/playback-candidates-unique.txt"
if [[ "$playback_verified" != true ]]; then
  echo "the candidate playback requests returned no media bytes" >&2
  exit 1
fi

if [[ -n "$(docker logs "$container_name" 2>&1)" ]]; then
  echo "the exercised candidate emitted unexpected logs" >&2
  exit 1
fi

printf 'image=%s\n' "$image"
printf 'revision=%s\n' "$image_revision"
printf 'manifest=%s\n' "$image_manifest"
printf 'rollback=%s\n' "$rollback_image"
printf 'health=ok\n'
printf 'staticUi=ok\n'
printf 'authenticatedApi=ok\n'
printf 'apiBrowse=ok\n'
printf 'apiSearch=ok\n'
printf 'apiGuide=ok\n'
printf 'apiRefresh=ok\n'
printf 'apiPlaybackBytes=ok\n'
printf 'exercisedLogs=empty\n'
printf 'startupSeconds=%s\n' "$startup_seconds"
