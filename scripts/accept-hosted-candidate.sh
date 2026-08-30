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
[[ $# -eq 4 ]] || { echo "usage: $0 <image@digest> <revision> <manifest> <output>" >&2; exit 2; }
readonly image="$1" revision="$2" manifest="$3" output="$4" password="${SPARROW_REHEARSAL_PASSWORD:-}"
unset SPARROW_REHEARSAL_PASSWORD
readonly fixture_image="docker.io/library/python:3.13.15-alpine3.24@sha256:540c7d91f98ff6880174c40e99067bf5941eb54d818a7a5e094d188b196a934d"
[[ "$image" == *@sha256:* && "$manifest" == "${image##*@}" ]] || exit 2
[[ "$revision" =~ ^[0-9a-f]{40}$ && -n "$password" ]] || exit 2
owner="$(tr -d '\n' < /proc/sys/kernel/random/uuid)"
readonly owner
work="$(mktemp -d)"
readonly work
readonly network="sparrow-accept-$owner" candidate="sparrow-candidate-$owner" fixture="sparrow-fixture-$owner"
context="$(/usr/bin/docker context show)"
readonly context
docker_endpoint="$(/usr/bin/docker context inspect "$context" --format '{{(index .Endpoints "docker").Host}}')"
readonly docker_endpoint
[[ "$docker_endpoint" == unix://* ]] || exit 2
docker_cmd() { /usr/bin/docker --host "$docker_endpoint" "$@"; }
cleanup() { while IFS= read -r id; do [[ -n "$id" ]] && docker_cmd rm -f --volumes "$id" >/dev/null 2>&1 || true; done < <(docker_cmd ps -aq --filter "label=xyz.ponbac.sparrow.acceptance=$owner" 2>/dev/null || true); docker_cmd network rm "$network" >/dev/null 2>&1 || true; rm -rf -- "$work"; }
trap cleanup EXIT
for bounded_image in "$image" "$fixture_image"; do
  if [[ "$bounded_image" == "$image" ]]; then docker_cmd image inspect "$bounded_image" >/dev/null
  else docker_cmd image inspect "$bounded_image" >/dev/null 2>&1 || docker_cmd pull "$bounded_image" >/dev/null; fi
  [[ "$(docker_cmd image inspect "$bounded_image" --format '{{json .Config.Volumes}}')" == null ]] || { echo "acceptance images must not declare volumes" >&2; exit 2; }
done
docker_cmd network create --internal --label "xyz.ponbac.sparrow.acceptance=$owner" "$network" >/dev/null
fixture_snapshot="$work/fixture.py"
"$bun_path" app/scripts/hosted-cutover.ts snapshot-rehearsal-fixture --output "$fixture_snapshot"
fixture_user="$(stat -Lc '%u:%g' "$fixture_snapshot")"
readonly fixture_user
[[ "$fixture_user" =~ ^[0-9]+:[0-9]+$ ]] || exit 1
docker_cmd run -d --name "$fixture" --label "xyz.ponbac.sparrow.acceptance=$owner" --network "$network" --user "$fixture_user" --read-only --memory 64m --cpus 0.25 --pids-limit 32 --cap-drop ALL --security-opt no-new-privileges --mount "type=bind,src=$fixture_snapshot,dst=/fixture.py,readonly" "$fixture_image" python3 /fixture.py --port 8080 >/dev/null
fixture_ip="$(docker_cmd inspect --format "{{with index .NetworkSettings.Networks \"$network\"}}{{.IPAddress}}{{end}}" "$fixture")"
readonly fixture_ip
fixture_deadline=$((SECONDS + 30))
until curl -q --proto '=http' --proto-redir '=http' --max-redirs 0 --noproxy '*' --resolve "fixture.local:8080:$fixture_ip" --silent --fail --max-time 2 --max-filesize 1048576 http://fixture.local:8080/catalog.m3u >/dev/null \
  && curl -q --proto '=http' --proto-redir '=http' --max-redirs 0 --noproxy '*' --resolve "fixture.local:8080:$fixture_ip" --silent --fail --max-time 2 --max-filesize 1048576 http://fixture.local:8080/guide.xml >/dev/null; do
  (( SECONDS < fixture_deadline )) || { echo "the synthetic fixture did not become ready" >&2; exit 1; }
  [[ "$(docker_cmd inspect --format '{{.State.Running}}' "$fixture")" == true ]] || exit 1
  sleep 1
done
printf 'M3U_PATH=http://%s:8080/catalog.m3u\nEPG_PATH=http://%s:8080/guide.xml\n' "$fixture" "$fixture" > "$work/synthetic.env"
PASSWORD="$password" docker_cmd run -d --name "$candidate" --label "xyz.ponbac.sparrow.acceptance=$owner" --network "$network" --env-file "$work/synthetic.env" --env PASSWORD --read-only --tmpfs /tmp:rw,noexec,nosuid,nodev,size=16m --memory 512m --cpus 1.0 --pids-limit 256 --cap-drop ALL --security-opt no-new-privileges "$image" >/dev/null
candidate_ip="$(docker_cmd inspect --format "{{with index .NetworkSettings.Networks \"$network\"}}{{.IPAddress}}{{end}}" "$candidate")"
readonly candidate_ip base_url="http://localhost:33733"
deadline=$((SECONDS + 720))
until curl -q --proto '=http' --proto-redir '=http' --max-redirs 0 --noproxy '*' --resolve "localhost:33733:$candidate_ip" --silent --fail --max-time 2 "$base_url/health" >/dev/null; do
  (( SECONDS < deadline )) || { echo "candidate did not become healthy" >&2; exit 1; }
  [[ "$(docker_cmd inspect --format '{{.State.Running}}' "$candidate")" == true ]] || exit 1
  sleep 2
done
SPARROW_PINNED_BUN="$bun_path" SPARROW_HOSTED_PASSWORD="$password" SPARROW_HOSTED_RESOLVE_IP="$candidate_ip" bash scripts/verify-hosted-endpoint.sh replacement candidate "$base_url" "$work/endpoint.json"
[[ "$(docker_cmd image inspect "$image" --format '{{index .Config.Labels "org.opencontainers.image.revision"}}')" == "$revision" ]] || exit 1
jq -n --arg recordedAt "$("$bun_path" -e 'process.stdout.write(new Date().toISOString())')" --arg image "$image" --arg digest "$manifest" --arg revision "$revision" --arg fixtureImage "$fixture_image" --arg fixtureDigest "${fixture_image##*@}" --arg fixtureScript "$(sha256sum "$fixture_snapshot" | cut -d' ' -f1)" --argjson endpoint "$(<"$work/endpoint.json")" '{schemaVersion:1,verdict:"hosted-accepted",recordedAt:$recordedAt,image:{reference:$image,digest:$digest},revision:$revision,reproducedManifestDigest:$digest,containerPort:33733,endpoint:$endpoint,fixture:{image:{reference:$fixtureImage,digest:$fixtureDigest},scriptSha256:$fixtureScript}}' > "$work/acceptance.json"
"$bun_path" app/scripts/hosted-cutover.ts record-hosted-acceptance --input "$work/acceptance.json" --output "$output" >/dev/null
