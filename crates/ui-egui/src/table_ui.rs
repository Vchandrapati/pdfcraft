//! Review and export tables inferred from a page's searchable text.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::export::Exporter;
use pdfcraft_engine::tables::{PageTables, Table, TableOptions};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TableFormat {
    Csv,
    Xlsx,
}

impl TableFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Xlsx => "Excel workbook",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct RequestKey {
    doc: DocId,
    generation: u64,
    page: usize,
    options: TableOptions,
    layers: Vec<((u32, u16), bool)>,
}

type JobResult = Arc<Mutex<Option<Result<PageTables, String>>>>;

struct PendingJob {
    key: RequestKey,
    result: JobResult,
}

#[derive(Default)]
pub(crate) struct TablesUi {
    /// The dialog's page, zero based.
    page: usize,
    /// A Column Select rectangle captured when the dialog opened: (page, displayed points).
    selection: Option<(usize, [f32; 4])>,
    use_selection: bool,
    /// Optional internal separators in displayed page points, comma separated.
    columns: String,
    preview: Option<(RequestKey, PageTables)>,
    selected_table: usize,
    pending: Option<PendingJob>,
    error: Option<String>,
}

impl TablesUi {
    pub(crate) fn open(page: usize, selection: Option<(usize, [f32; 4])>) -> Self {
        Self { page, use_selection: selection.is_some(), selection, ..Self::default() }
    }

    fn options(&self) -> Result<TableOptions, String> {
        let columns = parse_columns(&self.columns)?;
        Ok(TableOptions { rect: self.use_selection.then(|| self.selection.map(|(_, rect)| rect)).flatten(), columns })
    }

    fn poll(&mut self, current_key: Option<RequestKey>, ctx: Option<&egui::Context>) {
        let Some(pending) = &self.pending else { return };
        let outcome = pending.result.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some(outcome) = outcome else {
            if let Some(ctx) = ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            return;
        };
        let key = pending.key.clone();
        self.pending = None;
        match outcome {
            Ok(result) if current_key.as_ref() == Some(&key) => {
                self.selected_table = self.selected_table.min(result.tables.len().saturating_sub(1));
                self.preview = Some((key, result));
                self.error = None;
            }
            Ok(_) => {
                self.preview = None;
                self.error = Some(tl!("The document or extraction settings changed. Run extraction again.").to_string());
            }
            Err(error) => {
                self.preview = None;
                self.error = Some(error);
            }
        }
    }

    fn current_key(&self, app: &PdfCraftApp) -> Option<RequestKey> {
        let (_, id) = app.active_ids()?;
        let doc = app.session.get(id)?;
        Some(RequestKey {
            doc: id,
            generation: doc.edit_generation(),
            page: self.page,
            options: self.options().ok()?,
            layers: doc.info.layers.iter().map(|layer| (layer.id, layer.visible)).collect(),
        })
    }

    fn preview_is_current(&self, app: &PdfCraftApp) -> bool {
        self.preview.as_ref().is_some_and(|(key, _)| self.current_key(app).as_ref() == Some(key))
    }

    fn selected_table(&self, current_key: Option<&RequestKey>) -> Option<(RequestKey, Table)> {
        let (key, result) = self.preview.as_ref()?;
        if current_key != Some(key) {
            return None;
        }
        Some((key.clone(), result.tables.get(self.selected_table)?.clone()))
    }
}

