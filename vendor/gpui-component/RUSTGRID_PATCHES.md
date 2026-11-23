# gpui-component 0.8.1, patched for RustGrid

This is the published `gpui-component` 0.8.1 crate (crates.io checksum
`46ce53430332e30f303364eaabfa10ed2aa23dfaf2a147e04370e0e9c423578b`), wired in through
`[patch.crates-io]` in the root `Cargo.toml`. It reaches the app through `gpui-kit`'s `component`
re-export. The first commit that added this directory imports it verbatim; every change since is
listed here. Licence: Apache-2.0 OR MIT (the files alongside).

## Changes

1. **Window controls follow the desktop's button layout** (`src/title_bar.rs`).
   The kit drew minimize/maximize/close in one fixed right-hand group, ignoring GNOME's
   `button-layout` (for example `close,minimize,maximize:` puts them on the left). The title bar
   now reads the layout gpui already exposes as `App::button_layout()` (fed by the XDG desktop
   portal on Linux) and renders one control group per side, keeping the platform's supported
   controls. A layout change repaints the window through
   `Window::observe_button_layout_changed`, so moving the buttons in the desktop's settings takes
   effect without a restart.
