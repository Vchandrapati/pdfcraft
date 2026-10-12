//! Compare files in the real shell (egui_kittest): the dialog, the Compare panel, marks and
//! the report.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Session;
use pdfcraft_ui_egui::{PdfCraftApp, RightPanel};

#[test]
fn compare_two_versions() {
    let s = Session::new();
    let v1 = s.create_from_text("t", "Delivery within five days. Returns accepted.").unwrap().to_vec();
    let v2 = s.create_from_text("t", "Delivery within three days. Returns accepted for a week.").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("v1.pdf", None, v1.clone()).unwrap();
        app.open_bytes("v2.pdf", None, v2.clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("doc.compare"));
    h.run_steps(2);
    h.get_by_label("Compare Files");
    h.get_by_label("Compare").click();
    h.run_steps(3);
    assert_eq!(h.state().right, Some(RightPanel::Compare));
    assert_eq!(h.state().toast.clone().unwrap().0, "2 differences found");
    h.get_by_label("2 Replaced");
    h.get_by_label("\"five\" → \"three\"").click();
    h.run_steps(2);
    assert_eq!(h.state().compare.as_ref().unwrap().selected, Some(0));
    assert!(h.state().compare.as_ref().unwrap().side_by_side);
    assert_eq!(h.state().views[0].compare_marks.len(), 2, "older text is highlighted too");
    let id = h.state().active_ids().unwrap().1;
    let i = h.state().active_ids().unwrap().0;
    assert_eq!(h.state().views[i].compare_marks.len(), 2);
    h.get_by_label("Mark as comments").click();
    h.run_steps(3);
    assert_eq!(h.state().session.get(id).unwrap().info.annotations.len(), 2);
    h.get_by_label("Report…").click();
    h.run_steps(3);
    assert_eq!(h.state().session.get(h.state().active_ids().unwrap().1).unwrap().name, "Compare Report.pdf");
}

fn paired_harness() -> Harness<'static, PdfCraftApp> {
    let old = Session::new().create_blank(200.0, 300.0, 6).unwrap().to_vec();
    let new = Session::new().create_blank(240.0, 360.0, 4).unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 900.0)).build_eframe(move |cc| {
        cc.egui_ctx.options_mut(|options| options.warn_on_id_clash = true);
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("old.pdf", None, old.clone()).unwrap();
        app.open_bytes("new.pdf", None, new.clone()).unwrap();
        app.compare_old = Some(app.views[0].id);
        app.run_compare();
        app
    });
    h.run_steps(6);
    h
}

fn top_position(view: &pdfcraft_ui_egui::canvas::DocView) -> (usize, f32) {
    let top = view.viewport_rect().top();
    let (page, rect) = view.visible_page_rects().into_iter().find(|(_, rect)| rect.bottom() > top).unwrap();
    (page, (top - rect.top()) / rect.height())
}

#[test]
fn side_by_side_links_scroll_zoom_and_keeps_both_renderers_alive() {
    let mut h = paired_harness();
    fn no_duplicate_ids(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().all(no_duplicate_ids),
            egui::Shape::Text(text) => !text.galley.job.text.contains("use of widget ID"),
            _ => true,
        }
    }
    assert!(h.output().shapes.iter().all(|shape| no_duplicate_ids(&shape.shape)), "comparison panes must not duplicate widget IDs");
    let old = h.state().views[0].viewport_rect();
    let new = h.state().views[1].viewport_rect();
    assert!(old.is_positive() && new.is_positive() && old.right() < new.left());
    // A precise wheel over the older pane also gives it keyboard/toolbar ownership.
    h.event(egui::Event::PointerMoved(old.center()));
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -180.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(6);
    assert_eq!(h.state().active, Some(0));
    let a = top_position(&h.state().views[0]);
    let b = top_position(&h.state().views[1]);
    assert_eq!(a.0, b.0);
    assert!((a.1 - b.1).abs() < 0.005, "{a:?} versus {b:?}");
    h.state_mut().set_option("zoom", "125").unwrap();
    h.run_steps(6);
    assert!((h.state().views[0].zoom - 1.25).abs() < 0.001);
    assert!((h.state().views[1].zoom - 1.25).abs() < 0.001);
    h.state_mut().set_option("compare-focus", "new").unwrap();
    h.state_mut().set_option("page", "3").unwrap();
    h.run_steps(6);
    assert_eq!(h.state().views[0].current, 2);
    assert_eq!(h.state().views[1].current, 2);
    for _ in 0..80 {
        h.run_steps(2);
        if h.state().views.iter().all(|v| v.raster_memory().page_count > 0) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(h.state().views.iter().all(|v| v.raster_memory().page_count > 0), "both visible panes retain page rasters");
    h.state_mut().set_option("compare-sync", "off").unwrap();
    h.state_mut().set_option("page", "1").unwrap();
    h.state_mut().set_option("zoom", "150").unwrap();
    h.run_steps(6);
    assert_eq!(h.state().views[0].current, 2, "unlinked peer keeps its page");
    assert!((h.state().views[0].zoom - 1.25).abs() < 0.001);
    assert!((h.state().views[1].zoom - 1.5).abs() < 0.001);
}

