//! System print spooler supplied by craft-print. PDF composition and permission checks
//! stay in PdfCraft; these adapters retain the application's print-error surface.

pub use craft_print::{Duplex, HAS_PRINTER_PREFERENCES, Job, Printer, PrinterOption, Scaling, printer_options, printers};

pub fn submit(pdf: &[u8], job: &Job) -> Result<String, crate::PrintError> {
    craft_print::submit(pdf, job).map_err(|e| crate::PrintError::Spool(e.to_string()))
}

pub fn open_printer_preferences(printer: &str) -> Result<(), crate::PrintError> {
    craft_print::open_printer_preferences(printer).map_err(|e| crate::PrintError::Spool(e.to_string()))
}
