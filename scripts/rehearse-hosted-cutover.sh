#!/usr/bin/env bash

set -euo pipefail
umask 077
bun_path="$(mise which bun)"
bun_path="$(readlink -f "$bun_path")"
[[ "$bun_path" == */.local/share/mise/installs/bun/1.4.0/bin/bun && "$(sha256sum "$bun_path" | cut -d' ' -f1)" == "33d56b070be6a9e3da0ab013038b43d1645d0534ca811ecdba4472599117eb4b" ]] || { echo "the pinned Bun runtime is unavailable or altered" >&2; exit 2; }
readonly bun_path
export PATH=/usr/bin:/bin
for ambient in DOCKER_HOST DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH DOCKER_CONFIG DOCKER_CLI_PLUGIN_EXTRA_DIRS; do
  [[ -z "${!ambient+x}" ]] || { echo "$ambient must be unset" >&2; exit 2; }
done
[[ "$(stat -Lc '%u:%a' /usr/bin/docker)" == "0:755" ]] || { echo "the system Docker client is untrusted" >&2; exit 2; }

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <plan> <evidence-key> <verified-output>" >&2
  exit 2
fi

readonly plan="$1"
readonly evidence_key="$2"
readonly verified_output="$3"
readonly password="${SPARROW_REHEARSAL_PASSWORD:-}"
unset SPARROW_REHEARSAL_PASSWORD

if [[ -z "$password" || "$password" == *$'\n'* || "$password" == *$'\r'* ]]; then
  echo "SPARROW_REHEARSAL_PASSWORD is required without line breaks" >&2
  exit 2
fi
for required_file in "$plan" "$evidence_key"; do
  [[ -f "$required_file" && ! -L "$required_file" ]] || {
    echo "a required rehearsal input is unavailable or symlinked" >&2
    exit 2
  }
done