fn parse_columns(value: &str) -> Result<Vec<f32>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|part| {
            let position = part.trim().parse::<f32>().map_err(|_| "Enter separator positions as comma-separated page points".to_string())?;
            if !position.is_finite() {
                return Err("Separator positions must be finite numbers".into());
            }
            Ok(position)
        })
        .collect()
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, Option<TableFormat>, bool) {
    let current_key = app.tables_ui.current_key(app);
    let ctx = app.ctx.clone();
    app.tables_ui.poll(current_key, ctx.as_ref());
    let count = app.active_ids().and_then(|(_, id)| app.session.get(id).map(|doc| doc.info.pages.len())).unwrap_or(0);

    ui.label(egui::RichText::new(tl!("Extract Tables")).font(theme::semibold(18.0)));
    ui.add_space(5.0);
    ui.label(egui::RichText::new(tl!("Review the extracted cells before exporting. Automatic detection uses text spacing and may not match the document's intended table layout.")).color(t.text_muted));
    ui.add_space(10.0);

    let mut run = false;
    let mut export = None;
    let mut cancel = false;
    {
        let d = &mut app.tables_ui;
        d.page = d.page.min(count.saturating_sub(1));
        egui::Grid::new("extract_tables_settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tl!("Page"));
            ui.horizontal(|ui| {
                let mut page_number = d.page.saturating_add(1);
                let page_changed = ui.add(egui::DragValue::new(&mut page_number).range(1..=count.max(1))).changed();
                if page_changed {
                    d.page = page_number.saturating_sub(1);
                    d.use_selection = false;
                }
                ui.label(crate::i18n::fmt(tl!("of {n}"), &[("n", &count.to_string())]));
            });
            ui.end_row();

            if let Some((region_page, rect)) = d.selection {
                let enabled = region_page == d.page;
                ui.label(tl!("Region"));
                ui.add_enabled_ui(enabled, |ui| {
                    ui.add(widgets::checkbox(&mut d.use_selection, tl!("Use the selected rectangle")));
                });
                ui.end_row();
                if enabled {
                    ui.label("");
                    ui.label(
                        egui::RichText::new(format!("x {:.0}–{:.0}, y {:.0}–{:.0} pt", rect[0], rect[2], rect[1], rect[3]))
                            .small()
                            .color(t.text_faint),
                    );
                    ui.end_row();
                } else {
                    d.use_selection = false;
                }
            } else {
                ui.label(tl!("Region"));
                ui.label(
                    egui::RichText::new(tl!("Select a rectangle with Column Select before opening this dialog to limit the page area."))
                        .small()
                        .color(t.text_faint),
                );
                ui.end_row();
            }

            ui.label(tl!("Column separators"));
            ui.add(widgets::line(&mut d.columns).desired_width(310.0).hint_text(tl!("Automatic, or e.g. 180, 340")));
            ui.end_row();
        });
        if d.use_selection && d.selection.is_none_or(|(page, _)| page != d.page) {
            d.use_selection = false;
        }
    }
    ui.label(
        egui::RichText::new(tl!(
            "Leave separators empty for automatic detection. Positions are measured from the displayed page's top-left in points."
        ))
        .small()
        .color(t.text_faint),
    );
    ui.add_space(8.0);

    let busy = app.tables_ui.pending.is_some();
    ui.horizontal(|ui| {
        let label = if app.tables_ui.preview.is_some() { tl!("Run extraction") } else { tl!("Detect tables") };
        if ui.add_enabled_ui(!busy && count > 0, |ui| widgets::pill_button(ui, label, true)).inner.clicked() {
            run = true;
        }
        if busy {
            ui.spinner();
            ui.label(tl!("Extracting…"));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::pill_button(ui, tl!("Close"), false).clicked() {
                cancel = true;
            }
        });
    });

    if let Some(error) = &app.tables_ui.error {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(error).color(ui.visuals().error_fg_color));
    }

    let preview_current = app.tables_ui.preview_is_current(app);
    let d = &mut app.tables_ui;
    if let Some((_, result)) = &d.preview {
        ui.separator();
        if !preview_current {
            ui.label(egui::RichText::new(tl!("Settings changed. Run extraction again to refresh this preview.")).color(ui.visuals().error_fg_color));
        } else if !result.has_text {
            ui.label(
                egui::RichText::new(tl!("No text in this region. For scans, run Recognize Text first; otherwise select an area containing text."))
                    .color(t.text_muted),
            );
        } else if result.tables.is_empty() {
            ui.label(egui::RichText::new(tl!("No tables detected. Adjust the region or add separator positions and run again.")).color(t.text_muted));
        } else {
            let selected = d.selected_table.min(result.tables.len().saturating_sub(1));
            egui::ComboBox::from_id_salt("extract_tables_choice").selected_text(table_label(selected, result.tables.get(selected))).show_ui(
                ui,
                |ui| {
                    for (index, table) in result.tables.iter().enumerate() {
                        ui.selectable_value(&mut d.selected_table, index, table_label(index, Some(table)));
                    }
                },
            );
            if let Some(table) = result.tables.get(d.selected_table) {
                preview_grid(ui, table, t);
                if widgets::pill_button(ui, tl!("Use selected table's separators for correction"), false).clicked() {
                    d.use_selection = false;
                    // Keep the selected table as the region so automatic separators from nearby
                    // body text cannot affect the corrected extraction.
                    d.selection = Some((d.page, table.rect));
                    d.use_selection = true;
                    d.columns = table.columns.iter().map(|x| format!("{x:.2}")).collect::<Vec<_>>().join(", ");
                }
                ui.horizontal(|ui| {
                    let enabled = preview_current && !busy;
                    if ui.add_enabled_ui(enabled, |ui| widgets::pill_button(ui, tl!("Export Excel"), true)).inner.clicked() {
                        export = Some(TableFormat::Xlsx);
                    }
                    if ui.add_enabled_ui(enabled, |ui| widgets::pill_button(ui, tl!("Export CSV"), true)).inner.clicked() {
                        export = Some(TableFormat::Csv);
                    }
                });
            }
        }
    }
    (run, export, cancel)
}

