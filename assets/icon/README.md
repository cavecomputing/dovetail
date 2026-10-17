# App icon

The monogram: a D with a dovetail socket cut into its spine, in the Gruvbox `accent` yellow on a
`bg-surface` tile (see docs/design-tokens.json).

- `icon.svg`: the app icon, for launchers, docks and 48 px and up.
- `icon-small.svg`: the same mark bigger in a full-bleed tile, for 16 to 32 px (panels, tray, title bars).
- `icon-symbolic.svg`: the bare mark in one colour, for themes and bars that recolour symbolic icons.

The files are named generically on purpose: the installer copies them under the app's name from
src/brand.rs, so a rename doesn't touch them.
