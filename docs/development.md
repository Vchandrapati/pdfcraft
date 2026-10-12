# Developing PdfCraft

Working instructions for agents and contributors are in `AGENTS.md` and `../CODE.md`. This page
collects what the desktop app reads from its environment and where it writes its diagnostics.

## Side-by-side comparison (M12)

Open both PDFs, activate the newer version, then use **All tools → Compare files…** and choose
the older version. The existing Compare workflow opens older/newer panes in the Canvas panel,
with text changes highlighted on both sides and visual-change regions on the newer side. Click
a pane or scroll over it to focus it; the page box, zoom controls, editing, Save and Undo target
that document. Report and Mark as comments continue to use the comparison pair; comments are
added to the newer document regardless of pane focus.

The Compare panel has **Side by side** and **Link scrolling and zoom** switches. Linked scrolling
uses a page number and fractional position within the page, rather than raw pixel offsets.
Fixed zoom percentages match; Fit width/page/height fits each pane's own page dimensions. Pages
initially correspond by index. Selecting a text difference navigates both versions to their
respective pages and uses those pages as the subsequent scroll correspondence. The shorter
document stops at its last page. Turning linking off permits independent navigation.

Read and the docked Canvas use the same two document views. Full screen, Organize, Home and
unrelated document tabs retain their existing single-document workflows. Returning to a member
of the comparison restores the pair. Closing either member or clearing the comparison removes
the paired view and its highlights. Comparison results describe the last comparison; editing a
document does not automatically recompute them. Automatic page matching, change filters and
scanned-document OCR comparison remain separate work.

Implementation follows `CODE.md`'s simplicity and minimal-change principles: no dependencies or
second viewer are introduced. `compare_ui.rs` owns pair presentation and linking; `canvas.rs`
exposes page-relative navigation, uses stable document-scoped widget IDs and gates inactive-pane
input. Only the focused pane draws the shared floating quick-tools bar. `finish_render_frame`
admits results for both visible documents and suspends hidden tabs.
Each visible pane keeps the existing cache limits; this change does not add a global memory
budget or move comparison computation into a background worker.
The three new UI labels are included in all 20 existing language catalogs, with updated
translation-asset hashes in `ATTRIBUTION.toml`.

Headless `Session::compare_pages` and `doc_compare_pages` render independent old/new page choices
without edits. The tool returns JSON metadata, then two PNGs in older/newer order:

```json
{"tool":"doc_compare_pages","args":{"doc":2,"other":1,"page":3,"other_page":2,"dpi":96}}
```

In a CLI script, adding `"out":"pair.png"` saves `pair-1.png` (older) and `pair-2.png`
(newer), both subject to `--root`. Single-image tools retain their exact output filename.

Through the opt-in UI control channel, `ui.set` accepts `compare-view=side-by-side|single`,
`compare-sync=on|off` and `compare-focus=old|new` after comparison. `ui.state.compare` reports
pair IDs, whether both are visible, linking, and each pane's page, zoom, viewport and raster
memory. This also makes both page rectangles available for pointer-based UI tests.

Regression coverage is in `crates/ui-egui/tests/compare.rs`,
`crates/ui-egui/tests/control.rs::control_drives_comparison_panes_focus_and_view_options` and
`crates/automation/tests/automation.rs::comparison_pages_render_both_sides_without_mutating_documents`.
`apps/pdfcraft-cli/tests/run_root.rs::comparison_script_saves_both_pages_and_confines_both_outputs`
checks the script's two saved images and root confinement.

Validation on Windows (2026-10-12): `cargo test --locked --offline --workspace` passed
2,654 tests, with 13 ignored. The native `pdfcraft`/`pdfcraft-cli` build, workspace Clippy with
all targets and `-D warnings`, formatting, assets, dependency layers and parity checks passed.
The nine comparison UI tests passed, including a rendered synthetic screenshot with system-font
fallback disabled and a painted duplicate-widget-ID check. The opt-in UI control regression
passed in the workspace suite.

