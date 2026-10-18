# App icon

The monogram: a D with a dovetail socket cut into its spine, in the Gruvbox `accent` yellow on a
`bg-surface` tile (see docs/design-tokens.json).

- `icon.svg`: the app icon, for launchers, docks and 48 px and up.
- `icon-small.svg`: the same mark bigger in a full-bleed tile, for 16 to 32 px (panels, tray, title bars).
- `icon-symbolic.svg`: the bare mark in one colour, for themes and bars that recolour symbolic icons.
- `icon.icns`: the macOS icon, which `scripts/install-macos.sh` puts in the app bundle. It is
  `icon.svg` on Apple's icon grid (the tile 824 px wide in 1024), so it sits level with other apps
  in the Dock.
- `icon.ico`: the Windows icon, for the Start Menu entry `scripts/install-windows.ps1` makes. As on
  Linux, 16 to 32 px come from `icon-small.svg` and 48 px and up from `icon.svg`.
- `icon-64.rgba`: `icon.svg` at 64x64 as raw RGBA bytes, which the app sets as its window icon
  (it needs no image decoder that way).

The files are named generically on purpose: on Linux the install script copies them under the app
id from src/brand.rs, and on macOS and Windows they go inside the app's own bundle or folder, so a
rename doesn't touch them.

After changing `icon.svg` or `icon-small.svg`, regenerate the other files from this folder, in bash.
The tools are rsvg-convert, ImageMagick and png2icns (Ubuntu: `librsvg2-bin imagemagick icnsutils`;
Arch: `librsvg imagemagick libicns`):

    rsvg-convert -w 64 -h 64 icon.svg -o /tmp/icon.png && convert /tmp/icon.png -depth 8 rgba:icon-64.rgba

    tmp=$(mktemp -d)
    for s in 16 24 32; do rsvg-convert -w $s -h $s icon-small.svg -o $tmp/ico-$s.png; done
    for s in 48 64 256; do rsvg-convert -w $s -h $s icon.svg -o $tmp/ico-$s.png; done
    convert $tmp/ico-{16,24,32,48,64,256}.png icon.ico

    rsvg-convert -w 880 -h 880 --left 72 --top 72 --page-width 1024 --page-height 1024 icon.svg -o $tmp/icns-1024.png
    for s in 16 32 128 256 512; do convert $tmp/icns-1024.png -resize ${s}x$s -strip $tmp/icns-$s.png; done
    png2icns icon.icns $tmp/icns-{16,32,128,256,512,1024}.png