context="$(/usr/bin/docker context show)"; readonly context
context_host="$(/usr/bin/docker context inspect "$context" --format '{{(index .Endpoints "docker").Host}}')"; readonly context_host
if [[ "$context_host" != unix://* ]]; then
  echo "the disposable rehearsal requires a local Unix-socket Docker context" >&2
  exit 2
fi
docker_cmd() { /usr/bin/docker --host "$context_host" "$@"; }

execution_plan="$("$bun_path" app/scripts/hosted-cutover.ts print-rehearsal-plan --plan "$plan")"; readonly execution_plan
baseline_image="$(jq -er '.rollbackImage' <<<"$execution_plan")"; readonly baseline_image
candidate_image="$(jq -er '.candidateImage' <<<"$execution_plan")"; readonly candidate_image
candidate_revision="$(jq -er '.candidateRevision' <<<"$execution_plan")"; readonly candidate_revision
fixture_image="$(jq -er '.fixtureImage' <<<"$execution_plan")"; readonly fixture_image
for bounded_image in "$baseline_image" "$candidate_image" "$fixture_image"; do
  if [[ "$bounded_image" == "$candidate_image" ]]; then docker_cmd image inspect "$bounded_image" >/dev/null
  else docker_cmd image inspect "$bounded_image" >/dev/null 2>&1 || docker_cmd pull "$bounded_image" >/dev/null; fi
  [[ "$(docker_cmd image inspect "$bounded_image" --format '{{json .Config.Volumes}}')" == null ]] || {
    echo "rehearsal images must not declare volumes" >&2
    exit 2
  }
done
readonly container_name="sparrow-cutover-rehearsal-$$"
work="$(mktemp -d)"; readonly work
readonly environment_file="$work/rehearsal.env"
owner="$(tr -d '\n' < /proc/sys/kernel/random/uuid)"; readonly owner
readonly network_name="sparrow-cutover-$owner"
cleanup() {
  while IFS= read -r owned_container; do
    [[ -n "$owned_container" ]] && docker_cmd rm --force --volumes "$owned_container" >/dev/null 2>&1 || true
  done < <(docker_cmd ps --all --quiet --filter "label=xyz.ponbac.sparrow.cutover=$owner" 2>/dev/null || true)
  docker_cmd network rm "$network_name" >/dev/null 2>&1 || true
  rm -rf -- "$work"
}
trap cleanup EXIT

docker_cmd network create --internal --label "xyz.ponbac.sparrow.cutover=$owner" "$network_name" >/dev/null
fixture_snapshot="$work/fixture.py"
"$bun_path" app/scripts/hosted-cutover.ts snapshot-rehearsal-fixture --output "$fixture_snapshot"
fixture_user="$(stat -Lc '%u:%g' "$fixture_snapshot")"
readonly fixture_user
[[ "$fixture_user" =~ ^[0-9]+:[0-9]+$ ]] || exit 1
docker_cmd run --detach --name "sparrow-fixture-$owner" --label "xyz.ponbac.sparrow.cutover=$owner" --network "$network_name" \
  --user "$fixture_user" --read-only --tmpfs /tmp:rw,noexec,nosuid,nodev,size=4m --memory 64m --cpus 0.25 --pids-limit 32 --cap-drop ALL --security-opt no-new-privileges \
  --mount "type=bind,src=$fixture_snapshot,dst=/fixture.py,readonly" "$fixture_image" python3 /fixture.py --port 8080 >/dev/null
fixture_ip="$(docker_cmd inspect --format "{{with index .NetworkSettings.Networks \"$network_name\"}}{{.IPAddress}}{{end}}" "sparrow-fixture-$owner")"
readonly fixture_ip
fixture_deadline=$((SECONDS + 30))
until curl -q --proto '=http' --proto-redir '=http' --max-redirs 0 --noproxy '*' --resolve "fixture.local:8080:$fixture_ip" --silent --fail --max-time 2 --max-filesize 1048576 http://fixture.local:8080/catalog.m3u >/dev/null \
  && curl -q --proto '=http' --proto-redir '=http' --max-redirs 0 --noproxy '*' --resolve "fixture.local:8080:$fixture_ip" --silent --fail --max-time 2 --max-filesize 1048576 http://fixture.local:8080/guide.xml >/dev/null; do
  (( SECONDS < fixture_deadline )) || { echo "the synthetic fixture did not become ready" >&2; exit 1; }
  [[ "$(docker_cmd inspect --format '{{.State.Running}}' "sparrow-fixture-$owner")" == true ]] || exit 1
  sleep 1
done
printf 'M3U_PATH=http://sparrow-fixture-%s:8080/catalog.m3u\n' "$owner" > "$environment_file"
printf 'EPG_PATH=http://sparrow-fixture-%s:8080/guide.xml\n' "$owner" >> "$environment_file"

start_stage() {
  local image="$1"
  PASSWORD="$password" docker_cmd run --detach \
    --name "$container_name" \
    --label "xyz.ponbac.sparrow.cutover=$owner" \
    --network "$network_name" \
    --env-file "$environment_file" \
    --env PASSWORD \
    --read-only \
    --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m \
    --cap-drop ALL \
    --security-opt no-new-privileges \
    --pids-limit 256 \
    --memory 512m --cpus 1.0 \
    "$image" >/dev/null
}

stop_stage() {
  docker_cmd rm --force --volumes "$container_name" >/dev/null
}

wait_for_http() {
  local path="$1"
  local container_ip="$2"
  local deadline=$((SECONDS + 720))
  until curl -q --proto '=http' --proto-redir '=http' --max-redirs 0 --noproxy '*' \
    --resolve "localhost:33733:$container_ip" --silent --fail --output /dev/null --max-time 2 \
    "http://localhost:33733$path"; do
    (( SECONDS < deadline )) || return 1
    [[ "$(docker_cmd inspect --format '{{.State.Running}}' "$container_name")" == true ]] || return 1
    sleep 2
  done
}

run_stage() {
  local mode="$1"
  local role="$2"
  local image="$3"
  local evidence="$work/$role.json"
  start_stage "$image"
  local container_ip
  container_ip="$(docker_cmd inspect --format "{{with index .NetworkSettings.Networks \"$network_name\"}}{{.IPAddress}}{{end}}" "$container_name")"
  local base_url="http://localhost:33733"
  if [[ "$mode" == replacement ]]; then
    wait_for_http "/health" "$container_ip"
    SPARROW_PINNED_BUN="$bun_path" SPARROW_HOSTED_PASSWORD="$password" SPARROW_HOSTED_RESOLVE_IP="$container_ip" \
      bash scripts/verify-hosted-endpoint.sh "$mode" "$role" "$base_url" "$evidence"
  else
    wait_for_http "/app/" "$container_ip"
    SPARROW_PINNED_BUN="$bun_path" SPARROW_HOSTED_RESOLVE_IP="$container_ip" bash scripts/verify-hosted-endpoint.sh "$mode" "$role" "$base_url" "$evidence"
  fi
  stop_stage
}

run_stage legacy baseline "$baseline_image"
run_stage replacement candidate "$candidate_image"
run_stage legacy rollback "$baseline_image"

actual_revision="$(docker_cmd image inspect "$candidate_image" --format '{{index .Config.Labels "org.opencontainers.image.revision"}}')"
if [[ "$actual_revision" != "$candidate_revision" ]]; then
  echo "the candidate image revision does not match the accepted hosted revision" >&2
  exit 1
fi

jq -n \
  --arg recordedAt "$("$bun_path" -e 'process.stdout.write(new Date().toISOString())')" \
  --argjson baseline "$(<"$work/baseline.json")" \
  --argjson candidate "$(<"$work/candidate.json")" \
  --argjson rollback "$(<"$work/rollback.json")" \
  --arg baselineRef "$baseline_image" \
  --arg baselineDigest "${baseline_image##*@}" \
  --arg candidateRef "$candidate_image" \
  --arg candidateDigest "${candidate_image##*@}" \
  --arg candidateRevision "$candidate_revision" \
  --arg fixtureImage "$fixture_image" \
  --arg fixtureDigest "${fixture_image##*@}" \
  --arg fixtureScript "$(sha256sum "$fixture_snapshot" | cut -d' ' -f1)" \
  '{
    schemaVersion:1,
    rehearsal:"isolated-baseline-candidate-rollback",
    recordedAt:$recordedAt,
    dockerContextClass:"local-unix",
    fixture:{image:{reference:$fixtureImage,digest:$fixtureDigest},scriptSha256:$fixtureScript},
    steps:[
      {role:"baseline",image:{reference:$baselineRef,digest:$baselineDigest},revision:null,serviceName:"sparrow",containerPort:33733,endpoint:$baseline},
      {role:"candidate",image:{reference:$candidateRef,digest:$candidateDigest},revision:$candidateRevision,serviceName:"sparrow",containerPort:33733,endpoint:$candidate},
      {role:"rollback",image:{reference:$baselineRef,digest:$baselineDigest},revision:null,serviceName:"sparrow",containerPort:33733,endpoint:$rollback}
    ]
  }' > "$work/observation.json"

"$bun_path" app/scripts/hosted-cutover.ts verify-rehearsal \
  --plan "$plan" \
  --observation "$work/observation.json" \
  --environment-backup "$environment_file" \
  --evidence-key "$evidence_key" \
  --output "$verified_output"
