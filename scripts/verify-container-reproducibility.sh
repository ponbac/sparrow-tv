#!/usr/bin/env bash

set -euo pipefail
umask 077

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <git-revision> <verified-oci-output>" >&2
  exit 2
fi

readonly requested_revision="$1"
readonly output_archive="$2"
readonly buildkit_image="moby/buildkit:v0.32.2@sha256:040d34121c27906c4ff9ac152a30d52bf2c5d328d3bb748916bb3d2743c02528"
revision="$(git rev-parse --verify "${requested_revision}^{commit}")"
readonly revision
source_epoch="$(git show -s --format=%ct "$revision")"
readonly source_epoch
verification_root="$(mktemp -d)"
readonly verification_root
readonly builder_a="sparrow-repro-a-$$"
readonly builder_b="sparrow-repro-b-$$"

builder_a_created=false
builder_b_created=false

cleanup() {
  if [[ "$builder_a_created" == true ]]; then
    docker buildx rm "$builder_a" >/dev/null 2>&1 || true
  fi
  if [[ "$builder_b_created" == true ]]; then
    docker buildx rm "$builder_b" >/dev/null 2>&1 || true
  fi
  rm -rf -- "$verification_root"
}
trap cleanup EXIT

git archive --format=tar --output="$verification_root/context.tar" "$revision"

docker buildx create \
  --name "$builder_a" \
  --driver docker-container \
  --driver-opt "image=$buildkit_image" >/dev/null
builder_a_created=true
docker buildx create \
  --name "$builder_b" \
  --driver docker-container \
  --driver-opt "image=$buildkit_image" >/dev/null
builder_b_created=true

build_once() {
  local label="$1"
  local builder="$2"

  SOURCE_DATE_EPOCH="$source_epoch" docker buildx build \
    --builder "$builder" \
    --platform linux/amd64 \
    --pull \
    --no-cache \
    --build-arg "SOURCE_DATE_EPOCH=$source_epoch" \
    --build-arg "VCS_REF=$revision" \
    --tag "sparrow-repro:$revision" \
    --provenance=false \
    --sbom=false \
    --output "type=oci,dest=$verification_root/$label.oci,rewrite-timestamp=true,compatibility-version=30" \
    - < "$verification_root/context.tar"

  mkdir "$verification_root/$label"
  tar -xf "$verification_root/$label.oci" -C "$verification_root/$label"
}

build_once a "$builder_a"
build_once b "$builder_b"

diff --no-dereference --recursive --brief \
  "$verification_root/a" \
  "$verification_root/b"

manifest_a="$(
  jq -er \
    'if (.manifests | length) == 1 then .manifests[0].digest else error("unexpected OCI index") end' \
    "$verification_root/a/index.json"
)"
readonly manifest_a
manifest_b="$(
  jq -er \
    'if (.manifests | length) == 1 then .manifests[0].digest else error("unexpected OCI index") end' \
    "$verification_root/b/index.json"
)"
readonly manifest_b

if [[ "$manifest_a" != "$manifest_b" ]]; then
  echo "container manifests are not reproducible" >&2
  exit 1
fi

install -m 600 "$verification_root/a.oci" "$output_archive"

printf 'revision=%s\n' "$revision"
printf 'manifest=%s\n' "$manifest_a"
printf 'verifiedOci=%s\n' "$output_archive"
