#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
work="$root/.local/media/source"
prefix="$work/prefix"
jobs="${VIDEO_TRANS_BUILD_JOBS:-8}"
export MACOSX_DEPLOYMENT_TARGET=13.0
export CFLAGS="-O2 -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET"
export CXXFLAGS="$CFLAGS"
export LDFLAGS="-mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET"

if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
  echo 'Run this script on an Apple Silicon Mac.' >&2
  exit 1
fi

mkdir -p "$work" "$prefix" "$root/src-tauri/binaries" "$root/src-tauri/resources/licenses"
cd "$work"

fetch() {
  local url="$1" output="$2"
  if [[ ! -s "$output" ]]; then
    curl --fail --location --retry 2 --show-error --silent "$url" -o "$output.partial"
    mv "$output.partial" "$output"
  fi
}

fetch https://ffmpeg.org/releases/ffmpeg-8.0.1.tar.xz ffmpeg-8.0.1.tar.xz
fetch https://codeload.github.com/webmproject/libvpx/tar.gz/refs/tags/v1.15.2 libvpx-1.15.2.tar.gz
fetch https://downloads.xiph.org/releases/opus/opus-1.5.2.tar.gz opus-1.5.2.tar.gz
fetch https://distfiles.ariadne.space/pkgconf/pkgconf-2.4.3.tar.xz pkgconf-2.4.3.tar.xz

shasum -a 256 -c <<'CHECKSUMS'
05ee0b03119b45c0bdb4df654b96802e909e0a752f72e4fe3794f487229e5a41  ffmpeg-8.0.1.tar.xz
26fcd3db88045dee380e581862a6ef106f49b74b6396ee95c2993a260b4636aa  libvpx-1.15.2.tar.gz
65c1d2f78b9f2fb20082c38cbe47c951ad5839345876e46941612ee87f9a7ce1  opus-1.5.2.tar.gz
51203d99ed573fa7344bf07ca626f10c7cc094e0846ac4aa0023bd0c83c25a41  pkgconf-2.4.3.tar.xz
CHECKSUMS

extract() {
  if [[ ! -d "$2" ]]; then
    tar -xf "$1"
  fi
}

extract pkgconf-2.4.3.tar.xz pkgconf-2.4.3
extract libvpx-1.15.2.tar.gz libvpx-1.15.2
extract opus-1.5.2.tar.gz opus-1.5.2
extract ffmpeg-8.0.1.tar.xz ffmpeg-8.0.1

if [[ ! -x "$prefix/bin/pkgconf" ]]; then
  (cd pkgconf-2.4.3 && ./configure --prefix="$prefix" --disable-shared && make -j"$jobs" && make install)
fi
if [[ ! -f "$prefix/lib/libopus.a" ]]; then
  (cd opus-1.5.2 && ./configure --prefix="$prefix" --disable-shared --disable-doc --disable-extra-programs &&
    make -j"$jobs" && make install)
fi
if [[ ! -f "$prefix/lib/libvpx.a" ]]; then
  (cd libvpx-1.15.2 &&
    ./configure --prefix="$prefix" --target=arm64-darwin20-gcc --disable-shared --disable-examples \
      --disable-tools --disable-docs --disable-unit-tests --enable-vp9-highbitdepth &&
    make -j"$jobs" && make install)
fi

(
  cd ffmpeg-8.0.1
  PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig" ./configure \
    --arch=arm64 --target-os=darwin --cc=clang --disable-autodetect --disable-network \
    --disable-doc --disable-debug --disable-ffplay --disable-shared --enable-static \
    --enable-videotoolbox --enable-libvpx --enable-libopus --enable-zlib \
    --pkg-config="$prefix/bin/pkgconf" --pkg-config-flags=--static \
    --extra-cflags="-I$prefix/include -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET" \
    --extra-ldflags="-L$prefix/lib -mmacosx-version-min=$MACOSX_DEPLOYMENT_TARGET"
  make -j"$jobs" ffmpeg ffprobe
)
fetch https://raw.githubusercontent.com/notofonts/noto-cjk/Sans2.004/Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Regular.otf NotoSansCJKsc-Regular.otf
fetch https://raw.githubusercontent.com/notofonts/noto-cjk/main/Sans/LICENSE Noto-OFL.txt
shasum -a 256 -c <<'CHECKSUMS'
2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b  NotoSansCJKsc-Regular.otf
6a73f9541c2de74158c0e7cf6b0a58ef774f5a780bf191f2d7ec9cc53efe2bf2  Noto-OFL.txt
CHECKSUMS
for program in ffmpeg ffprobe; do
  install -m 755 "ffmpeg-8.0.1/$program" "$root/src-tauri/binaries/$program-aarch64-apple-darwin"
done
install -m 644 NotoSansCJKsc-Regular.otf "$root/src-tauri/resources/NotoSansCJKsc-Regular.otf"
install -m 644 Noto-OFL.txt "$root/src-tauri/resources/licenses/Noto-OFL.txt"
install -m 644 ffmpeg-8.0.1/COPYING.LGPLv2.1 "$root/src-tauri/resources/licenses/FFmpeg-LGPLv2.1.txt"
install -m 644 libvpx-1.15.2/LICENSE "$root/src-tauri/resources/licenses/libvpx-LICENSE.txt"
install -m 644 libvpx-1.15.2/PATENTS "$root/src-tauri/resources/licenses/libvpx-PATENTS.txt"
install -m 644 opus-1.5.2/COPYING "$root/src-tauri/resources/licenses/Opus-COPYING.txt"
shasum -a 256 ffmpeg-8.0.1.tar.xz libvpx-1.15.2.tar.gz opus-1.5.2.tar.gz \
  NotoSansCJKsc-Regular.otf > "$root/src-tauri/resources/licenses/source-sha256.txt"
