#!/bin/sh
# Build the app bundle's icon (`Contents/Resources/AppIcon.icns`) from the app's square PNG logo.
#
#   sh make-icns.sh <source.png> <out.icns>
#
# Runs on macOS only: it shells out to `sips` (resampling) and `iconutil` (iconset -> icns), both of
# which ship with macOS. The release workflow calls it on the macOS runner; there is no local
# equivalent on Windows/Linux, which is why the bundle is only ever assembled in CI.
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: sh make-icns.sh <source.png> <out.icns>" >&2
    exit 2
fi

source_png="$1"
out_icns="$2"

# An explicit template keeps this portable across BSD `mktemp` (macOS) and GNU `mktemp`, which
# disagree about the default when no template is given.
work="$(mktemp -d "${TMPDIR:-/tmp}/rustgrid-icns.XXXXXX")"
trap 'rm -rf "$work"' EXIT

# `iconutil` only accepts the `.iconset` layout, and it wants the @2x names to hold the doubled
# pixel size (so `icon_512x512@2x.png` is 1024x1024). The padded Dock logo is 512x512, so the
# largest entry is upscaled by `sips` — slightly soft, but it keeps the iconset complete.
iconset="$work/AppIcon.iconset"
mkdir -p "$iconset"
for entry in \
    16:icon_16x16 \
    32:icon_16x16@2x \
    32:icon_32x32 \
    64:icon_32x32@2x \
    128:icon_128x128 \
    256:icon_128x128@2x \
    256:icon_256x256 \
    512:icon_256x256@2x \
    512:icon_512x512 \
    1024:icon_512x512@2x
do
    pixels="${entry%%:*}"
    name="${entry#*:}"
    sips -z "$pixels" "$pixels" "$source_png" --out "$iconset/$name.png" >/dev/null
done

iconutil -c icns "$iconset" -o "$out_icns"
echo "wrote $out_icns"
