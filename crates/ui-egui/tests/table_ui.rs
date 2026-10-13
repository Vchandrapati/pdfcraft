//! Export a PDF ▸ Tables in the real shell (egui_kittest): preview extracted cells, then save CSV
//! and Excel data without editing the source document.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

fn table_pdf() -> Vec<u8> {
    let stream = concat!(
        "BT /F1 12 Tf\n",
        "1 0 0 1 20 92 Tm (Name) Tj\n",
        "1 0 0 1 130 92 Tm (Amount) Tj\n",
        "1 0 0 1 20 70 Tm (Tea) Tj\n",
        "1 0 0 1 130 70 Tm ($4.00) Tj\n",
        "1 0 0 1 20 48 Tm (Coffee) Tj\n",
        "1 0 0 1 130 48 Tm ($5.00) Tj\n",
        "ET\n",
    );
    format!(
        "%PDF-1.7\n\
         1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 240 120] >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >> endobj\n\
         4 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n\
         5 0 obj << /Length {} >> stream\n{}endstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        stream.len(),
        stream
    )
    .into_bytes()
}

#[test]
fn table_dialog_previews_and_exports_csv_and_xlsx_without_dirtying_document() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-table-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 720.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("table.pdf", None, table_pdf()).unwrap();
        app.export_dir_override = Some(out.to_string_lossy().into_owned());
        app.run_inline = true;
        app.set_option("dialog", "extract-tables").unwrap();
        app
    });

    h.run_steps(4);
    h.get_by_label("Detect tables").click();
    h.run_steps(4);
    h.get_by_value("Table 1 · 3 rows × 2 columns");
    h.get_by_label("Tea");
    h.get_by_label("$5.00");
    let card: egui::Rect = h.ctx.data(|d| d.get_temp(egui::Id::new("dialog").with("card"))).unwrap();
    for label in ["Export CSV", "Export Excel"] {
        let bounds = h.get_by_label(label).rect();
        assert!(bounds.top() >= 0.0 && bounds.bottom() <= 720.0, "{label} must remain visible after the preview opens: {bounds:?}");
        assert!(card.contains_rect(bounds), "{label} must fit inside the dialog card: {bounds:?} in {card:?}");
    }
    assert!(card.contains_rect(h.get_by_label("Tea").rect()), "preview cells must fit inside the dialog card");

    h.get_by_label("Export CSV").click();
    h.run_steps(2);
    let csv = std::fs::read_to_string(dir.join("table_page_1_1.csv")).unwrap();
    assert_eq!(csv, "\"Name\",\"Amount\"\r\n\"Tea\",\"$4.00\"\r\n\"Coffee\",\"$5.00\"\r\n");

    h.get_by_label("Export Excel").click();
    h.run_steps(2);
    let xlsx = std::fs::read(dir.join("table_page_1_1.xlsx")).unwrap();
    assert!(xlsx.starts_with(b"PK\x03\x04"), "Excel output is a ZIP workbook");

    let app = h.state();
    let id = app.active_ids().unwrap().1;
    assert!(!app.session.get(id).unwrap().dirty, "table extraction and export leave the PDF unchanged");
}
