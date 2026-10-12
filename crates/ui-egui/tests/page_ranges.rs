//! Shared export/marks page ranges retain the user's selection across dialog frames (#1304).

use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::{
    PdfCraftApp,
    marks::{PageRange, Subset},
};

fn fixture() -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [4 0 R 7 0 R 10 0 R] /Count 3 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    for (i, name) in ["First", "Second", "Third"].iter().enumerate() {
        let page = 4 + i * 3;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> /XObject << /Im {} 0 R >> >> >>",
                page + 1,
                page + 2
            )
            .into_bytes(),
        );
        let content = format!("BT /F1 12 Tf 20 150 Td ({name} page) Tj ET q 20 0 0 20 20 20 cm /Im Do Q");
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes());
        let color = [b"ff0000>".as_slice(), b"00ff00>", b"0000ff>"][i];
        let mut image = b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode /Length 7 >>\nstream\n".to_vec();
        image.extend_from_slice(color);
        image.extend_from_slice(b"\nendstream");
        objects.push(image);
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    for (i, object) in objects.iter().enumerate() {
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    bytes.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
    bytes
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.run_inline = true;
        app.open_bytes("doc.pdf", None, fixture()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

#[test]
fn explicit_ranges_survive_redraws_in_every_shared_dialog() {
    for command in ["export.image", "export.text", "export.all_images", "edit.header_footer", "edit.watermark", "edit.background", "edit.bates"] {
        let mut h = harness();
        assert!(h.state_mut().execute(command));
        h.run_steps(2);
        let export = command.starts_with("export.");
        let range = if export { h.state().export_draft.range } else { h.state().marks_draft.range };
        assert!(range.all);
        assert_eq!((range.from, range.to), (1, 3));
        h.get_by_label("Pages from").click();
        h.run_steps(2);
        for (from, to, subset, expected) in
            [(1, 1, Subset::All, vec![0]), (2, 3, Subset::All, vec![1, 2]), (1, 2, Subset::Odd, vec![0]), (1, 3, Subset::Even, vec![1])]
        {
            let requested = PageRange { all: false, from, to, subset };
            if export {
                h.state_mut().export_draft.range = requested;
            } else {
                h.state_mut().marks_draft.range = requested;
            }
            h.run_steps(3);
            let actual = if export { h.state().export_draft.range } else { h.state().marks_draft.range };
            assert_eq!(actual, requested, "{command}");
            assert_eq!(actual.pages(3), expected, "{command}");
        }
        h.get_by_label("All pages").click();
        h.run_steps(2);
        let range = if export { &mut h.state_mut().export_draft.range } else { &mut h.state_mut().marks_draft.range };
        range.subset = Subset::All;
        assert_eq!(range.pages(3), [0, 1, 2]);
    }
}

#[test]
fn page_one_exports_exclude_other_pages_text_and_images() {
    for command in ["export.image", "export.text", "export.all_images"] {
        let mut h = harness();
        let dir = std::env::temp_dir().join(format!("pdfcraft-page-range-{command}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        h.state_mut().export_dir_override = Some(dir.to_string_lossy().into_owned());
        assert!(h.state_mut().execute(command));
        h.run_steps(2);
        h.state_mut().export_draft.range = PageRange { all: false, from: 1, to: 1, subset: Subset::All };
        h.run_steps(3);
        h.get_by_label("Export").click();
        h.run_steps(4);
        let files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|f| f.unwrap().path()).collect();
        assert_eq!(files.len(), 1, "{command}: {files:?}");
        if command == "export.text" {
            assert_eq!(std::fs::read_to_string(&files[0]).unwrap().trim(), "First page");
        } else if command == "export.image" {
            assert_eq!(files[0].file_name().unwrap(), "doc_page_1.png");
        } else {
            let image = image::open(&files[0]).unwrap().to_rgb8();
            assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0]);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

fn pixels(h: &Harness<'static, PdfCraftApp>) -> Vec<Vec<u8>> {
    let app = h.state();
    let doc = app.session.get(app.views[0].id).unwrap();
    let source = doc.export_source();
    let mut renderer = pdfcraft_render::PageRenderer::new(source.bytes, source.config);
    (0..3)
        .map(|page| {
            let shot = renderer.render(pdfcraft_render::RenderRequest { page, scale: 1.0, ..Default::default() });
            assert!(shot.error.is_none(), "{:?}", shot.error);
            shot.rgba.to_vec()
        })
        .collect()
}

#[test]
fn adding_and_updating_marks_changes_only_page_one() {
    for command in ["edit.header_footer", "edit.watermark", "edit.background"] {
        let mut h = harness();
        let original = pixels(&h);
        for update in [false, true] {
            let before = pixels(&h);
            let action = if update { format!("{command}.update") } else { command.to_string() };
            assert!(h.state_mut().execute(&action));
            h.run_steps(2);
            let draft = &mut h.state_mut().marks_draft;
            draft.range = PageRange { all: false, from: 1, to: 1, subset: Subset::All };
            let text = if update { "UPDATED" } else { "DRAFT" };
            draft.hf.text[4] = text.into();
            draft.wm.text = text.into();
            draft.bg.color = if update { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
            h.run_steps(3);
            h.get_by_label("OK").click();
            h.run_steps(4);
            assert!(h.state().dialog.is_none());
            let after = pixels(&h);
            assert!(after[0] != before[0], "{action}: page one must change");
            assert!(after[1..] == original[1..], "{action}: other pages must remain unchanged");
        }
    }
}
