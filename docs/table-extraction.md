# Extracting tables

> **Last updated:** 2026-10-13 · **Status:** basic text-table extraction with review and explicit limits.

PdfCraft can extract rectangular text tables from the current PDF, preview the cells, and export
one table as CSV or Excel (`.xlsx`). It reads unsaved edits and the current displayed text layer;
extracting or exporting adds no table-related edit or undo step. As with other exports, pending
form-field typing is committed before extraction so the output includes the value you entered.

For a scan, run **Recognize text** first. That operation uses the English Tesseract Best model
and adds the searchable text layer. Table extraction reads that layer without running OCR again.
Native text PDFs do not need Tesseract.

## Desktop workflow

Use **File → Extract tables…** or **All tools → Export a PDF → Tables (CSV / Excel)**.
Choose a page, extract, and review the preview. Choose the table when more than one is detected.
For a specific area, use **Edit → Column select** to drag around the table before opening the
dialog. Internal column separators can be supplied in displayed PDF points to correct inference.
Changing the page, region or separators requires a new preview before exporting.

CSV is UTF-8 with comma-separated, quoted cells and CRLF row endings. Values beginning with
spreadsheet formula triggers receive an apostrophe for safety; spreadsheet programs may also
automatically convert numbers and dates. Excel exports preserve every cell as a literal string,
including leading zeros and formula-like values. No formulas, images or formatting are inferred.

## CLI and MCP

Both interfaces use the same tools, with 1-based pages and table indices. Coordinates are points
from the top-left of the displayed page, including rotation, crop and user-unit scaling.

```json
{"tool":"table_extract","args":{"doc":1,"page":1}}
```

The result has `has_text` and `tables`, each with `table`, `rect`, `columns` and rectangular `rows`.
`columns` contains internal x separator positions, not column starts. Inspect these cells before
exporting. To override detection, provide a region and ordered separators:

```json
{"tool":"table_extract","args":{"doc":1,"page":1,"rect":[20,50,550,700],"columns":[250,400]}}
{"tool":"table_export","args":{"doc":1,"page":1,"rect":[20,50,550,700],"columns":[250,400],"table":1,"format":"xlsx","path":"table.xlsx"}}
```

`table_export` re-extracts the current document and writes atomically, respecting `--root` and
PDF copying permissions. Keep the document and options unchanged between preview and export.
It refuses an absent table, truncated page text, invalid page/region, unsorted separators, and separators cutting
through glyphs. No output is written when extraction fails.

## What detection does

The extractor groups horizontal glyphs into visual rows, finds wide gaps shared by multiple rows,
and uses their intersections as separators. This supports right-aligned numeric columns, ordinary
spaces within cells, sparse rows, and tables separated vertically. Glyph centres decide whether
text belongs to a selected region. Manual separators allow extraction when automatic detection
does not find a suitable grid.

This is a geometry heuristic, not a semantic understanding of the document. It does not use
PDF table tags or raster ruling lines. Two-column prose can look like a table. Merged cells,
wrapped/multiline cells, entirely empty rows/columns and cross-page joins are not reconstructed;
each visual text line becomes one row. Side-by-side tables sharing a row may appear as one table.
Rotated/vertical/right-to-left text in the selected region is refused. Text shown in form or
annotation appearances is included by the existing displayed-text extractor. Hidden OCR text is
included; extraction quality therefore depends on recognition quality and word geometry.

For large or ambiguous pages, restrict the region and specify separators. Extraction is bounded
to 100,000 glyphs / 16 MiB of text per page, 128 columns and 200,000 cells per table. Excel rejects
cells beyond its 32,767 UTF-16-unit limit or containing invalid XML characters.

## Implementation

`crates/engine/src/tables.rs` holds glyph grouping, inference, manual extraction and serializers;
`Exporter::tables` supplies the working PDF and rendering configuration behind the engine's panic
guard. The existing Office exporter continues to use paragraph-based inference: that model has
different coordinates and a Word-specific column limit and is not the OCR text-layer interface.
The new workflow adds no dependencies, models, external processes or downloads. XLSX uses the
existing Office ZIP writer with stored entries and a single worksheet of inline strings. Cell
encoding follows Microsoft's [ST_Xstring rules](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oi29500/d34ae755-c53f-4a44-a363-c6dd3ee018a4)
to preserve carriage returns and literal `_xHHHH_` values.

The desktop workflow lives in `crates/ui-egui/src/table_ui.rs`; `table_extract` and `table_export`
are registered in the shared automation table. Synthetic engine and automation tests cover numeric
alignment, missing cells, separate tables, manual regions, invalid geometry/settings, OCR layer
save/reopen, CSV escaping, literal XLSX strings, root confinement and unchanged document state.

## Validation (2026-10-13)

The feature tests cover engine extraction, CLI/MCP tools, and real-shell preview plus CSV/XLSX
saving. The dialog regression checks that cells and export buttons fit inside the card on a
720-point-high window. A layer-visibility regression refuses stale preview export.

A native CLI smoke run extracted two contributor-original Inter text tables (7 rows total),
including right-aligned numbers and a blank cell. CSV and XLSX cells were read back with Python's
CSV reader and openpyxl; literal strings and the CSV formula guard matched the expected values.
Before/after page PNGs were byte-identical and the document stayed clean. A previously generated
Tesseract Best searchable scan produced the expected 9-row, 5-column table and XLSX cells.

The rebuilt Windows app was driven through its opt-in control channel: open the dialog, detect,
inspect the cells/buttons and screenshot, then use the detected separators and re-extract.
The preview was visually checked. Native save pickers and Microsoft Excel itself were not driven;
the real-shell save test and workbook readback cover output generation.

One parallel workspace run failed the unchanged
`reduce_file_size_saves_nothing_when_the_file_is_already_small` test with `no copy was written`.
The same test binary passed its isolated rerun; the cause is unconfirmed. A subsequent run in a
longer temporary directory failed three single-instance tests; shortening the directory resolved
them, consistent with Windows Unix-socket path limits. The final workspace run uses a fresh,
short temporary directory and serial test execution.

`cargo test --offline --workspace --no-fail-fast -- --test-threads=1` completed with **2,668
passed, 0 failed, 13 ignored**, including the table engine, automation and UI regressions.

Formatting, strict workspace Clippy, asset attribution, dependency layering, parity validation,
native builds and the 31-crate WASM check passed. This is validation on synthetic fixtures, not a
quality benchmark over a representative real-document corpus.

## Revision history

| Date | Change |
|---|---|
| 2026-10-13 | Added table extraction, preview, CSV/XLSX export and automation tools. |
