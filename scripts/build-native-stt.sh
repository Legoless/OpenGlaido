#!/bin/bash
set -euo pipefail
# Called by Cargo's build.rs; all downloaded/build output stays outside tracked sources.
root=$(cd "$(dirname "$0")/.." && pwd)
target=${1:?target triple required}
cache=${2:?build directory required}
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "Unsupported native speech target: $target" >&2; exit 1 ;;
esac
if [ -L "$cache" ] && [ ! -e "$cache" ]; then
  rm "$cache"
fi
mkdir -p "$cache"
cache=$(cd "$cache" && pwd -P)
source_archive="$cache/transcribe-ark.tar.gz"
source_dir="$cache/transcribe.cpp-arkasr"
sha=776c2a7a690dc21dc629aa79d03003a0a9f9a463f1e0a4f3a4cefc6ee59dbe97
if [ ! -f "$source_archive" ]; then
  curl --fail --location --retry 3 --connect-timeout 30 --max-time 600 \
    'https://huggingface.co/harshav/ARK-ASR-3B-GGUF/resolve/14239df600b76b1fd697423eb927f9f4622fe739/transcribe.cpp-arkasr-b838a2b.tar.gz' \
    --output "$source_archive.part"
  mv "$source_archive.part" "$source_archive"
fi
actual=$(shasum -a 256 "$source_archive" | cut -d ' ' -f 1)
if [ "$actual" != "$sha" ]; then
  echo "Native speech source checksum mismatch; remove $source_archive and retry." >&2
  exit 1
fi
if [ ! -d "$source_dir" ]; then
  staging=$(mktemp -d "$cache/unpack.XXXXXX")
  trap 'rm -rf "$staging"' EXIT
  tar -xzf "$source_archive" -C "$staging"
  mv "$staging/transcribe.cpp-arkasr" "$source_dir"
  rmdir "$staging"
  trap - EXIT
fi
cmake -S "$root/native-stt" -B "$cache/build" \
  -DTRANSCRIBE_SOURCE="$source_dir" -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_ARCHITECTURES="$arch" -DCMAKE_OSX_DEPLOYMENT_TARGET=11.0
cmake --build "$cache/build" --target openglaido-stt --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-4}"
helper="$cache/build/openglaido-stt"
"$helper" --self-test
mkdir -p "$root/src-tauri/binaries"
destination="$root/src-tauri/binaries/openglaido-stt-$target"
if ! cmp -s "$helper" "$destination"; then
  staged=$(mktemp "$destination.XXXXXX")
  trap 'rm -f "$staged"' EXIT
  cp "$helper" "$staged"
  chmod 755 "$staged"
  mv "$staged" "$destination"
  trap - EXIT
fi