#[test]
fn paired_view_focus_edit_undo_and_lifecycle_use_existing_workflows() {
    let mut h = paired_harness();
    let old_id = h.state().views[0].id;
    let new_id = h.state().views[1].id;
    let p = h.state().views[0].viewport_rect().center();
    h.event(egui::Event::PointerMoved(p));
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    assert_eq!(h.state().active_ids().unwrap().1, old_id);
    h.state_mut().execute("page.insert_blank");
    h.run_steps(4);
    assert_eq!(h.state().session.get(old_id).unwrap().info.pages.len(), 7);
    assert_eq!(h.state().session.get(new_id).unwrap().info.pages.len(), 4);
    h.state_mut().execute("edit.undo");
    h.run_steps(4);
    assert_eq!(h.state().session.get(old_id).unwrap().info.pages.len(), 6);
    // Reports still target the comparison pair when its old document owns focus.
    h.get_by_label("Report…").click();
    h.run_steps(4);
    assert_eq!(h.state().session.get(h.state().active_ids().unwrap().1).unwrap().name, "Compare Report.pdf");
    let report = h.state().views.last().unwrap().viewport_rect();
    assert!(report.width() > p.x, "an unrelated tab has one full-width canvas");
    h.state_mut().set_option("compare-focus", "new").unwrap();
    h.state_mut().set_option("mode", "read").unwrap();
    h.run_steps(4);
    assert!(h.state().views[0].viewport_rect().right() < h.state().views[1].viewport_rect().left());
    h.state_mut().execute("page.organize");
    h.run_steps(4);
    assert!(h.state().views[1].organize);
    assert!(h.state().views[1].viewport_rect().width() > report.width() / 2.0);
    h.state_mut().set_option("compare-focus", "old").unwrap();
    h.run_steps(4);
    assert!(h.state().views[0].viewport_rect().width() > report.width() / 2.0, "the inactive organizer must not reappear as a comparison pane");
    h.state_mut().views[1].organize = false;
    h.run_steps(4);
    assert!(h.state().compare.as_ref().unwrap().side_by_side);
    h.state_mut().close_tab(0);
    h.run_steps(4);
    assert!(h.state().compare.is_none());
    assert!(h.state().views.iter().all(|v| v.compare_marks.is_empty()));
}

