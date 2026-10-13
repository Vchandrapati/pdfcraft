//! Dialog action buttons stay against the bottom-right edge of their cards.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{PdfCraftApp, PropsTab};

fn harness() -> Harness<'static, PdfCraftApp> {
    let bytes = pdfcraft_engine::Session::new().create_from_text("layout", "Synthetic layout check").unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("layout.pdf", None, bytes.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn assert_footer(h: &Harness<'_, PdfCraftApp>, label: &str) {
    let card = h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("dialog").with("card"))).unwrap();
    let button = h.get_by_label(label).rect();
    let bottom_gap = card.bottom() - button.bottom();
    let right_gap = card.right() - button.right();
    assert!((16.0..=24.0).contains(&bottom_gap), "{label}: bottom gap {bottom_gap}, button {button:?}, card {card:?}");
    assert!((16.0..=24.0).contains(&right_gap), "{label}: right gap {right_gap}, button {button:?}, card {card:?}");
}

#[test]
fn sanitize_actions_are_at_the_bottom_right() {
    let mut h = harness();
    // The shared modal remembers its height: a short confirmation follows a taller dialog.
    assert!(h.state_mut().execute("file.properties"));
    h.run_steps(8);
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("redact.sanitize"));
    h.run_steps(8);
    assert_footer(&h, "Sanitize");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
}

#[test]
fn document_properties_close_is_at_the_bottom_right_on_every_tab() {
    let mut h = harness();
    assert!(h.state_mut().execute("file.properties"));
    for tab in [PropsTab::Description, PropsTab::InitialView, PropsTab::Security, PropsTab::Fonts, PropsTab::Advanced] {
        h.state_mut().dialog = Some(pdfcraft_ui_egui::Dialog::Properties(tab));
        h.run_steps(8);
        assert_footer(&h, "Close");
    }
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
}
