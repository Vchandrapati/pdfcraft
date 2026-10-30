//! Exact offscreen rendering comparisons against independent reference painters.
//!
//! These are paired same-renderer comparisons, not portable GPU golden files. The reference
//! paths deliberately do not call the shared components. Literal geometry and colors specify
//! the baseline; deliberate mutations prove color, spacing and stroke regressions are caught.
//! Set CRAFT_UI_VISUAL_DIR to retain bounded before/after/difference PNGs and comparison tables.

#[path = "visual/fixtures.rs"]
mod fixtures;

use std::path::PathBuf;

use egui::{Color32, Context, Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Rect, Response, TextureId, Ui};
use egui_kittest::{TestRenderer, wgpu::WgpuTestRenderer};
use fixtures::{CANVAS, Case, Component, Mutation, State, Theme};
use image::{Rgba, RgbaImage};

#[derive(Debug, PartialEq)]
struct Mesh {
    clip: Rect,
    texture: TextureId,
    indices: Vec<u32>,
    vertices: Vec<(Pos2, Pos2, Color32)>,
}

struct Capture {
    image: RgbaImage,
    meshes: Option<Vec<Mesh>>,
}

fn context(theme: Theme, scale: f32) -> Context {
    let ctx = Context::default();
    ctx.set_theme(theme.egui());
    ctx.set_pixels_per_point(scale);
    ctx.set_fonts(egui::FontDefinitions::empty());
    ctx.all_styles_mut(|style| {
        style.animation_time = 0.0;
        style.visuals.text_cursor.blink = false;
        style.visuals.disabled_alpha = 0.4;
    });
    ctx
}

fn raw_input(frame: usize, events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, CANVAS)),
        time: Some(frame as f64 / 60.0),
        events,
        max_texture_side: Some(2048),
        ..Default::default()
    }
}

fn background(ui: &mut Ui, theme: Theme) {
    let screen = Rect::from_min_size(Pos2::ZERO, CANVAS);
    ui.set_clip_rect(screen);
    ui.painter().rect_filled(screen, 0.0, theme.surface());
}

fn finish_capture(renderer: &mut WgpuTestRenderer, ctx: &Context, output: &FullOutput) -> Capture {
    let meshes = ctx
        .tessellate(output.shapes.clone(), output.pixels_per_point)
        .into_iter()
        .map(|clipped| {
            let egui::epaint::Primitive::Mesh(mesh) = clipped.primitive else { panic!("visual fixtures must not contain paint callbacks") };
            Mesh {
                clip: clipped.clip_rect,
                texture: mesh.texture_id,
                indices: mesh.indices,
                vertices: mesh.vertices.into_iter().map(|vertex| (vertex.pos, vertex.uv, vertex.color)).collect(),
            }
        })
        .collect();
    let image = renderer.render(ctx, output).expect("offscreen renderer is required for the visual suite");
    assert!(image.width() > 0 && image.height() > 0 && image.width() <= 256 && image.height() <= 144);
    Capture { image, meshes: Some(meshes) }
}