#[test]
fn pointer_and_undo_on_entering_split_target_the_clicked_pane() {
    let mut h = paired_harness();
    let old_id = h.state().views[0].id;
    let new_id = h.state().views[1].id;
    h.state_mut().set_option("compare-view", "single").unwrap();
    for pane in ["old", "new"] {
        h.state_mut().set_option("compare-focus", pane).unwrap();
        h.state_mut().execute("page.insert_blank");
        h.run_steps(4);
    }
    // Both remembered viewports are full-width, as when the pair first becomes visible.
    assert!(h.state().views[0].viewport_rect().intersects(h.state().views[1].viewport_rect()));
    let full = h.state().views[1].viewport_rect();
    let p = egui::pos2(full.left() + full.width() * 0.25, full.center().y);
    h.state_mut().set_option("compare-view", "side-by-side").unwrap();
    h.event(egui::Event::PointerMoved(p));
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.event(egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
    h.step();
    assert_eq!(h.state().active_ids().unwrap().1, old_id);
    assert_eq!(h.state().session.get(old_id).unwrap().info.pages.len(), 6);
    assert_eq!(h.state().session.get(new_id).unwrap().info.pages.len(), 5, "new pane's edit must not be undone");
}

#[test]
fn toggling_side_by_side_preserves_scrolled_position_and_shorter_peer_is_clamped() {
    let mut h = paired_harness();
    h.state_mut().set_option("compare-focus", "old").unwrap();
    h.state_mut().set_option("zoom", "100").unwrap();
    h.state_mut().set_option("page", "6").unwrap();
    h.run_steps(6);
    assert_eq!(h.state().views[0].current, 5);
    assert_eq!(h.state().views[1].current, 3);
    // Away from the document end, changing viewport height need not clamp its scroll offset.
    h.state_mut().set_option("page", "3").unwrap();
    h.run_steps(6);
    let before = top_position(&h.state().views[0]);
    h.state_mut().set_option("compare-view", "single").unwrap();
    h.run_steps(4);
    let single = top_position(&h.state().views[0]);
    h.state_mut().set_option("compare-view", "side-by-side").unwrap();
    h.run_steps(6);
    let after = top_position(&h.state().views[0]);
    assert_eq!((before.0, single.0, after.0), (before.0, before.0, before.0));
    assert!((before.1 - single.1).abs() < 0.01 && (before.1 - after.1).abs() < 0.01, "{before:?}, {single:?}, {after:?}");
    h.get_by_label("Clear").click();
    h.run_steps(4);
    assert!(h.state().compare.is_none());
}

#[test]
fn selecting_a_difference_keeps_corresponding_pages_after_an_insertion() {
    let source = Session::new();
    let old = source.create_from_text("t", "Delivery within five days.").unwrap().to_vec();
    let new = source.create_from_text("t", "Delivery within three days.").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("old.pdf", None, old.clone()).unwrap();
        app.open_bytes("new.pdf", None, new.clone()).unwrap();
        app.apply_edit(pdfcraft_engine::Edit::InsertBlankPage { at: 0, width: 595.0, height: 842.0 });
        app.compare_old = Some(app.views[0].id);
        app.run_compare();
        app
    });
    h.run_steps(6);
    h.get_by_label("\"five\" → \"three\"").click();
    h.run_steps(6);
    assert_eq!((h.state().views[0].current, h.state().views[1].current), (0, 1));
    h.state_mut().set_option("compare-focus", "old").unwrap();
    h.state_mut().set_option("zoom", "125").unwrap();
    h.run_steps(6);
    assert_eq!((h.state().views[0].current, h.state().views[1].current), (0, 1), "linked zoom must preserve the selected correspondence");
}

/// Writes a screenshot of the Compare panel when PDFCRAFT_SHOT is set (for review).
#[test]
fn compare_panel_screenshot() {
    let Ok(out) = std::env::var("PDFCRAFT_SHOT") else { return };
    let s = Session::new();
    let v1 = s.create_from_text("t", "Delivery within five days. Returns accepted.").unwrap().to_vec();
    let v2 = s.create_from_text("t", "Delivery within three days. Returns accepted for a week.").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("v1.pdf", None, v1.clone()).unwrap();
        app.open_bytes("v2.pdf", None, v2.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().compare_old = h.state().views.first().map(|v| v.id);
    h.state_mut().run_compare();
    for _ in 0..20 {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    h.render().unwrap().save(out).unwrap();
}

#[test]
fn pdfa_dialog_verifies_and_converts() {
    let doc = Session::new().create_from_text("t", "Keep forever").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("keep.pdf", None, doc.clone()).unwrap();
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("standards.pdfa"));
    h.run_steps(2);
    h.get_by_label("Declared conformance: none");
    h.get_by_label("Verify").click();
    h.run_steps(3);
    h.get_by_label("The document has no XMP metadata");
    h.get_by_label("Save as PDF/A").click();
    h.run_steps(3);
    h.get_by_label("Declared conformance: PDF/A-2b");
    let issues = h.state().pdfa.issues.clone().unwrap();
    assert!(issues.iter().all(|i| !i.fixable), "{issues:?}");
    let id = h.state().active_ids().unwrap().1;
    assert_eq!(h.state().session.get(id).unwrap().can_undo(), Some("Save as PDF/A-2b"));
}

#[test]
fn export_to_word_html_and_rtf() {
    let doc = Session::new().create_from_text("t", "Exported words").unwrap().to_vec();
    let dir = std::env::temp_dir().join(format!("pdfcraft-office-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("e.pdf", None, doc).unwrap();
    for ext in ["docx", "pptx", "html", "rtf"] {
        let out = dir.join(format!("e.{ext}"));
        app.save_override = Some(out.to_string_lossy().into_owned());
        assert!(app.execute(&format!("export.{ext}")));
        assert!(std::fs::metadata(&out).unwrap().len() > 40, "{ext}");
    }
    assert!(std::fs::read_to_string(dir.join("e.html")).unwrap().contains("Exported words"));
}
