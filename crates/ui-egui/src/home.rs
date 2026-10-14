//! Home tab: quick tools, pinned folders and recent files (local only, never
//! another app's list).

use egui::{Align2, CornerRadius, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::catalog;

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, panels::human_size, widgets};

const QUICK_TOOLS: [(&str, &str, &str, &str); 4] = [
    ("Rotate", "rotate-cw", "page.rotate_dialog", "organize"),
    ("Merge", "files", "page.combine", "combine"),
    ("Split", "scissors", "page.split", "organize"),
    ("Reorder", "layout-grid", "page.organize", "organize"),
];

/// A soft shadow that rises on hover and settles when the card is pressed.
fn card_lift(ui: &egui::Ui, t: &Tokens, rect: Rect, press: theme::Press) {
    let lift = press.hover * (1.0 - press.down);
    if lift > 0.04 {
        let shadow = egui::Shadow { offset: [0, 3], blur: (14.0 * lift) as u8, spread: 0, color: t.page_shadow.gamma_multiply(0.85 * lift) };
        ui.painter().add(shadow.as_shape(rect, CornerRadius::same(10)));
    }
}

pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 36, right: 36, top: 28, bottom: 28 }).show(ui, |ui| {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::same(18))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new(tl!("Quick tools")).font(theme::semibold(15.5)));
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(14.0, 14.0);
                        for (label, icon, command, group) in QUICK_TOOLS {
                            let Some(group) = catalog::group(group) else { continue };
                            let (rect, resp) = ui.allocate_exact_size(vec2(190.0, 70.0), Sense::click());
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!(label)));
                            let press = theme::Press::track(ui, &resp);
                            card_lift(ui, &t, rect, press);
                            let fill = press.fill(t.card, t.hover, t.pressed);
                            ui.painter().rect(rect, CornerRadius::same(10), fill, Stroke::new(1.0, t.divider), egui::StrokeKind::Inside);
                            let body = rect.translate(press.offset());
                            icons::paint(
                                ui,
                                Rect::from_center_size(body.left_center() + vec2(25.0, 0.0), vec2(22.0, 22.0)),
                                icon,
                                21.0,
                                egui::Color32::from_rgb(group.hue[0], group.hue[1], group.hue[2]),
                            );
                            let mut title_job = egui::text::LayoutJob::simple_singleline(tl!(label).to_owned(), theme::semibold(13.5), t.text);
                            title_job.wrap =
                                egui::text::TextWrapping { max_width: body.width() - 58.0, max_rows: 1, break_anywhere: true, ..Default::default() };
                            let title = ui.fonts_mut(|f| f.layout_job(title_job));
                            let title_elided = title.elided;
                            ui.painter().galley(body.left_center() + vec2(44.0, -title.size().y * 0.5), title, t.text);
                            let resp = if title_elided { resp.on_hover_text(tl!(label)) } else { resp };
                            if theme::hand(resp).clicked() {
                                if app.active.is_none() && command != "page.combine" {
                                    if let Some(spec) = pdfcraft_engine::commands::command(command) {
                                        app.open_then_run(spec);
                                    }
                                } else {
                                    app.run_command(command);
                                }
                            }
                        }
                    });
                });

            #[cfg(not(target_arch = "wasm32"))]
            {
                ui.add_space(26.0);
                crate::folders_ui::section(app, ui);
            }

            ui.add_space(26.0);
            let mut clear = false;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Recent")).font(theme::semibold(17.0)));
                if !app.recent.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let resp = widgets::ghost_button(ui, "trash-2", tl!("Clear"));
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Clear Recent Files")));
                        clear = resp.on_hover_text(tl!("Clear Recent Files")).clicked();
                    });
                }
            });
            ui.add_space(8.0);
            if app.recent.is_empty() {
                ui.label(egui::RichText::new(tl!("Files you open in PdfCraft appear here. Drop a PDF anywhere to open it.")).color(t.text_muted));
            }
            let mut open = None;
            let mut remove = None;
            for r in &app.recent {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &r.name));
                let press = theme::Press::track(ui, &resp);
                // The row, not its response: the pointer stays "in" the row over its remove button.
                let hovered = ui.rect_contains_pointer(rect);
                if hovered && press.hover < 0.04 && press.down < 0.04 {
                    // The remove button sits in the row, so the row response may not be hovered.
                    ui.painter().rect_filled(rect, CornerRadius::same(8), t.hover);
                } else {
                    press.wash(ui, rect, 8, false);
                }
                if hovered {
                    // Remove just this file from the list (#430); the file itself is untouched.
                    let x_rect = Rect::from_center_size(rect.right_center() - vec2(26.0, 0.0), vec2(28.0, 28.0));
                    let x = ui.interact(x_rect, ui.id().with(("remove-recent", &r.path)), Sense::click());
                    let label = crate::i18n::fmt(tl!("Remove {name} from Recent"), &[("name", &r.name)]);
                    x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
                    if x.hovered() {
                        ui.painter().rect_filled(x_rect, CornerRadius::same(6), t.pressed);
                    }
                    icons::paint(ui, x_rect.shrink(6.0), "x", 16.0, t.icon);
                    if x.on_hover_text(tl!("Remove from Recent")).clicked() {
                        remove = Some(r.path.clone());
                    }
                }
                let body = rect.translate(press.offset());
                icons::paint(
                    ui,
                    Rect::from_min_size(body.min + vec2(10.0, 11.0), vec2(24.0, 24.0)),
                    "file-text",
                    22.0,
                    egui::Color32::from_rgb(0xE0, 0x3E, 0x3E),
                );
                // Room on the right for the remove button.
                let detail = ui.painter().text(
                    body.right_center() - vec2(48.0, 0.0),
                    Align2::RIGHT_CENTER,
                    format!("{}  ·  {}", crate::i18n::trn(crate::i18n::current(), r.pages as u64, "{n} page", "{n} pages"), human_size(r.size)),
                    theme::regular(12.0),
                    t.text_muted,
                );
                // The name and path stop short of the page count; hovering shows the whole path.
                let width = detail.left() - 16.0 - (body.left() + 46.0);
                widgets::row_text(ui, body.min + vec2(46.0, 15.0), crate::bidi::visual(&r.name), theme::medium(13.5), t.text, width);
                widgets::row_text(ui, body.min + vec2(46.0, 32.0), crate::bidi::visual(&r.path), theme::regular(11.0), t.text_faint, width);
                if theme::hand(resp).on_hover_text(&r.path).clicked() {
                    open = Some(r.path.clone());
                }
            }
            if let Some(p) = open {
                app.open_recent(&p);
            }
            if let Some(p) = remove {
                app.recent.retain(|r| r.path != p);
            }
            if clear {
                app.execute("file.clear_recent");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translated_quick_tool_titles_fit_the_cards() {
        let de = crate::i18n::Lang::from_code("de").unwrap();
        crate::i18n::set_current(de);
        let ctx = egui::Context::default();
        ctx.set_fonts(theme::font_definitions());
        let mut app = PdfCraftApp::new();
        let output = ctx
            .run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1440.0, 900.0))), ..Default::default() }, |ui| {
                show(&mut app, ui)
            });
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let text_shapes: Vec<_> =
            output.shapes.iter().filter_map(|shape| if let egui::Shape::Text(text) = &shape.shape { Some(text) } else { None }).collect();
        for (label, _, _, _) in QUICK_TOOLS {
            let title = text_shapes.iter().find(|text| text.galley.job.text == crate::i18n::tr(de, label)).unwrap();
            assert_eq!(title.galley.rows.len(), 1);
            assert!(!title.galley.elided, "{label} must fit its quick-tool card");
        }
        output.drop_without_applying_deltas();
    }
}