The WASM check remains unverified: its offline attempt failed fetching uncached
`bumpalo v3.20.3`, and `wasm32-unknown-unknown` is not installed. Dependency-advisory validation
was skipped by `xtask deny` because `cargo-deny` is not installed. No dependencies changed.

## Low-disk Cargo profile

When local build storage is constrained, select the opt-in `low-disk` profile explicitly:

```sh
cargo build --profile low-disk -p pdfcraft-cli
cargo test --profile low-disk -p pdfcraft-cos --lib
```

The profile inherits the normal development settings, including debug assertions and overflow
checks. It uses line-table-only debug information for workspace code, which keeps file and line
numbers in backtraces but omits local-variable and parameter details, and it disables incremental
compilation for this profile only. Ordinary `cargo build`, `cargo test`, and release builds keep
their existing profiles.

Cargo stores custom-profile artifacts separately under `target/low-disk`. Switching between the
normal and low-disk profiles can therefore leave both sets of artifacts on disk. To remove only
the low-disk artifacts, run `cargo clean --profile low-disk`.

Building PdfCraft requires Rust 1.95 or later: the UI crate and its `egui` and `craft-ui`
dependencies need it, and the workspace `rust-version` states the same minimum.

## Interface appearance

Display theme saves System, Light or Dark separately from the palette currently drawn. New
installations still default to Light. System follows native appearance events on macOS and
Windows. The native window keeps its inherited appearance in every mode; both egui style branches
use the selected application palette, so desktop changes cannot override a manual choice.

On Linux the native app starts an XDG Settings portal subscription when System is first selected.
It reads `org.freedesktop.appearance` / `color-scheme`, where numeric `1` requests Dark, `2` requests
Light, and `0` means no preference. When that states no preference or is missing, it falls back to
`org.gnome.desktop.interface` / `color-scheme` (`prefer-dark`, `prefer-light`, `default`), which
XFCE's portal answers instead of the freedesktop namespace; KDE answers only the freedesktop one. The subscription is installed before the initial read. Initial
results and later `SettingChanged` signals wake an idle window. There is no periodic desktop
polling, and signals received in a manual mode update the cache without requesting frames.

System falls back to Light if no appearance is available, including an absent portal, an
unsupported appearance backend, no preference, an unknown value or a failed connection. This
does not rewrite the saved System choice. The watcher cancels when its window closes; it does not
reconnect after a session-bus disconnection. Headless UI contexts use their supplied egui input and
do not contact a desktop session bus. Page and document colours retain their existing behavior.


## Logs

