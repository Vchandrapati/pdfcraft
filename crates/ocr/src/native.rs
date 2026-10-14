//! Safe, local Tesseract CLI boundary. Images are private, short-lived files; output stays in RAM.
//! arguments never go through a shell. A hung or failed process cannot modify the document.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use super::{Line, OcrError, Word};

const MAX_PIXELS: usize = 100_000_000;
const MAX_TSV_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LOG_BYTES: u64 = 64 * 1024;
const PAGE_TIMEOUT: Duration = Duration::from_secs(120);

fn failed(error: impl std::fmt::Display) -> OcrError {
    OcrError::Recognize(error.to_string())
}

pub(super) fn executable_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PDFCRAFT_TESSERACT") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let name = if cfg!(windows) { "tesseract.exe" } else { "tesseract" };
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("tesseract").join(name));
        candidates.push(dir.join(name));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).filter(|p| !p.as_os_str().is_empty()).map(|p| p.join(name)));
    }
    if cfg!(windows) {
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(variable) {
                candidates.push(PathBuf::from(dir).join("Tesseract-OCR").join(name));
            }
        }
    } else {
        candidates.extend([PathBuf::from("/opt/homebrew/bin/tesseract"), PathBuf::from("/usr/local/bin/tesseract")]);
    }
    // Optional, ignored developer runtime; never embedded in or copied by a Cargo build.
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tesseract").join(name));
    candidates.into_iter().find(|p| p.is_file())
}

fn command(executable: &Path) -> Command {
    let mut cmd = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: recognition must not flash a console over the desktop app.
        cmd.creation_flags(0x08000000);
    }
    cmd
}

