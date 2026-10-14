#!/bin/bash
# Builds the app icon from the banner (assets/logo_ts.png): a square steel dog tag with the
# banner's blood splatter and "TS" in Anton, the banner's lettering style.
# Writes icon.png (512px) and ts-launcher.ico (16-256px) next to this script. Needs ImageMagick 7.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
banner=$here/../logo_ts.png
font=$here/../Anton-Regular.ttf
tmp=${ICON_TMP:-$(mktemp -d)}
[ -n "${ICON_TMP:-}" ] || trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
S=512
TXT=(-font "$font" -pointsize 300 -gravity center)

# Brushed steel, lighter at the top like the banner.
# Made wider than the tag and cropped, so the blur's ragged ends fall outside it.
magick -size 768x${S} xc:gray55 -seed 7 -attenuate 0.5 +noise Gaussian -motion-blur 0x70+0 -gravity center -crop ${S}x${S}+0+0 +repage \
  -colorspace gray -level 30%,75% \( -size ${S}x${S} gradient:gray75-gray40 \) -compose multiply -composite -level 0%,85% -modulate 135 -level 5%,95% \
  -set colorspace sRGB -type TrueColor -define png:color-type=2 steel.png

# The tag: a rounded square with a hole for the chain.
magick -size ${S}x${S} xc:black -fill white -draw "roundrectangle 16,16 495,495 96,96" -fill black -draw "circle 108,108 108,138" mask.png

# Blood: the red pixels of the banner's right end, turned to run down the tag's right side,
# faded in from the left and cleared of the banner's lettering at the bottom.
magick "$banner" -crop 110x78+290+0 +repage -resize 560x368! blood_big.png
magick blood_big.png -fx 'r>g*1.6 && r>0.25 ? 1 : 0' -blur 0x1 blood_mask.png
magick blood_big.png blood_mask.png -alpha off -compose copy_opacity -composite -background none -rotate 90 -resize 330x \
  -crop 300x502+30+0 +repage blood_rot.png
magick -size 502x300 gradient:black-white -rotate 270 -level 0%,70% ramp.png
magick blood_rot.png \( blood_rot.png -alpha extract ramp.png -compose multiply -composite -fill black -draw "rectangle 0,440 120,502" \) \
  -alpha off -compose copy_opacity -composite blood.png

# Lettering: a bevelled light face, a dark outline and a drop shadow, as on the banner.
magick -size ${S}x${S} xc:none "${TXT[@]}" -fill white -annotate +0+0 'TS' text.png
magick text.png -alpha extract -blur 0x4 -shade 120x50 -auto-level -level 20%,100% -fill '#ece8e2' -colorize 30 face_rgb.png
magick face_rgb.png text.png -compose copy_opacity -composite face.png
magick text.png -alpha extract -morphology dilate disk:7 -background black -alpha shape -fill '#262626' -colorize 100 outline.png
magick text.png -alpha extract -morphology dilate disk:7 -blur 0x9 -background black -alpha shape -fill black -colorize 100 \
  -channel A -evaluate multiply 0.8 +channel shadow.png

magick steel.png -set colorspace sRGB -type TrueColor \( blood.png -geometry +240-30 \) -compose over -composite \
  \( shadow.png -geometry +12+16 \) -compose over -composite outline.png -compose over -composite face.png -compose over -composite \
  mask.png -alpha off -compose copy_opacity -composite \
  \( mask.png -morphology edgein disk:5 -fill '#303030' -opaque white -transparent black \) -compose over -composite \
  -depth 8 -define png:color-type=6 "$here/icon.png"

magick "$here/icon.png" -define icon:auto-resize=256,128,64,48,32,24,16 "$here/ts-launcher.ico"
echo "wrote $here/icon.png and $here/ts-launcher.ico"

# Installer wizard images (installer/): the icon on the banner's grunge background for the side
# panel, and on white for the page header. Sized for 200% scaling; Inno Setup scales them down.
inst=$here/../../installer
magick "$here/../grunge.jpg" -resize 328x628^ -gravity center -extent 328x628 -modulate 70 \
  \( "$here/icon.png" -resize 240x240 \) -gravity north -geometry +0+120 -compose over -composite \
  \( "$here/../logo_ts.png" -resize 300x \) -gravity south -geometry +0+140 -compose over -composite \
  -background black -flatten -type TrueColor BMP3:"$inst/wizard.bmp"
magick "$here/icon.png" -resize 110x110 -background white -gravity center -extent 110x110 -flatten -type TrueColor \
  BMP3:"$inst/wizard-small.bmp"
echo "wrote $inst/wizard.bmp and $inst/wizard-small.bmp"
