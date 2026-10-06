#!/bin/bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
target=${1:?target triple required}
cache=${2:?build directory required}
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "Unsupported VibeVoice speech target: $target" >&2; exit 1 ;;
esac
mkdir -p "$cache"
cache=$(cd "$cache" && pwd -P)
fetch() {
    local name=$1 url=$2 sha=$3
    local archive="$cache/$name.tar.gz"
    if [ ! -f "$archive" ]; then
        curl --fail --location --retry 3 --connect-timeout 30 --max-time 600 "$url" --output "$archive.part"
        mv "$archive.part" "$archive"
    fi
    if [ "$(shasum -a 256 "$archive" | cut -d ' ' -f 1)" != "$sha" ]; then
        echo "VibeVoice source checksum mismatch; remove $archive and retry." >&2
        exit 1
    fi
}
fetch vibe 'https://codeload.github.com/microsoft/VibeASR.cpp/tar.gz/c4334009c88060f86cdbbd684b62662f710b6c20' '3d115d10f1e3e43cbc77b544ca342d21e66c7ed955d15eb86e2c9a70e6b72670'
fetch llama 'https://codeload.github.com/XsquirrelC/llama.cpp/tar.gz/a2fdadc20285df2dce90402fca9264a93a8eb32f' 'e8b167ae29f345c5e1b43ea80f252d63a3ae13deb742567f333342134525c2dc'
source_dir="$cache/VibeASR.cpp-c4334009c88060f86cdbbd684b62662f710b6c20"
if [ ! -d "$source_dir" ]; then
    staging=$(mktemp -d "$cache/unpack.XXXXXX")
    trap 'rm -rf "$staging"' EXIT
    tar -xzf "$cache/vibe.tar.gz" -C "$staging"
    tar -xzf "$cache/llama.tar.gz" -C "$staging"
    rmdir "$staging/VibeASR.cpp-c4334009c88060f86cdbbd684b62662f710b6c20/3rdparty/llama.cpp"
    mv "$staging/llama.cpp-a2fdadc20285df2dce90402fca9264a93a8eb32f" "$staging/VibeASR.cpp-c4334009c88060f86cdbbd684b62662f710b6c20/3rdparty/llama.cpp"
    mv "$staging/VibeASR.cpp-c4334009c88060f86cdbbd684b62662f710b6c20" "$source_dir"
    rmdir "$staging"
    trap - EXIT
fi
cmake -S "$root/native-vibe" -B "$cache/build" -DVIBE_SOURCE="$source_dir" \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_OSX_ARCHITECTURES="$arch" -DCMAKE_OSX_DEPLOYMENT_TARGET=12.0
cmake --build "$cache/build" --target asr_stream_server --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-4}"
helper="$cache/build/bin/openglaido-vibe"
"$helper" --self-test
lipo "$helper" -verify_arch "$arch"
mkdir -p "$root/src-tauri/binaries"
destination="$root/src-tauri/binaries/openglaido-vibe-$target"
if ! cmp -s "$helper" "$destination"; then
    staged=$(mktemp "$destination.XXXXXX")
    trap 'rm -f "$staged"' EXIT
    cp "$helper" "$staged"
    chmod 755 "$staged"
    mv "$staged" "$destination"
    trap - EXIT
fi