/// Kill and reap on every early return (timeout, output limit or I/O error).
struct Process {
    child: Child,
    readers: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

fn bounded_bytes(stream: impl Read, limit: u64) -> Result<Vec<u8>, OcrError> {
    let mut bytes = Vec::new();
    stream.take(limit.saturating_add(1)).read_to_end(&mut bytes).map_err(failed)?;
    if bytes.len() as u64 > limit {
        return Err(failed("Tesseract output exceeds the allowed size"));
    }
    Ok(bytes)
}

fn run(cmd: &mut Command, timeout: Duration, stdout_limit: u64) -> Result<(ExitStatus, String, String), OcrError> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut process = Process { child: cmd.spawn().map_err(|e| failed(format!("starting Tesseract: {e}")))?, readers: Vec::new() };
    let stdout = process.child.stdout.take().ok_or_else(|| failed("could not capture Tesseract output"))?;
    let stderr = process.child.stderr.take().ok_or_else(|| failed("could not capture Tesseract errors"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let output_sender = sender.clone();
    process.readers.push(
        std::thread::Builder::new()
            .name("pdfcraft-ocr-output".into())
            .spawn(move || {
                let _ = output_sender.send((true, bounded_bytes(stdout, stdout_limit)));
            })
            .map_err(failed)?,
    );
    process.readers.push(
        std::thread::Builder::new()
            .name("pdfcraft-ocr-errors".into())
            .spawn(move || {
                let _ = sender.send((false, bounded_bytes(stderr, MAX_LOG_BYTES)));
            })
            .map_err(failed)?,
    );
    let start = Instant::now();
    let (mut output, mut errors, mut status) = (None, None, None);
    loop {
        for (is_output, result) in receiver.try_iter() {
            let bytes = result?;
            if is_output {
                output = Some(bytes);
            } else {
                errors = Some(bytes);
            }
        }
        if status.is_none() {
            status = process.child.try_wait().map_err(failed)?;
        }
        if status.is_some() && output.is_some() && errors.is_some() {
            break;
        }
        if start.elapsed() >= timeout {
            return Err(failed(format!("Tesseract timed out after {} seconds; the page was not changed", timeout.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let status = status.ok_or_else(|| failed("Tesseract stopped without an exit status"))?;
    let output = String::from_utf8(output.ok_or_else(|| failed("Tesseract returned no output"))?).map_err(failed)?;
    let errors = errors.ok_or_else(|| failed("Tesseract returned no diagnostic stream"))?;
    Ok((status, output, String::from_utf8_lossy(&errors).into_owned()))
}

pub(super) fn check_version(executable: &Path) -> Result<(), OcrError> {
    let (status, version, error) = run(command(executable).arg("--version"), Duration::from_secs(10), MAX_LOG_BYTES)?;
    if !status.success() {
        return Err(OcrError::Load("Tesseract".into(), error.trim().into()));
    }
    if !version.lines().next().is_some_and(|line| line.starts_with("tesseract v5.") || line.starts_with("tesseract 5.")) {
        return Err(OcrError::Load("Tesseract".into(), "version 5 is required".into()));
    }
    Ok(())
}

fn color(pixels: &[u8], width: u32, height: u32) -> Result<png::ColorType, OcrError> {
    if width == 0 || height == 0 || pixels.is_empty() {
        return Err(OcrError::EmptyImage);
    }
    let count = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .filter(|n| *n <= MAX_PIXELS)
        .ok_or_else(|| failed("OCR images must contain at most 100 million pixels"))?;
    for (channels, color) in [(1, png::ColorType::Grayscale), (3, png::ColorType::Rgb), (4, png::ColorType::Rgba)] {
        if count.checked_mul(channels) == Some(pixels.len()) {
            return Ok(color);
        }
    }
    Err(failed("OCR pixels must be a complete grey, RGB or RGBA image"))
}

/// Read `pixels` with the `language` model (Tesseract's name, such as `jpn`) from `model_dir`.
pub(super) fn recognize(
    executable: &Path,
    model_dir: &Path,
    language: &str,
    pixels: &[u8],
    width: u32,
    height: u32,
    dpi: u32,
) -> Result<Vec<Line>, OcrError> {
    let color = color(pixels, width, height)?;
    if !(72..=600).contains(&dpi) {
        return Err(failed("OCR resolution must be between 72 and 600 dpi"));
    }
    let dir = tempfile::Builder::new().prefix("pdfcraft-ocr-").tempdir().map_err(failed)?;
    let input = dir.path().join("page.png");
    {
        let mut encoder = png::Encoder::new(File::create(&input).map_err(failed)?, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(failed)?;
        writer.write_image_data(pixels).map_err(failed)?;
        writer.finish().map_err(failed)?;
    }
    let mut cmd = command(executable);
    cmd.arg(&input)
        .arg("stdout")
        .arg("--tessdata-dir")
        .arg(model_dir)
        .args(["-l", language, "--oem", "1", "--psm", "3", "--dpi"])
        .arg(dpi.to_string())
        // Set the output variable directly: no tessdata/configs/tsv installation is required.
        .args(["-c", "tessedit_create_tsv=1"]);
    let (status, tsv, stderr) = run(&mut cmd, PAGE_TIMEOUT, MAX_TSV_BYTES)?;
    if !status.success() {
        return Err(failed(format!("Tesseract failed ({status}): {}", stderr.trim())));
    }
    parse_tsv(&tsv, width, height)
}

/// Keep Tesseract's block/paragraph/line order; sorting solely by y would interleave columns.
fn parse_tsv(text: &str, width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
    const HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext";
    let mut rows = text.lines();
    if rows.next() != Some(HEADER) {
        return Err(failed("Tesseract returned an invalid TSV header"));
    }
    let mut lines = Vec::<Line>::new();
    let mut previous = None;
    for row in rows {
        if row.is_empty() {
            continue;
        }
        let fields: Vec<&str> = row.splitn(12, '\t').collect();
        if fields.len() != 12 {
            return Err(failed("Tesseract returned an incomplete TSV row"));
        }
        let number = |index: usize| fields.get(index).ok_or_else(|| failed("missing TSV field"))?.parse::<u32>().map_err(failed);
        let level = number(0)?;
        if !(1..=5).contains(&level) {
            return Err(failed("Tesseract returned an invalid TSV level"));
        }
        if level != 5 {
            continue;
        }
        let text = fields.get(11).ok_or_else(|| failed("missing TSV text"))?.trim();
        if text.is_empty() {
            continue;
        }
        if text.chars().any(|c| c.is_control()) {
            return Err(failed("Tesseract returned control characters in a word"));
        }
        let (left, top, w, h) = (number(6)?, number(7)?, number(8)?, number(9)?);
        let right = left.checked_add(w).filter(|n| *n <= width).ok_or_else(|| failed("Tesseract word box is outside the image"))?;
        let bottom = top.checked_add(h).filter(|n| *n <= height).ok_or_else(|| failed("Tesseract word box is outside the image"))?;
        if w == 0 || h == 0 {
            return Err(failed("Tesseract returned an empty word box"));
        }
        let key = (number(1)?, number(2)?, number(3)?, number(4)?);
        if key.0 != 1 {
            return Err(failed("Tesseract returned more than one page for one image"));
        }
        if previous != Some(key) {
            lines.push(Line::default());
            previous = Some(key);
        }
        if let Some(line) = lines.last_mut() {
            line.words.push(Word { text: text.into(), rect: [left as f32, top as f32, right as f32, bottom as f32] });
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n";

    #[test]
    fn tsv_preserves_columns_and_word_boxes() {
        let rows = "5\t1\t1\t1\t1\t1\t10\t20\t40\t15\t97.5\tHello\n5\t1\t1\t1\t1\t2\t60\t20\t50\t15\t90\tworld\n5\t1\t2\t1\t1\t1\t200\t20\t60\t15\t90\t€250\n";
        let lines = parse_tsv(&format!("{HEADER}{rows}"), 400, 200).unwrap();
        assert_eq!(lines.iter().map(Line::text).collect::<Vec<_>>(), ["Hello world", "€250"]);
        assert_eq!(lines[0].words[0].rect, [10.0, 20.0, 50.0, 35.0]);
    }

    #[test]
    fn malformed_tsv_and_control_characters_fail_clearly() {
        for row in ["broken", "5\t1\t1\t1\t1\t1\t4294967295\t0\t2\t15\t90\tHello", "5\t1\t1\t1\t1\t1\t0\t0\t20\t10\t90\ta\u{7}b"] {
            assert!(parse_tsv(&format!("{HEADER}{row}"), 400, 200).is_err());
        }
        assert!(parse_tsv("not a TSV header", 400, 200).is_err());
        assert!(parse_tsv(HEADER, 400, 200).unwrap().is_empty());
    }

    /// Any script Tesseract reads is kept: the text layer is no longer limited to WinAnsi.
    #[test]
    fn non_latin_words_are_kept() {
        let rows =
            "5\t1\t1\t1\t1\t1\t10\t20\t40\t15\t90\t日本語\n5\t1\t1\t1\t1\t2\t60\t20\t50\t15\t90\tمرحبا\n5\t1\t1\t1\t1\t3\t120\t20\t20\t15\t90\t☃\n";
        let lines = parse_tsv(&format!("{HEADER}{rows}"), 400, 200).unwrap();
        assert_eq!(lines.iter().map(Line::text).collect::<Vec<_>>(), ["日本語 مرحبا ☃"]);
    }

    #[test]
    fn hostile_dimensions_and_incomplete_images_are_errors() {
        assert!(color(&[0], 0, 1).is_err());
        assert!(color(&[0], u32::MAX, u32::MAX).is_err());
        assert!(color(&[0], 10, 10).is_err());
        assert_eq!(color(&[0; 12], 2, 2).unwrap(), png::ColorType::Rgb);
    }

    // Child test executable: no shell-specific fixtures, assets or environment mutation.
    #[test]
    fn process_fixture() {
        match std::env::var("PDFCRAFT_OCR_TEST_CHILD").as_deref() {
            Ok("timeout") => std::thread::sleep(Duration::from_secs(5)),
            Ok("failure") => {
                eprintln!("synthetic recognizer failure");
                std::process::exit(23);
            }
            Ok("large-output") => println!("{}", "x".repeat(MAX_LOG_BYTES as usize + 1)),
            _ => {}
        }
    }

    fn child(mode: &str) -> Command {
        let mut cmd = command(&std::env::current_exe().unwrap());
        cmd.args(["--exact", "native::tests::process_fixture", "--nocapture"]).env("PDFCRAFT_OCR_TEST_CHILD", mode);
        cmd
    }

    #[test]
    fn timeout_and_excessive_output_are_errors() {
        let start = Instant::now();
        let error = run(&mut child("timeout"), Duration::from_millis(100), MAX_LOG_BYTES).unwrap_err();
        assert!(error.to_string().contains("timed out"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(4), "child was not killed promptly");
        let error = run(&mut child("large-output"), Duration::from_secs(5), MAX_LOG_BYTES).unwrap_err();
        assert!(error.to_string().contains("allowed size"), "{error}");
    }

    #[test]
    fn failed_process_preserves_its_actionable_error() {
        let (status, _, error) = run(&mut child("failure"), Duration::from_secs(5), MAX_LOG_BYTES).unwrap();
        assert_eq!(status.code(), Some(23));
        assert!(error.contains("synthetic recognizer failure"));
    }
}
