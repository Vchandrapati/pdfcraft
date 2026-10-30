//! The About dialog in the personal fork: no community links, no contributor credits.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn about_dialog_shows_the_version_without_community_links_or_credits() {
    let mut h = harness(|app| app.dialog = Some(Dialog::About));
    assert!(h.query_by_label(&format!("Version {}", env!("CARGO_PKG_VERSION"))).is_some());
    for label in ["ArtCraft", "Join our Discord", "PdfCraft web page", "PdfCraft on GitHub", "ArtCraft website", "Contributors", "Models"] {
        assert!(h.query_by_label(label).is_none(), "{label} was removed from About");
    }
    for command in ["help.discord", "help.app_page", "help.github", "help.website"] {
        assert!(pdfcraft_engine::commands::command(command).is_none());
        assert!(!h.state_mut().execute(command), "removed community commands cannot run");
    }
}
