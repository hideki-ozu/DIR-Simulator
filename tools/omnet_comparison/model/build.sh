#!/usr/bin/env bash
# Build only this test adapter; leave installed FiCo and OMNeT++ untouched.
set -eo pipefail
adapter_src="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
omnet_workspace="${OMNET_WORKSPACE:-/home/hideki/Omnet++}"
source "$omnet_workspace/scripts/env.sh"
set -u
adapter_build="${1:-${BUILD_DIR:-$adapter_src/../../../tmp/omnet-adapter-build}}"
mkdir -p "$adapter_build"
adapter_build="$(cd "$adapter_build" && pwd)"
cp "$adapter_src/Adapter.cc" "$adapter_build/Adapter.cc"
cd "$adapter_build"
opp_makemake --make-so -f -o DirOmnetAdapter \
    -I"$omnet_workspace/upstream/FiCo4OMNeT/src" \
    -L"$omnet_workspace/upstream/FiCo4OMNeT/src" -lFiCo4OMNeT
make MODE=release -j"${BUILD_JOBS:-2}"
