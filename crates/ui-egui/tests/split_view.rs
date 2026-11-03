//! View ▸ View side by side in the real shell (egui_kittest): any two open documents share the
//! comparison workspace's two panes, with linked navigation and no comparison.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::Session;
use pdfcraft_ui_egui::PdfCraftApp;

fn two_documents() -> Harness<'static, PdfCraftApp> {
    let a = Session::new().create_blank(200.0, 300.0, 6).unwrap().to_vec();
    let b = Session::new().create_blank(240.0, 360.0, 4).unwrap().to_vec();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 900.0)).build_eframe(move |cc| {
        cc.egui_ctx.options_mut(|options| options.warn_on_id_clash = true);
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("a.pdf", None, a.clone()).unwrap();
        app.open_bytes("b.pdf", None, b.clone()).unwrap();
        app
    });
    h.run_steps(6);
    h
}

#[test]
fn view_side_by_side_shows_two_tabs_and_links_their_pages() {
    let mut h = two_documents();
    assert_eq!(h.state().active, Some(1));
    assert!(h.state_mut().execute("view.split"));
    h.run_steps(6);
    // The active document and the tab before it, each in its own pane.
    assert_eq!(h.state().split_view_indices(), Some([1, 0]));
    let a = h.state().views[0].viewport_rect();
    let b = h.state().views[1].viewport_rect();
    assert!(a.is_positive() && b.is_positive() && !a.intersects(b), "{a:?} {b:?}");
    assert!(h.query_by_label("Close side by side").is_some());
    assert!(h.state().compare.is_none(), "no comparison is run");

    h.state_mut().set_option("page", "3").unwrap();
    h.run_steps(6);
    assert_eq!((h.state().views[0].current, h.state().views[1].current), (2, 2), "linked: page n follows page n");

    h.get_by_label("Link scrolling and zoom").click();
    h.run_steps(4);
    // Clicking in a pane focuses it; move b's pane back to the front explicitly.
    h.state_mut().active = Some(1);
    h.state_mut().set_option("page", "1").unwrap();
    h.run_steps(6);
    assert_eq!((h.state().views[0].current, h.state().views[1].current), (2, 0), "unlinked: the other pane stays");

    h.get_by_label("Close side by side").click();
    h.run_steps(4);
    assert!(h.state().split.is_none());
    assert!(h.state().split_view_indices().is_none());
}

#[test]
fn view_side_by_side_needs_a_second_document_and_toggles_off() {
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("a.pdf", None, Session::new().create_blank(200.0, 300.0, 2).unwrap().to_vec()).unwrap();
    assert!(!app.execute("view.split"));
    assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Open a second document to view side by side"));

    app.open_bytes("b.pdf", None, Session::new().create_blank(200.0, 300.0, 2).unwrap().to_vec()).unwrap();
    assert!(app.execute("view.split"));
    assert!(app.split_view_indices().is_some());
    assert!(app.execute("view.split"), "the same command leaves side by side");
    assert!(app.split.is_none());
}

#[test]
fn view_side_by_side_steps_aside_for_a_third_tab_and_returns() {
    let mut h = two_documents();
    assert!(h.state_mut().execute("view.split"));
    let c = Session::new().create_blank(200.0, 300.0, 2).unwrap().to_vec();
    h.state_mut().open_bytes("c.pdf", None, c).unwrap();
    h.run_steps(6);
    assert_eq!(h.state().active, Some(2));
    assert!(h.state().split_view_indices().is_none(), "another tab shows on its own");
    h.state_mut().active = Some(0);
    h.run_steps(6);
    assert_eq!(h.state().split_view_indices(), Some([1, 0]), "returning to either document shows both again");
}
