# Craft UI

> **Owned copy.** Copied on 2026-10-14 from [`storytold/craft-libs`](https://github.com/storytold/craft-libs) `crates/ui` at commit `5c874d51eeb3de70e735c8ce87a59a7dedd47675` (MIT OR Apache-2.0, like this project). It is maintained here and no longer tracks upstream; there is no git dependency on craft-libs.

`craft-ui` provides small egui components with application-supplied styles. `icons` prepares
embedded SVG catalogs; `buttons` paints accessible icon buttons; `tabs` handles tab fitting and
interaction; `layout` provides split geometry and resize handles.
`docking` supplies transactional panel moves and an optional docking surface. PdfCraft
(`crates/ui-egui`) is its only consumer. Assets, complete themes, localization and command
handling remain in the application. No icons, fonts or application engines are bundled.

## SVG catalogs

```rust
use std::sync::OnceLock;
use craft_ui::icons::{SvgIconSet, SvgStyle};

// Supply the application's own attributed assets here.
static ICONS: &[(&str, &[u8])] = &[];

fn icons() -> &'static SvgIconSet {
    static ICONS_SET: OnceLock<SvgIconSet> = OnceLock::new();
    ICONS_SET.get_or_init(|| SvgIconSet::new(
        ICONS,
        SvgStyle { current_color: "#ffffff", stroke_width: Some("1.75") },
        "bytes://my-app/icons/",
        "square",
    ))
}

fn icon(name: &str) -> egui::Image<'static> {
    egui::Image::new(icons().source(name))
}
```

The application installs `egui_extras::install_image_loaders` with its `svg` feature before drawing.
It sets tint and size on the returned image.
The library does not install loaders or an egui theme.

The catalog is immutable. SVGs are prepared once and held in `Arc<[u8]>`; lookups share those bytes.
The fallback uses the requested name in its URI, matching PhotoCraft and VectorCraft. Missing both
the requested name and fallback produces empty bytes for the loader to report as an error, without
indexed lookups or a panic. `contains` and `canonical_name` inspect only actual catalog entries. PdfCraft resolves an unknown
name to `square` before calling `source`, preserving its fallback cache key.

Use a different URI prefix for every catalog/style combination used in the same egui context.
egui caches by URI; supplying different bytes under one URI would reuse stale content. Apps with
their own contexts can retain their current prefixes. Unknown names do not add entries to this
catalog, though the egui loader may cache a requested URI.

This is an adapter for trusted, compiled-in monochrome assets, not an SVG sanitizer. It performs
literal `currentColor` and optional `stroke-width="2"` substitutions, keeps other widths and explicit
colors untouched, and uses the existing lossy UTF-8 behavior. General SVG parsing and rasterization
remain the responsibility of the existing loader.

## Icon buttons

`buttons::IconButton` accepts a label, size and `IconButtonStyle`, and returns `egui::Response`.
`ButtonVisuals` describes fill, border and icon tint for normal, hovered, pressed and selected
states. Selection wins over press and hover. `pressed: None` keeps ordinary hover/normal styling;
`Some` supplies a separate press treatment. egui owns pointer capture and click cancellation.

```rust
use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};
use egui::{Color32, CornerRadius, Stroke, Vec2};

fn layer_button(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let normal = ButtonVisuals {
        fill: Color32::TRANSPARENT, stroke: Stroke::NONE, icon: Color32::WHITE,
    };
    let style = IconButtonStyle {
        normal,
        hovered: ButtonVisuals { fill: Color32::DARK_GRAY, ..normal },
        pressed: None,
        selected: ButtonVisuals { fill: Color32::BLUE, ..normal },
        corner_radius: CornerRadius::same(4),
        focus_stroke: Stroke::new(1.0, Color32::LIGHT_BLUE),
    };
    IconButton::new("Layers", Vec2::splat(28.0), &style).selected(selected)
        .show(ui, |ui, rect, tint| {
            // Use the application's SVG catalog or its existing painter icon here.
            ui.painter().circle_filled(rect.center(), 5.0, tint);
        })
}
```

Use actual theme colors in the style adapter and localized text for the label. The painter receives
the full button rectangle and icon tint; the app chooses icon padding/size. Parent disabled state
applies to painting, interaction and accessibility. The library reports label and selection to
AccessKit and draws the supplied inset focus outline. Mouse, Enter and Space activation come from
egui. The caller reads `clicked()`, runs its command and updates selection; the component neither
dispatches nor toggles state. Attach tooltips to the returned response.

IDs come from the surrounding UI scope/position, independent of the translated label. Use
`ui.push_id(stable_id, ...)` when items reorder. Dimensions must be finite, positive and no larger
than 4096 points; other sizes produce an inert zero-size response. Invalid stroke widths are
ignored and large strokes are bounded to half the smaller button dimension.

## Tabs

`tabs::fit(natural_widths, selected, available, minimum, overflow_width)` returns visible tab
indices and widths, overflow indices, and the actual reserved overflow width. It shrinks wide
tabs before hiding them, keeps input order and retains the selected tab. Tiny strips may reduce
its width to zero. Negative/non-finite dimensions become zero and large dimensions are bounded.
The work is linear in tab count with at most 32 fitting passes; the caller caps its own tab count.

`tabs::Tab` registers a tab at a supplied rectangle and invokes the application's painter:

```rust
fn tab(ui: &mut egui::Ui, rect: egui::Rect, selected: bool) -> egui::Response {
    craft_ui::tabs::Tab::new(ui.id().with("layers-tab"), "Layers", selected)
        .sense(egui::Sense::click_and_drag())
        .show_at(ui, rect, |ui, response| {
            let fill = if response.hovered() { egui::Color32::GRAY } else { egui::Color32::DARK_GRAY };
            ui.painter().rect_filled(response.rect, 0.0, fill);
        })
}
```

Supply a stable ID independent of the translated label. The component reports the accessible
Tab role, label and selection, handles egui keyboard activation, and paints a focus outline from
the caller's theme. The caller keeps selection, labels, overflow menus, painting and drag actions;
close buttons and tab context menus stay separate. Dock and floating-panel adapters use the same
component without sharing window or workspace state.

## Splits and resize handles

`layout::split_rect` divides a rectangle along `SplitAxis::Horizontal` (left/right children) or
`Vertical` (top/bottom). `SplitSize` accepts a ratio, fixed first pane, or fixed second pane in
logical points. The result contains `first`, `divider` and `second` rectangles. Gap and minimum
are supplied by the application. Tiny areas relax the minimum to half the space available;
non-finite or inverted input returns `None` instead of panicking.

```rust
use craft_ui::layout::{split_rect, SplitAxis, SplitSize};

fn two_panes(rect: egui::Rect) -> Option<(egui::Rect, egui::Rect)> {
    let split = split_rect(rect, SplitAxis::Horizontal, SplitSize::Ratio(0.5), 4.0, 20.0)?;
    Some((split.first, split.second))
}
```

`layout::ResizeHandle` receives a stable ID, axis, divider rectangle, localized label and
`ResizeHandleStyle` colors. `show` returns an egui response and a movement delta for that frame.
Pointer drags, focused axis-arrow keys and AccessKit increment/decrement actions all produce
that delta. `value(current, range)` adds accessible numeric bounds and SetValue support. Tab
continues normal focus traversal. Invalid geometry or numeric parameters make the handle inert.

Apply the delta through `SplitLayout::resized(size, delta, minimum)` to retain fixed/ratio sizing,
or use an application-specific adapter. The library does not persist geometry, dispatch commands
or create panes. Apps keep their existing dock trees, stacked panels, layout files, pane contents,
cameras, document focus and native windows. Shared controls do not constitute a dock-tree migration.

## Panel docking

`docking::Layout<P>` owns panel placement independently of application content: a tree of split,
tab and accordion nodes plus floating tab groups. `apply(Action)` validates the complete layout,
applies a proposed change to a copy and commits only a valid result. Moves, tab insertion/reorder,
side splits, detach, activate, close, reveal and resizing use this one transaction path. Panel IDs
must be unique, stable values; labels and document state are supplied by the app.

`Layout::location` captures a panel's placement before hiding it; `restore` returns it there if
the remembered destination still exists. Apps choose an explicit fallback when it does not.
Enable the optional `serde` feature to persist the shared layout, or convert to/from an existing
workspace schema. Validate loaded input before rendering or recursively converting it.

`DockArea::show_with_limits` is an optional renderer. It takes a layout snapshot, styles, labels,
panel permissions, size limits and a body callback, then returns actions for the app's command
handler. Its floating groups stay
inside the current egui viewport. Protected document canvases have no tab header. Per-panel size
limits constrain geometry without overwriting saved preferred widths when the window shrinks.
Applications with established chrome can use the shared transaction model and `resolve_drop`
without changing their renderer.

## Compatibility and checks

The crate uses egui 0.36 and Rust 1.95 or later. Its production dependency disables egui's default
features so fonts and rendering features remain the application's choice.

Validation:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --target wasm32-unknown-unknown -p craft-ui
```

The visual regression suite (`tests/visual_regression.rs`) compares the actual controls
against independent reference painters, including exact mesh and RGBA checks, and proves that
color, spacing and stroke changes fail those comparisons. To retain reviewable PNG pairs:

```sh
CRAFT_UI_VISUAL_DIR=/absolute/path/to/visuals cargo test -p craft-ui --test visual_regression
```
