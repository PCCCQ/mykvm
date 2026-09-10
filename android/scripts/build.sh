#!/usr/bin/env bash
# Builds the MyKVM Android receiver APK.
#
# Usage:
#   ./scripts/build.sh [debug|release] [abis]
#
#   ./scripts/build.sh release arm64-v8a
#
# The Rust protocol core (libmykvm_core.so) is cross-compiled by the
# :app:buildRustCore Gradle task before packaging.
set -euo pipefail

variant="${1:-debug}"
abis="${2:-}"

android_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

case "$variant" in
  debug)   task=":app:assembleDebug" ;;
  release) task=":app:assembleRelease" ;;
  *) echo "variant must be 'debug' or 'release' (got '$variant')" >&2; exit 2 ;;
esac

# Gradle needs a JDK 17+.
if [[ -z "${JAVA_HOME:-}" ]]; then
  echo "warning: JAVA_HOME is not set; relying on 'java' from PATH" >&2
fi

args=("$task")
if [[ -n "$abis" ]]; then
  args+=("-PrustAbis=$abis")
fi

cd "$android_dir"
if [[ -x "./gradlew" ]]; then
  ./gradlew "${args[@]}"
else
  gradle "${args[@]}"
fi

apk_dir="$android_dir/app/build/outputs/apk/$variant"
apk="$(find "$apk_dir" -maxdepth 1 -name '*.apk' -print -quit 2>/dev/null || true)"
if [[ -n "$apk" ]]; then
  echo
  echo "APK: $apk"
  ls -lh "$apk"
fi