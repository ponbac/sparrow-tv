#!/usr/bin/env bash

set -euo pipefail
umask 077
bun_path="${SPARROW_PINNED_BUN:-}"
unset SPARROW_PINNED_BUN
[[ -n "$bun_path" ]] || bun_path="$(mise which bun)"
bun_path="$(readlink -f "$bun_path")"
[[ "$bun_path" == */.local/share/mise/installs/bun/1.4.0/bin/bun && "$(sha256sum "$bun_path" | cut -d' ' -f1)" == "33d56b070be6a9e3da0ab013038b43d1645d0534ca811ecdba4472599117eb4b" ]] || { echo "the pinned Bun runtime is unavailable or altered" >&2; exit 2; }
readonly bun_path
export PATH=/usr/bin:/bin

if [[ $# -ne 4 ]]; then
  echo "usage: $0 <replacement|legacy> <candidate|production|baseline|rollback> <base-url> <output>" >&2
  exit 2
fi

readonly mode="$1"
readonly role="$2"
readonly base_url="${3%/}"
readonly output="$4"
readonly resolve_ip="${SPARROW_HOSTED_RESOLVE_IP:-}"
unset SPARROW_HOSTED_RESOLVE_IP

if [[ ! "$base_url" =~ ^https?://[A-Za-z0-9.-]+(:[0-9]{1,5})?$ ]]; then
  echo "the hosted verifier requires an origin without credentials, path, query, or fragment" >&2
  exit 2
fi
if [[ "$role" == production || "$role" == *-production ]]; then
  [[ "$base_url" == "https://tv.ponbac.xyz" && -z "$resolve_ip" ]] || { echo "production verification requires the contract public origin without address overrides" >&2; exit 2; }
else
  [[ "$base_url" == "http://localhost:33733" ]] || { echo "isolated endpoint verification requires the fixed loopback origin" >&2; exit 2; }
  IFS=. read -r ip_a ip_b ip_c ip_d ip_extra <<< "$resolve_ip"
  for octet in "$ip_a" "$ip_b" "$ip_c" "$ip_d"; do
    [[ "$octet" =~ ^(0|[1-9][0-9]{0,2})$ && "$octet" -le 255 ]] || { echo "isolated endpoint verification requires the inspected Docker IPv4 address" >&2; exit 2; }
  done
  [[ -z "$ip_extra" ]] || { echo "isolated endpoint verification requires the inspected Docker IPv4 address" >&2; exit 2; }
fi
if [[ "$mode:$role" != "replacement:candidate" && "$mode:$role" != "replacement:production" \
  && "$mode:$role" != "legacy:baseline" && "$mode:$role" != "legacy:rollback" \
  && "$mode:$role" != "legacy:baseline-production" && "$mode:$role" != "legacy:rollback-production" ]]; then
  echo "the endpoint mode and evidence role are inconsistent" >&2
  exit 2
fi

work="$(mktemp -d)"
readonly work
cleanup() { rm -rf -- "$work"; }
trap cleanup EXIT

safe_curl() {
  local -a resolution=()
  [[ -z "$resolve_ip" ]] || resolution=(--resolve "localhost:33733:$resolve_ip")
  curl -q --proto '=http,https' --proto-redir '=https' --location --max-redirs 0 --noproxy '*' "${resolution[@]}" "$@"
}
status() {
  safe_curl --silent --output /dev/null --write-out '%{http_code}' --max-time 10 "$@"
}
require_200() {
  [[ "$(awk 'toupper($1) ~ /^HTTP\// { code=$2 } END { print code }' "$1")" == 200 ]]
}

if [[ "$mode" == legacy ]]; then
  [[ "$(status "$base_url/app/")" == 200 ]]
  safe_curl --silent --fail --max-time 30 --max-filesize 1048576 --dump-header "$work/search.headers" --get --data-urlencode 'q=a' \
    --output "$work/search.json" "$base_url/search"
  require_200 "$work/search.headers"
  grep -Eiq '^content-type:[[:space:]]*application/(json|[^;]+\+json)' "$work/search.headers"
  jq -e '(.programmes | type == "array") and (.channels | type == "array")' \
    "$work/search.json" >/dev/null
  gates='["legacy-ui-liveness","legacy-search-liveness"]'
else
  readonly password="${SPARROW_HOSTED_PASSWORD:-}"
  unset SPARROW_HOSTED_PASSWORD
  if [[ -z "$password" || "$password" == *$'\n'* || "$password" == *$'\r'* ]]; then
    echo "SPARROW_HOSTED_PASSWORD must be present without line breaks" >&2
    exit 2
  fi
  escaped="${password//\\/\\\\}"
  escaped="${escaped//\"/\\\"}"
  printf 'user = "sparrow:%s"\n' "$escaped" > "$work/auth.conf"
  printf 'user = "sparrow:wrong-verifier-credential"\n' > "$work/wrong.conf"
  auth_curl() { safe_curl --config "$work/auth.conf" "$@"; }

  safe_curl --silent --fail --max-time 10 --max-filesize 1024 --dump-header "$work/health.headers" --output "$work/health.json" "$base_url/health"
  require_200 "$work/health.headers"
  grep -Eiq '^content-type:[[:space:]]*application/(json|[^;]+\+json)' "$work/health.headers"
  [[ "$(<"$work/health.json")" == '{"status":"ok"}' ]]
  [[ "$(status "$base_url/app/")" == 401 ]]
  [[ "$(status --config "$work/wrong.conf" "$base_url/app/")" == 401 ]]
  [[ "$(status --config "$work/auth.conf" "$base_url/app/")" == 200 ]]
  [[ "$(status "$base_url/api/v1/status")" == 401 ]]
  [[ "$(status --config "$work/wrong.conf" "$base_url/api/v1/status")" == 401 ]]

  auth_curl --silent --fail --max-time 30 --max-filesize 1048576 --dump-header "$work/status.headers" --output "$work/status.json" "$base_url/api/v1/status"
  auth_curl --silent --fail --max-time 30 --max-filesize 1048576 --dump-header "$work/groups.headers" --output "$work/groups.json" "$base_url/api/v1/groups?limit=1"
  auth_curl --silent --fail --max-time 30 --max-filesize 4194304 --dump-header "$work/channels.headers" --output "$work/channels.json" "$base_url/api/v1/channels?limit=10"
  auth_curl --silent --fail --max-time 30 --max-filesize 4194304 --dump-header "$work/search.headers" --output "$work/search.json" \
    "$base_url/api/v1/search?term=a&channelLimit=10&programmeLimit=10"
  for headers in "$work/status.headers" "$work/groups.headers" "$work/channels.headers" "$work/search.headers"; do
    require_200 "$headers"
    grep -Eiq '^content-type:[[:space:]]*application/(json|[^;]+\+json)' "$headers"
  done
  jq -e '.generation != null and (.m3u | type == "object")' "$work/status.json" >/dev/null
  jq -e '(.items | type == "array") and (.items | length > 0)' "$work/groups.json" >/dev/null
  jq -e '(.items | type == "array") and (.items | length > 0)' "$work/channels.json" >/dev/null
  jq -e '(.channels.items | type == "array") and (.programmes.items | type == "array")' "$work/search.json" >/dev/null
  channel_id="$(jq -er '.programmes.items[0].channelId // .channels.items[0].id // empty' "$work/search.json" 2>/dev/null \
    || jq -er '.items[0].id' "$work/channels.json")"
  [[ "$channel_id" =~ ^[A-Za-z0-9_-]+$ ]]
  auth_curl --silent --fail --max-time 30 --max-filesize 4194304 --dump-header "$work/schedule.headers" --output "$work/schedule.json" \
    "$base_url/api/v1/channels/$channel_id/schedule?limit=10"
  require_200 "$work/schedule.headers"
  grep -Eiq '^content-type:[[:space:]]*application/(json|[^;]+\+json)' "$work/schedule.headers"
  jq -e '(.items | type == "array") and (.items | length > 0)' "$work/schedule.json" >/dev/null
  auth_curl --silent --fail --max-time 15 --max-filesize 8388608 --dump-header "$work/playback.headers" --output "$work/playback.bin" \
    "$base_url/api/v1/play/$channel_id" || [[ $? -eq 28 ]]
  require_200 "$work/playback.headers"
  [[ -s "$work/playback.bin" ]]
  grep -Eiq '^content-type:[[:space:]]*(video/mp2t|application/octet-stream)' "$work/playback.headers"
  jq -s -e '
    def forbidden:
      paths as $p
      | ($p[-1] | tostring | ascii_downcase)
      | test("(?i)(url|location|headers?|fingerprint|raw_?body)$");
    def unsafe_value: .. | strings | test("(?i)(https?://|[^[:space:]]+:[^[:space:]]+@|authorization|bearer[[:space:]]|password)");
    all(forbidden; not) and (any(unsafe_value) | not)
  ' "$work/status.json" "$work/groups.json" "$work/channels.json" \
    "$work/search.json" "$work/schedule.json" >/dev/null
  gates='["health","ui-authentication","api-authentication","refresh-state","browse","search","guide","playback-by-channel-id","privacy-projection"]'
fi

if [[ -e "$output" || -L "$output" ]]; then
  echo "the endpoint evidence output already exists" >&2
  exit 2
fi
jq -n \
  --arg recordedAt "$("$bun_path" -e 'process.stdout.write(new Date().toISOString())')" \
  --arg role "$role" \
  --arg targetOrigin "$base_url" \
  --argjson ids "$gates" \
  '{schemaVersion:1, recordedAt:$recordedAt, targetOrigin:$targetOrigin, role:$role, result:"passed", gates:[$ids[] | {id:.,result:"passed"}]}' \
  > "$work/evidence.json"
"$bun_path" app/scripts/hosted-cutover.ts record-endpoint --input "$work/evidence.json" --output "$output" >/dev/null
