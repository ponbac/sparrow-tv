#!/bin/bash

# Source the environment variables from .env.local
M3U_PATH=$(grep "M3U_PATH=" .env.local | cut -d '=' -f2-)
EPG_PATH=$(grep "EPG_PATH=" .env.local | cut -d '=' -f2-)

if [ -z "$M3U_PATH" ] || [ -z "$EPG_PATH" ]; then
  echo "M3U_PATH or EPG_PATH is not set in .env.local"
  exit 1
fi

# Ensure output directory exists
mkdir -p ./examples

echo "Fetching EPG..."
curl -o ./examples/epg.xml "$EPG_PATH"

echo "Fetching M3U..."
curl -o ./examples/playlist.m3u "$M3U_PATH"
