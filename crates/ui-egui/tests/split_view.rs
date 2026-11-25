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

/// Dragging a document tab reorders the tabs and never moves the window; dragging the empty part
/// of the strip still moves it.
#[test]
fn dragging_a_tab_reorders_it_instead_of_moving_the_window() {
    let mut h = two_documents();
    let names = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> {
        h.state().views.iter().filter_map(|v| h.state().session.get(v.id).map(|d| d.name.clone())).collect()
    };
    // A primary drag from `a` to `b`, and whether any frame of it asked to move the window.
    let drag = |h: &mut Harness<'static, PdfCraftApp>, a: egui::Pos2, b: egui::Pos2| {
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let mut events = vec![egui::Event::PointerMoved(a), button(a, true)];
        events.extend((1..=10).map(|k| egui::Event::PointerMoved(a + (b - a) * (k as f32 / 10.0))));
        events.push(button(b, false));
        let mut moved_window = false;
        for e in events {
            h.event(e);
            h.step();
            moved_window |= h.output().viewport_output.values().any(|v| v.commands.contains(&egui::ViewportCommand::StartDrag));
        }
        h.run_steps(3);
        moved_window
    };
    let tab = |h: &Harness<'static, PdfCraftApp>, name: &str| h.get_by_role_and_label(egui::accesskit::Role::Tab, name).rect();
    assert_eq!(names(&h), ["a.pdf", "b.pdf"]);
    let front = h.state().active_ids().map(|(_, id)| id);

    let (a, b) = (tab(&h, "a.pdf"), tab(&h, "b.pdf"));
    assert!(!drag(&mut h, b.center(), egui::pos2(a.left() + 10.0, a.center().y)), "the window stays put");
    assert_eq!(names(&h), ["b.pdf", "a.pdf"]);
    assert_eq!(h.state().active_ids().map(|(_, id)| id), front, "the dragged document stays in front");

    // A tab behind comes to the front as it is dragged.
    let (a, b) = (tab(&h, "a.pdf"), tab(&h, "b.pdf"));
    assert!(!drag(&mut h, a.center(), egui::pos2(b.left() + 10.0, b.center().y)));
    assert_eq!(names(&h), ["a.pdf", "b.pdf"]);
    assert_eq!(h.state().active, Some(0), "a.pdf was brought to the front");

    // Dragged past the last tab, it stops at the end.
    let a = tab(&h, "a.pdf");
    assert!(!drag(&mut h, a.center(), egui::pos2(1000.0, a.center().y)));
    assert_eq!(names(&h), ["b.pdf", "a.pdf"]);

    let empty = egui::pos2(1000.0, a.center().y);
    assert!(drag(&mut h, empty, empty + egui::vec2(60.0, 0.0)), "the empty strip moves the window");

    h.state_mut().move_tab(0, 9);
    assert_eq!(names(&h), ["b.pdf", "a.pdf"], "out of range does nothing");
}

/// A tab's right-click menu duplicates it: an independent copy beside it, at the same page, that
/// can't overwrite the original's file.
#[test]
fn a_tabs_menu_duplicates_it() {
    let mut h = two_documents();
    h.state_mut().views[0].go_to_page(3);
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "a.pdf").click_secondary();
    h.run_steps(4);
    h.get_by_label("Duplicate tab").click();
    h.run_steps(6);
    let app = h.state();
    let names: Vec<_> = app.views.iter().filter_map(|v| app.session.get(v.id).map(|d| d.name.clone())).collect();
    assert_eq!(names, ["a.pdf", "a (copy).pdf", "b.pdf"], "beside the original");
    assert_eq!(app.active, Some(1), "the copy is in front");
    assert_eq!(app.views[1].current, 3, "at the same page");
    let copy = app.session.get(app.views[1].id).unwrap();
    assert!(copy.path.is_none() && copy.dirty, "never saved: closing it asks");

    // The document in front has the menu too, and a copy of it edits on its own.
    let id = h.state().views[1].id;
    h.state_mut().apply_edit(pdfcraft_engine::Edit::DeletePages { pages: vec![0] });
    h.run_steps(3);
    let pages = |h: &Harness<'static, PdfCraftApp>, i: usize| h.state().session.get(h.state().views[i].id).unwrap().info.pages.len();
    assert_eq!((h.state().views[1].id, pages(&h, 0), pages(&h, 1)), (id, 6, 5));
    assert!(h.state_mut().execute("file.duplicate_tab"));
    assert_eq!((h.state().active, pages(&h, 2)), (Some(2), 5), "unsaved edits come along");
}