fn table_label(index: usize, table: Option<&Table>) -> String {
    match table {
        Some(table) => crate::i18n::fmt(
            tl!("Table {n} · {rows} rows × {columns} columns"),
            &[
                ("n", &index.saturating_add(1).to_string()),
                ("rows", &table.rows.len().to_string()),
                ("columns", &table.columns.len().saturating_add(1).to_string()),
            ],
        ),
        None => crate::i18n::fmt(tl!("Table {n}"), &[("n", &index.saturating_add(1).to_string())]),
    }
}

fn preview_grid(ui: &mut egui::Ui, table: &Table, t: &Tokens) {
    const ROW_LIMIT: usize = 32;
    const COLUMN_LIMIT: usize = 10;
    const CELL_CHAR_LIMIT: usize = 240;
    ui.add_space(8.0);
    egui::ScrollArea::both().max_height(270.0).show(ui, |ui| {
        egui::Grid::new("extract_tables_preview").striped(true).min_col_width(72.0).max_col_width(220.0).spacing([12.0, 5.0]).show(ui, |ui| {
            for row in table.rows.iter().take(ROW_LIMIT) {
                for cell in row.iter().take(COLUMN_LIMIT) {
                    let mut chars = cell.chars();
                    let mut preview: String = chars.by_ref().take(CELL_CHAR_LIMIT).collect();
                    if chars.next().is_some() {
                        preview.push('…');
                    }
                    ui.label(egui::RichText::new(preview).color(t.text));
                }
                ui.end_row();
            }
        });
    });
    let rows_hidden = table.rows.len().saturating_sub(ROW_LIMIT);
    let cells_hidden = table.rows.iter().take(ROW_LIMIT).map(|row| row.len().saturating_sub(COLUMN_LIMIT)).sum::<usize>();
    let cells_shortened = table
        .rows
        .iter()
        .take(ROW_LIMIT)
        .flat_map(|row| row.iter().take(COLUMN_LIMIT))
        .filter(|cell| cell.chars().nth(CELL_CHAR_LIMIT).is_some())
        .count();
    if rows_hidden > 0 || cells_hidden > 0 || cells_shortened > 0 {
        ui.label(
            egui::RichText::new(crate::i18n::fmt(
                tl!("Preview omits {rows} rows and {cells} cells; it shortens {shortened} long cells. Export includes the full table."),
                &[("rows", &rows_hidden.to_string()), ("cells", &cells_hidden.to_string()), ("shortened", &cells_shortened.to_string())],
            ))
            .small()
            .color(t.text_faint),
        );
    }
}

impl PdfCraftApp {
    pub(crate) fn open_tables_dialog(&mut self) {
        if self.tables_ui.pending.is_some() {
            self.dialog = Some(crate::Dialog::ExtractTables);
            return;
        }
        let page = self.active.and_then(|i| self.views.get(i)).map_or(0, |view| view.current);
        let selection = self.active.and_then(|i| self.views.get(i)).and_then(crate::DocView::selected_column_region);
        self.tables_ui = TablesUi::open(page, selection);
        self.dialog = Some(crate::Dialog::ExtractTables);
    }