The desktop app writes its `log` records to standard error and to `logs/pdfcraft.log` in its
settings folder, next to `app.ron`: Linux and FreeBSD `~/.local/share/pdfcraft/logs/` (or
`$XDG_DATA_HOME/pdfcraft/logs/`), macOS `~/Library/Application Support/PdfCraft/logs/`, Windows
`%APPDATA%\PdfCraft\data\logs\`. In portable mode (a `portable.txt` file next to the executable, as
the Windows portable zip ships) the settings folder is `PdfCraftData\` beside the executable, and
crash recovery and new digital IDs go there too. A start from a desktop menu, Finder or the Start
menu has no terminal, so this file is what to attach to a bug report: a failed autosave, a page that
would not render, a render worker that could not start and the report of an internal error all land
there.
Each launch moves the previous log to `pdfcraft.1.log` (and that one to `pdfcraft.2.log`), so the
log of a run that crashed survives the next start. The file stops growing at 16 MiB. `--version`
writes no file.

By default PdfCraft's own crates (`pdfcraft*`) log at `info` and everything else at `warn`.
`RUST_LOG` replaces that with env_logger-style directives, for example `RUST_LOG=debug`,
`RUST_LOG=warn,pdfcraft_render=trace` or `RUST_LOG=info,wgpu_core=warn`; a directive ending in `*`
covers every target starting with it (`pdfcraft*=debug`). The logger is
`apps/pdfcraft/src/logging.rs`. It never records the control-channel token or document passwords.

## GPU surface limits

The desktop renderer enables the selected adapter's supported 2D texture extent, while keeping
egui-wgpu's other device requirements. Restored windows on mixed-DPI monitors can exceed its
default 8192-pixel extent even when the GPU supports a larger surface (#577). The enabled extent
never exceeds the adapter's capability. A window larger than even that (restored at another
monitor's scale, or stretched across monitors) gets a surface fitted within the limit and is drawn
at a correspondingly lower scale instead of panicking in `Surface::configure` (vendored egui-wgpu,
`surface_fit`; see `vendor/README.md`). On DX12, PdfCraft's Windows default, the surface is
stretched over the window: softer, but complete and lined up with the pointer. wgpu's GL backend
copies it unscaled into a corner of the window instead.

For blurry or incorrectly scaled UI reports, attach `pdfcraft.log`. Look for the successful
startup `renderer:` line, which records the wgpu adapter, device type, backend and enabled texture
limit, or the OpenGL renderer. Also look for the `drawing it at … and scaling it up` warning: it
means PdfCraft hit the surface-size safety fallback and rendered below the physical window size, so
softness is expected.

## Renderer fallback

PdfCraft draws with wgpu, and starts again with eframe's own OpenGL renderer (glow) when wgpu fails
before the app is created: no adapter, no device, or a window surface the device can't configure.
The last of these was a panic, not an error, because wgpu's default error handler panics and
eframe configures the surface before any app code runs; a 2015 Intel GPU whose driver timed out in
`Surface::configure` on wgpu's GL backend closed PdfCraft at launch (#519). Vendored egui-wgpu
configures a new window's surface inside `winit::catch_errors` and returns a validation,
out-of-memory or internal error as `WgpuError::ConfigureSurface` (see `vendor/README.md`). Still a
panic, as upstream: an error in a later reconfigure (resizing, a lost surface); a device lost
during that first configure (wgpu reports it only to a callback, so the first frame panics, after
the app has started); and a window shown at zero size, which is first configured on resize. If
OpenGL can't start either, PdfCraft exits with both errors in the log. The fallback is logged at
`error`, with the wgpu error. Renderer resource creation now uses the same scopes and returns
`WgpuError::CreateRenderer` before the app is created, so a rejected `egui_pipeline` can also
reach this retry instead of the uncaptured-error panic handler. This does not fix the failing
driver's shader compilation, and explicit `PDFCRAFT_RENDERER=wgpu` still disables the retry.
`PDFCRAFT_RENDERER` skips it or chooses OpenGL from the start.

## Environment variables

| Variable | Effect |
|---|---|
| `RUST_LOG` | Log levels for standard error and the log file (see [Logs](#logs)) |
| `RUST_BACKTRACE` | `1` adds a backtrace to the report of an internal error |
| `XDG_DATA_HOME` | Linux/FreeBSD: base of the settings folder (`pdfcraft/`), the log folder and crash recovery |
| `WGPU_POWER_PREF` | GPU choice; by default PdfCraft draws on the GPU the Wayland compositor draws with (it says so through linux-dmabuf feedback), else the GPU that drives the primary display (Windows) or the built-in panel (Linux), else the low-power (integrated) GPU. The log says which one was chosen |
| `WGPU_BACKEND` | Graphics backend; by default Windows uses Direct3D 12, falling back to OpenGL |
| `PDFCRAFT_RENDERER` | `gl` starts with OpenGL (glow) and never loads wgpu; `wgpu` reports a wgpu failure instead of retrying with OpenGL; unset, wgpu is retried with OpenGL when it can't start (see [Renderer fallback](#renderer-fallback)) |
| `CRAFT_FONTS_DIR` | Build time: a [craft-fonts](https://github.com/storytold/craft-fonts) checkout to embed (Japanese fonts) |
| `PDFCRAFT_SYSTEM_FONTS` | `0` stops the desktop app from using an installed font for characters its embedded fonts lack (`cargo xtask screenshots` sets it) |