fn capture(renderer: &mut WgpuTestRenderer, case: Case, reference: bool, mutation: Mutation) -> Capture {
    let ctx = context(case.theme, case.scale);
    let mut output = FullOutput::default();
    let mut responses: Vec<Response> = Vec::new();
    for frame in 0..4 {
        let mut events = Vec::new();
        if !reference && frame == 2 {
            if case.state == State::Focus {
                ctx.memory_mut(|memory| memory.request_focus(responses[0].id));
            }
            if case.state.pointer() {
                let center = case.control_rect().center();
                events.push(Event::PointerMoved(center));
                if case.state.pressed() {
                    events.push(Event::PointerButton { pos: center, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
                }
            }
        }
        output = ctx.run_ui(raw_input(frame, events), |ui| {
            background(ui, case.theme);
            if reference {
                fixtures::reference(ui, case);
            } else {
                responses = fixtures::actual(ui, case, mutation);
            }
        });
        renderer.handle_delta(&mut output.textures_delta);
    }
    assert_eq!(output.pixels_per_point, case.scale);
    if !reference {
        let response = &responses[0];
        assert_eq!(response.enabled(), case.state.enabled(), "{case:?}");
        if case.state == State::Hover {
            assert!(response.hovered(), "hover was not exercised: {case:?}");
        }
        if case.state.pressed() {
            assert!(response.is_pointer_button_down_on(), "press was not exercised: {case:?}");
            if case.component.is_split() {
                assert!(response.dragged(), "divider drag was not exercised: {case:?}");
            }
        }
        if case.state == State::Focus {
            assert!(response.has_focus(), "focus was not exercised: {case:?}");
        }
        if case.state == State::Disabled {
            assert!(!response.hovered() && !response.has_focus(), "disabled pointer input must remain inert: {case:?}");
        }
    }
    finish_capture(renderer, &ctx, &output)
}

struct Artifacts {
    directory: Option<PathBuf>,
    rows: String,
}

impl Artifacts {
    fn new(suite: &str) -> Self {
        let directory = std::env::var_os("CRAFT_UI_VISUAL_DIR").map(|root| PathBuf::from(root).join(suite));
        if let Some(directory) = &directory {
            std::fs::create_dir_all(directory).unwrap();
        }
        Self { directory, rows: "case\twidth\theight\tchanged_pixels\tmesh_equal\texpected_change\n".into() }
    }

    fn record(&mut self, name: &str, before: &Capture, after: &Capture, expected_change: bool) -> usize {
        assert_eq!(before.image.dimensions(), after.image.dimensions(), "{name}");
        let mut changed = 0;
        let mut difference = RgbaImage::new(before.image.width(), before.image.height());
        for ((before, after), pixel) in before.image.pixels().zip(after.image.pixels()).zip(difference.pixels_mut()) {
            if before != after {
                changed += 1;
                *pixel = Rgba([255, 0, 128, 255]);
            }
        }
        let mesh_equal = match (&before.meshes, &after.meshes) {
            (Some(before), Some(after)) => (before == after).to_string(),
            (None, None) => "not_applicable".into(),
            _ => panic!("both captures must have the same comparison scope"),
        };
        self.rows.push_str(&format!("{name}\t{}\t{}\t{changed}\t{mesh_equal}\t{expected_change}\n", before.image.width(), before.image.height()));
        if let Some(directory) = &self.directory {
            before.image.save(directory.join(format!("{name}-before.png"))).unwrap();
            after.image.save(directory.join(format!("{name}-after.png"))).unwrap();
            difference.save(directory.join(format!("{name}-difference.png"))).unwrap();
            // Preserve completed rows even if a later assertion fails.
            std::fs::write(directory.join("comparison.tsv"), &self.rows).unwrap();
        }
        changed
    }
}

#[test]
fn rendered_controls_match_independent_reference_pixels_and_meshes() {
    let mut renderer = WgpuTestRenderer::new();
    let mut artifacts = Artifacts::new("controls");
    let mut count = 0;
    for component in Component::ALL {
        for state in State::ALL {
            if component.is_split() && state.selected() {
                continue; // A resize handle has no selected state in its public contract.
            }
            for theme in [Theme::Light, Theme::Dark] {
                for scale in [1.0, 1.5, 2.0] {
                    for compact in [false, true] {
                        let case = Case { component, state, theme, scale, compact };
                        let before = capture(&mut renderer, case, true, Mutation::None);
                        let after = capture(&mut renderer, case, false, Mutation::None);
                        let changed = artifacts.record(&case.name(), &before, &after, false);
                        assert_eq!(before.meshes, after.meshes, "reference mesh mismatch: {case:?}");
                        assert_eq!(changed, 0, "reference pixels changed: {case:?}");
                        count += 1;
                    }
                }
            }
        }
    }
    assert_eq!(count, 288);
    eprintln!("{count} control before/after pairs matched exact RGBA pixels and tessellated meshes");
}

#[test]
fn visual_comparisons_detect_color_spacing_and_stroke_mutations() {
    let mut renderer = WgpuTestRenderer::new();
    let mut artifacts = Artifacts::new("negative-controls");
    let mut count = 0;
    for theme in [Theme::Light, Theme::Dark] {
        for scale in [1.0, 1.5, 2.0] {
            for compact in [false, true] {
                let case = Case { component: Component::Button, state: State::Normal, theme, scale, compact };
                let before = capture(&mut renderer, case, true, Mutation::None);
                for mutation in [Mutation::Color, Mutation::Spacing, Mutation::Stroke] {
                    let after = capture(&mut renderer, case, false, mutation);
                    let changed = artifacts.record(&format!("{}-{mutation:?}", case.name()), &before, &after, true);
                    assert_ne!(before.meshes, after.meshes, "mesh comparison missed {mutation:?}: {case:?}");
                    assert!(changed > 0, "pixel comparison missed {mutation:?}: {case:?}");
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 36);
    eprintln!("{count} deliberate color/spacing/stroke changes were detected in both pixels and meshes");
}

#[test]
fn svg_preparation_matches_literal_raster_references_at_all_scales() {
    use craft_ui::icons::{SvgIconSet, SvgStyle};
    use egui::{
        ImageSource,
        load::{Bytes, ImagePoll, SizeHint},
    };

    const SOURCE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="4" y="4" width="16" height="16"/><path d="M4 4L20 20"/></svg>"#;
    const LIGHT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#0f172a" stroke-width="1.75"><rect x="4" y="4" width="16" height="16"/><path d="M4 4L20 20"/></svg>"##;
    const DARK: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#e2e8f0" stroke-width="1.75"><rect x="4" y="4" width="16" height="16"/><path d="M4 4L20 20"/></svg>"##;
    fn raster(ctx: &Context, uri: &str, size: u32) -> Capture {
        let ImagePoll::Ready { image } = ctx.try_load_image(uri, SizeHint::Size { width: size, height: size, maintain_aspect_ratio: true }).unwrap()
        else {
            panic!("compiled-in SVG must rasterize synchronously")
        };
        assert_eq!(image.size, [size as usize; 2]);
        let image = RgbaImage::from_raw(size, size, image.pixels.iter().flat_map(|pixel| pixel.to_srgba_unmultiplied()).collect()).unwrap();
        assert!(image.pixels().any(|pixel| pixel[3] != 0), "blank SVG is not a valid matching baseline");
        Capture { image, meshes: None }
    }
    let mut artifacts = Artifacts::new("svg");
    for (theme, color, literal) in [(Theme::Light, "#0f172a", LIGHT), (Theme::Dark, "#e2e8f0", DARK)] {
        for (scale, numerator) in [(1.0, 2), (1.5, 3), (2.0, 4)] {
            for logical_size in [12, 24] {
                let ctx = Context::default();
                egui_extras::install_image_loaders(&ctx);
                ctx.include_bytes("bytes://literal-reference.svg", Bytes::Static(literal));
                let icons =
                    SvgIconSet::new(&[("box", SOURCE)], SvgStyle { current_color: color, stroke_width: Some("1.75") }, "bytes://actual/", "box");
                let ImageSource::Bytes { uri, bytes } = icons.source("box") else { panic!("catalog must return embedded bytes") };
                ctx.include_bytes(uri.clone(), bytes);
                let size = logical_size * numerator / 2;
                let before = raster(&ctx, "bytes://literal-reference.svg", size);
                let after = raster(&ctx, &uri, size);
                let name = format!("svg-{}-{scale}x-{logical_size}pt", theme.name());
                assert_eq!(artifacts.record(&name, &before, &after, false), 0, "{name}");
            }
        }
    }
}