    pub(crate) fn start_table_extraction(&mut self) {
        if self.tables_ui.pending.is_some() {
            return;
        }
        if !self.commit_form_typing() {
            return;
        }
        let Some((_, id)) = self.active_ids() else { return };
        if !self.may_copy(id) {
            return;
        }
        let Some(key) = self.tables_ui.current_key(self) else {
            self.tables_ui.error = Some("Choose a valid page, region and set of column separators".into());
            return;
        };
        let Some(doc) = self.session.get(id) else { return };
        let src = doc.export_source();
        let options = key.options.clone();
        let page = key.page;
        let result: JobResult = Arc::new(Mutex::new(None));
        let worker_result = result.clone();
        let ctx = self.ctx.clone();
        let work = move || {
            let outcome = pdfcraft_engine::guard(|| Exporter::from_source(src).tables(page, &options))
                .unwrap_or_else(|error| Err(format!("Table extraction failed safely: {error}")));
            *worker_result.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };

        self.tables_ui.preview = None;
        self.tables_ui.error = None;
        self.tables_ui.pending = Some(PendingJob { key, result });
        #[cfg(target_arch = "wasm32")]
        work();
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else if let Err(error) = std::thread::Builder::new().name("pdfcraft-tables".into()).spawn(work) {
            self.tables_ui.pending = None;
            self.tables_ui.error = Some(format!("Couldn't start table extraction: {error}"));
        }
    }

    pub(crate) fn export_selected_table(&mut self, format: TableFormat) {
        let current_key = self.tables_ui.current_key(self);
        let Some((key, table)) = self.tables_ui.selected_table(current_key.as_ref()) else {
            self.notify_tr("Run table extraction again before exporting this preview");
            return;
        };
        let Some((_, id)) = self.active_ids() else { return };
        if !self.may_copy(id) {
            return;
        }
        let bytes = match format {
            TableFormat::Csv => Ok(table.csv().into_bytes()),
            TableFormat::Xlsx => table.xlsx(),
        };
        let bytes = match bytes {
            Ok(bytes) => bytes,
            Err(error) => {
                self.notify_fmt("Table export stopped: {error}", &[("error", &error)]);
                return;
            }
        };
        let name = format!("table_page_{}_{}.{}", key.page.saturating_add(1), self.tables_ui.selected_table.saturating_add(1), format.extension());
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(dir) = self.export_dir_override.clone() {
                let path = std::path::PathBuf::from(dir).join(&name);
                match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                    Ok(()) => self.notify_fmt("Exported {name}", &[("name", &path.display().to_string())]),
                    Err(error) => {
                        self.notify_fmt("Couldn't save {name}: {error}", &[("name", &path.display().to_string()), ("error", &error.to_string())])
                    }
                }
                return;
            }
            let extension = format.extension();
            let picker = rfd::AsyncFileDialog::new().set_file_name(&name).add_filter(format.label(), &[extension]);
            self.ask_one(crate::pickers::Ask::Save(picker), Some(id), move |app, path| {
                let shown = path.display().to_string();
                match crate::editing::write_atomically(&path.to_string_lossy(), &bytes) {
                    Ok(()) => app.notify_fmt("Exported {name}", &[("name", &shown)]),
                    Err(error) => app.notify_fmt("Couldn't save {name}: {error}", &[("name", &shown), ("error", &error.to_string())]),
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        if let Err(error) = crate::editing::download(&name, &bytes) {
            self.notify_fmt("Couldn't download {name}: {error}", &[("name", &name), ("error", &error)]);
        } else {
            self.notify_fmt("Downloaded {name}", &[("name", &name)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_layer_visibility_invalidates_table_preview() {
        let content = "/OC /TableLayer BDC BT /F1 12 Tf 20 150 Td (Name) Tj 130 0 Td (Qty) Tj -130 -24 Td (Tea) Tj 130 0 Td (2) Tj ET EMC";
        let bytes = format!("%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R] /D << /Order [6 0 R] /ON [6 0 R] >> >> >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> /Properties << /TableLayer 6 0 R >> >> >> endobj\n4 0 obj << /Length {} >> stream\n{content}\nendstream endobj\n5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\n6 0 obj << /Type /OCG /Name (Table) >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF", content.len()).into_bytes();
        let mut app = PdfCraftApp::new();
        app.open_bytes("layers.pdf", None, bytes).unwrap();
        app.run_inline = true;
        app.open_tables_dialog();
        app.start_table_extraction();
        app.tables_ui.poll(app.tables_ui.current_key(&app), None);
        assert!(app.tables_ui.preview_is_current(&app));
        assert!(app.tables_ui.preview.as_ref().unwrap().1.has_text);
        let id = app.active_ids().unwrap().1;
        assert!(app.session.set_layer_visible(id, 0, false));
        assert!(!app.tables_ui.preview_is_current(&app), "a hidden layer must not export stale cells");
        app.start_table_extraction();
        app.tables_ui.poll(app.tables_ui.current_key(&app), None);
        assert!(app.tables_ui.preview_is_current(&app));
        assert!(!app.tables_ui.preview.as_ref().unwrap().1.has_text);
    }
}
