//! pdfcraft-ocr — Scan & OCR ▸ Recognize text (L4).
//!
//! Recognition runs a local Tesseract 5 process with the pinned `tessdata_best` models
//! (Apache-2.0, fetched by `cargo xtask models`; see ATTRIBUTION.toml). No network is used for OCR.
//! The caller renders a page to pixels;
//! [`Ocr::recognize`] finds the words in them, and [`text_layer`] turns words placed in user space
//! into page content: invisible text (rendering mode 3) over each word, so the page becomes a
//! searchable image (Acrobat's "Searchable Image (Exact)": the image is left untouched).
//!
//! Desktop only: browser builds cannot launch Tesseract. The text layer uses generated Type 3
//! fonts whose glyphs draw nothing, so it can carry any character Tesseract reads.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pdfcraft_cos::{Dict, Document, Object, Stream};

#[cfg(not(target_arch = "wasm32"))]
mod native;

pub use pdfcraft_fonts::helvetica_width;

/// The English Best model, pinned in ATTRIBUTION.toml. It is required; the others are optional.
pub const RECOGNITION_MODEL: &str = "eng.traineddata";

/// The language choices (ISO 639-1 code, name); [`Models::has_language`] says which are installed.
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("ja", "Japanese"), ("ar", "Arabic")];

/// Tesseract's name for each of [`LANGUAGES`]: its model is `<name>.traineddata`.
fn tesseract_name(language: &str) -> Option<&'static str> {
    match language {
        "en" => Some("eng"),
        "ja" => Some("jpn"),
        "ar" => Some("ara"),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// Shown to end users (release packages ship the models; a source build fetches them with
    /// `cargo xtask models`). The UI catalogs key this exact text.
    #[error("the text recognition models are not installed (reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them)")]
    NoModels,
    #[error("Tesseract 5 is not installed: install it, or set PDFCRAFT_TESSERACT to its executable")]
    NoRuntime,
    /// A language whose model isn't in the models folder (or one PdfCraft doesn't offer).
    #[error("the {0} text recognition model ({1}) is not installed: put it in the models folder (cargo xtask models fetches it)")]
    NoLanguage(String, String),
    #[error("loading {0}: {1}")]
    Load(String, String),
    #[error("text recognition failed: {0}")]
    Recognize(String),
    #[error("the image is empty")]
    EmptyImage,
}

/// Where the English Best model is; the other languages' models sit beside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub recognition: PathBuf,
}

impl Models {
    /// The model in `dir`, if it is there.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        let m = Models { recognition: dir.join(RECOGNITION_MODEL) };
        m.recognition.is_file().then_some(m)
    }

    /// The folder that holds the models.
    pub fn dir(&self) -> &Path {
        self.recognition.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))
    }

    /// The model file for `language` (a [`LANGUAGES`] code), if it is installed.
    pub fn language(&self, language: &str) -> Option<PathBuf> {
        let path = self.dir().join(format!("{}.traineddata", tesseract_name(language)?));
        path.is_file().then_some(path)
    }

    /// Whether the model for `language` is installed.
    pub fn has_language(&self, language: &str) -> bool {
        self.language(language).is_some()
    }

    /// The [`LANGUAGES`] whose models are installed (English first).
    pub fn installed_languages(&self) -> Vec<(&'static str, &'static str)> {
        LANGUAGES.iter().copied().filter(|(code, _)| self.has_language(code)).collect()
    }

    /// Look for the models: `$PDFCRAFT_MODELS`, then where the release packages install them
    /// beside the executable ([`Models::dirs_beside_exe`]), then the source tree's `assets/models/`.
    pub fn find() -> Option<Models> {
        if let Some(dir) = std::env::var_os("PDFCRAFT_MODELS") {
            return Self::in_dir(Path::new(&dir));
        }
        Self::search_dirs().iter().find_map(|d| Self::in_dir(d))
    }

    /// The directories [`Models::find`] looks in, in order.
    pub fn search_dirs() -> Vec<PathBuf> {
        if let Some(d) = std::env::var_os("PDFCRAFT_MODELS") {
            return vec![PathBuf::from(d)];
        }
        let mut dirs = Vec::new();
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            dirs.extend(Self::dirs_beside_exe(&exe));
        }
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models"));
        dirs
    }

    /// Where the release packages put the models, relative to the directory holding the
    /// executable (packaging/*):
    /// - `models/` beside it: the Windows MSI and portable zip;
    /// - `../Resources/models`: the macOS app bundle (`Contents/MacOS` → `Contents/Resources`);
    /// - `../share/pdfcraft/models`: the FHS-style trees (`bin/` → `share/`) of the Linux
    ///   deb/rpm/tar.gz/AppImage/Flatpak and the FreeBSD tarball, wherever they are installed.
    pub fn dirs_beside_exe(exe_dir: &Path) -> Vec<PathBuf> {
        vec![exe_dir.join("models"), exe_dir.join("../Resources/models"), exe_dir.join("../share/pdfcraft/models")]
    }
}

/// A recognised word: its text and its box in image pixels `[left, top, right, bottom]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: [f32; 4],
}

/// A recognised line of words, in reading order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Line {
    pub words: Vec<Word>,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// A configured recogniser. Each page runs in a bounded local process so a failed OCR worker
/// cannot corrupt the document. The UI keeps this configuration and runs it off the UI thread.
pub struct Ocr {
    #[cfg(not(target_arch = "wasm32"))]
    executable: PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    models: Models,
}

impl Ocr {
    pub fn load(models: &Models) -> Result<Ocr, OcrError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !models.recognition.is_file() {
                return Err(OcrError::NoModels);
            }
            let executable = Self::executable_path().ok_or(OcrError::NoRuntime)?;
            native::check_version(&executable)?;
            Ok(Ocr { executable, models: models.clone() })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = models;
            Err(OcrError::Recognize("Tesseract OCR requires the desktop application".into()))
        }
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<Ocr, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }

    /// Find the local executable without launching it (safe to use when drawing the UI).
    pub fn executable_path() -> Option<PathBuf> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            native::executable_path()
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
    }

    /// An actionable installation problem, without launching a process or loading the model.
    pub fn unavailable_reason() -> Option<String> {
        if cfg!(target_arch = "wasm32") {
            Some("Tesseract OCR requires the desktop application".into())
        } else if Models::find().is_none() {
            Some(OcrError::NoModels.to_string())
        } else if Self::executable_path().is_none() {
            Some(OcrError::NoRuntime.to_string())
        } else {
            None
        }
    }

    pub fn available() -> bool {
        Self::unavailable_reason().is_none()
    }

    /// Recognise the English text in an RGBA (or RGB, or grey) image, `width` × `height` pixels.
    pub fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        self.recognize_at_dpi(pixels, width, height, 300, "en")
    }

    /// Recognise text in `language` (a [`LANGUAGES`] code) at the actual raster resolution; the
    /// resolution matters for low-resolution scans.
    pub fn recognize_at_dpi(&self, pixels: &[u8], width: u32, height: u32, dpi: u32, language: &str) -> Result<Vec<Line>, OcrError> {
        let name = LANGUAGES.iter().find(|l| l.0 == language).map_or(language, |l| l.1);
        let tesseract = tesseract_name(language).ok_or_else(|| OcrError::NoLanguage(name.into(), format!("{language}.traineddata")))?;
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !self.models.has_language(language) {
                return Err(OcrError::NoLanguage(name.into(), format!("{tesseract}.traineddata")));
            }
            native::recognize(&self.executable, self.models.dir(), tesseract, pixels, width, height, dpi)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (pixels, width, height, dpi, tesseract);
            Err(OcrError::Recognize("Tesseract OCR requires the desktop application".into()))
        }
    }
}

/// A word placed on a page, in PDF user space: the bottom-left corner of its box, and the
/// vectors along its bottom edge (`across`, left → right as read) and its left edge (`up`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedWord {
    pub text: String,
    pub origin: [f64; 2],
    pub across: [f64; 2],
    pub up: [f64; 2],
}

impl PlacedWord {
    /// Place a pixel-space word with `to_user`, which maps an image pixel (x right, y down) to
    /// user space.
    pub fn place(word: &Word, to_user: impl Fn(f32, f32) -> [f64; 2]) -> PlacedWord {
        let [l, t, r, b] = word.rect;
        let o = to_user(l, b);
        let br = to_user(r, b);
        let tl = to_user(l, t);
        PlacedWord { text: word.text.clone(), origin: o, across: [br[0] - o[0], br[1] - o[1]], up: [tl[0] - o[0], tl[1] - o[1]] }
    }
}

/// The invisible font's glyph box, in thousandths of an em (its `/FontBBox` and descriptor):
/// Helvetica-like ascent and descent, so selections look like the page's other text.
const FONT_ASCENT: i64 = 720;
const FONT_DESCENT: i64 = -210;
/// Helvetica's ascender and descender (em fractions): a word box spans about this much.
const BOX_EM: f64 = 0.93;
const DESCENT: f64 = 0.21;
/// Helvetica's cap height and x-height (em fractions).
const CAP_HEIGHT: f64 = 0.718;
const X_HEIGHT: f64 = 0.523;

/// How far `text`'s ink reaches above and below the baseline, in em fractions: a word box is
/// tight around the ink, so "DT" spans the cap height only, "sun" the x-height, and "gap" reaches
/// down to the descender. Text without letters or digits keeps the full ascender-descender span.
fn ink_extent(text: &str) -> (f64, f64) {
    if !text.chars().any(char::is_alphanumeric) {
        return (BOX_EM - DESCENT, -DESCENT);
    }
    let tall = text
        .chars()
        .any(|c| c.is_uppercase() || c.is_ascii_digit() || "bdfhijklt!?/\\()[]{}|'\"#$%&@".contains(c) || (c.is_alphabetic() && !c.is_ascii()));
    let low = text.chars().any(|c| "gjpqy()[]{}|_".contains(c));
    (if tall { CAP_HEIGHT } else { X_HEIGHT }, if low { -DESCENT } else { 0.0 })
}
/// One-byte codes 1–255 per font; a page with more distinct characters gets more fonts.
const CODES_PER_FONT: usize = 255;

fn num(v: f64) -> pdfcraft_content::Num {
    pdfcraft_content::Num(v, 4)
}

/// A character's advance in the invisible font, in thousandths of an em: full width for CJK,
/// Helvetica-like proportions otherwise, so a selection inside a word lands near its letters.
fn advance(c: char) -> i64 {
    (pdfcraft_fonts::cjk_width(c.encode_utf8(&mut [0; 4]), 1000.0).round() as i64).max(1)
}

/// `text` in the order its characters appear left to right. Right-to-left words (Arabic) are
/// reversed, as PDF producers write them; extractors apply the bidi algorithm to read them back.
fn visual_order(text: &str) -> String {
    let info = unicode_bidi::BidiInfo::new(text, None);
    match info.paragraphs.first() {
        Some(paragraph) if info.has_rtl() => info.reorder_line(paragraph, paragraph.range.clone()).into_owned(),
        _ => text.to_owned(),
    }
}

/// Writes `words` as invisible page text: the returned content, plus the fonts it names, added to
/// `fonts` (the page's font resources) under names not already there. Each word is stretched to
/// its box so selection and search highlight the right place, and the content is marked `/OCR`
/// so it can be told apart from the page's own text.
///
/// The fonts are generated Type 3 fonts whose glyphs draw nothing, with a `/ToUnicode` map, so
/// any character Tesseract reads (Japanese, Arabic, symbols) is searchable and copyable.
pub fn text_layer(doc: &mut Document, fonts: &mut Dict, words: &[PlacedWord]) -> Vec<u8> {
    // Each distinct character gets (font, code), in order of first appearance.
    let mut codes: HashMap<char, (usize, u8)> = HashMap::new();
    let mut sets: Vec<Vec<char>> = Vec::new();
    let mut placed = Vec::with_capacity(words.len());
    for w in words {
        let text = visual_order(&w.text);
        let width: i64 = text.chars().map(advance).sum();
        let height = w.up[0].hypot(w.up[1]);
        if width <= 0 || height.is_nan() || height <= 0.0 {
            continue;
        }
        for c in text.chars() {
            if codes.contains_key(&c) {
                continue;
            }
            if sets.last().is_none_or(|s| s.len() >= CODES_PER_FONT) {
                sets.push(Vec::new());
            }
            let font = sets.len() - 1;
            if let Some(set) = sets.last_mut() {
                set.push(c);
                // At most CODES_PER_FONT (255) characters per set, so the code fits a byte.
                codes.insert(c, (font, set.len() as u8));
            }
        }
        placed.push((w, text, width as f64 / 1000.0));
    }
    let names: Vec<String> = sets.iter().map(|set| add_font(doc, fonts, set)).collect();

    let mut out = b"/OCR BMC\nBT\n3 Tr\n".to_vec();
    // The font stays selected across words (Tm doesn't reset it), so Tf is written only on a change.
    let mut current = None;
    for (w, text, width) in placed {
        // The em square: the word's own ink (cap height or x-height, descenders or not) spans the
        // box, so the text is as tall as the scanned word; its width is stretched to fit the word.
        let (top, bottom) = ink_extent(&w.text);
        let em = [w.up[0] / (top - bottom), w.up[1] / (top - bottom)];
        let ax = [w.across[0] / width, w.across[1] / width];
        let o = [w.origin[0] - em[0] * bottom, w.origin[1] - em[1] * bottom];
        let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
        let nums: Vec<String> = m.iter().map(|v| num(*v).to_string()).collect();
        out.extend_from_slice(nums.join(" ").as_bytes());
        out.extend_from_slice(b" Tm");
        // One Tj per run of characters in the same font.
        let mut run = String::new();
        for c in text.chars() {
            let Some(&(font, code)) = codes.get(&c) else { continue };
            if current != Some(font) {
                if !run.is_empty() {
                    out.extend_from_slice(format!(" <{run}> Tj").as_bytes());
                    run.clear();
                }
                let name = names.get(font).map_or("PCOcr", String::as_str);
                out.extend_from_slice(format!(" /{name} 1 Tf").as_bytes());
                current = Some(font);
            }
            run.push_str(&format!("{code:02X}"));
        }
        if !run.is_empty() {
            out.extend_from_slice(format!(" <{run}> Tj").as_bytes());
        }
        out.push(b'\n');
    }
    out.extend_from_slice(b"ET\nEMC\n");
    out
}

/// Adds a Type 3 font whose codes 1 to `chars.len()` stand for `chars` and draw nothing, and
/// returns its resource name (`PCOcr`, `PCOcr1`, ...: the first one `fonts` doesn't have).
fn add_font(doc: &mut Document, fonts: &mut Dict, chars: &[char]) -> String {
    let mut procs: HashMap<i64, Object> = HashMap::new();
    let mut charprocs = Dict::new();
    let mut differences = vec![Object::Int(1)];
    let mut widths = Vec::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        let code = i + 1;
        let glyph = format!("g{code:02X}");
        let width = advance(c);
        // Glyphs of one width share one empty procedure.
        let proc = procs
            .entry(width)
            .or_insert_with(|| Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), format!("{width} 0 0 0 0 0 d1\n").into_bytes())))))
            .clone();
        charprocs.set(glyph.as_bytes().to_vec(), proc);
        differences.push(Object::name(&glyph));
        widths.push(Object::Int(width));
    }
    let texts: Vec<String> = chars.iter().map(char::to_string).collect();
    let cmap = pdfcraft_fonts::simple_to_unicode(texts.iter().enumerate().map(|(i, text)| (i + 1, text.as_str())));
    let to_unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));

    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name("PdfCraftOCR"));
    descriptor.set(b"Flags".to_vec(), Object::Int(4));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    descriptor.set(b"Ascent".to_vec(), Object::Int(FONT_ASCENT));
    descriptor.set(b"Descent".to_vec(), Object::Int(FONT_DESCENT));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
    font.set(b"FontBBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(FONT_DESCENT), Object::Int(1000), Object::Int(FONT_ASCENT)]));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(chars.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
    font.set(b"ToUnicode".to_vec(), Object::Ref(to_unicode));
    let mut name = String::from("PCOcr");
    let mut suffix = 0usize;
    while fonts.contains(name.as_bytes()) {
        suffix = suffix.saturating_add(1);
        name = format!("PCOcr{suffix}");
    }
    fonts.set(name.as_bytes().to_vec(), Object::Dict(font));
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_words_through_the_mapping() {
        // 2 pixels per point, page 612 × 792, y flipped.
        let to_user = |x: f32, y: f32| [x as f64 / 2.0, 792.0 - y as f64 / 2.0];
        let w = PlacedWord::place(&Word { text: "Hello".into(), rect: [100.0, 200.0, 300.0, 240.0] }, to_user);
        assert_eq!(w.origin, [50.0, 672.0]);
        assert_eq!(w.across, [100.0, 0.0]);
        assert_eq!(w.up, [0.0, 20.0]);
    }

    #[test]
    fn text_layer_is_invisible_text_fitted_to_each_box() {
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3] };
        let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
        let s = String::from_utf8(text_layer(&mut doc, &mut fonts, &[w])).unwrap();
        assert!(s.contains("3 Tr"), "{s}");
        let width = helvetica_width("Hi", 1.0);
        // "Hi" has no descender: its box spans the cap height, from the baseline up.
        assert!(s.contains(&format!("{} 0 0 {} 10 20 Tm /PCOcr 1 Tf <0102> Tj", num(30.0 / width), num(9.3 / CAP_HEIGHT))), "{s}");
        assert!(s.starts_with("/OCR BMC") && s.ends_with("EMC\n"));
        let font = fonts.get(b"PCOcr").and_then(Object::as_dict).unwrap();
        assert_eq!(font.name(b"Subtype"), Some(&b"Type3"[..]));
    }

    /// The ToUnicode map of resource font `name`.
    fn to_unicode(doc: &Document, fonts: &Dict, name: &str) -> String {
        let font = fonts.get(name.as_bytes()).and_then(Object::as_dict).unwrap();
        match &*doc.resolve(font.get(b"ToUnicode").unwrap()) {
            Object::Stream(s) => String::from_utf8(s.decoded().unwrap()).unwrap(),
            other => panic!("ToUnicode is {other:?}"),
        }
    }

    #[test]
    fn any_script_is_mapped_back_to_its_characters() {
        let word = |text: &str| PlacedWord { text: text.into(), origin: [0.0, 0.0], across: [40.0, 0.0], up: [0.0, 10.0] };
        let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
        let s = String::from_utf8(text_layer(&mut doc, &mut fonts, &[word("日本"), word("مرحبا"), word("😀")])).unwrap();
        let cmap = to_unicode(&doc, &fonts, "PCOcr");
        assert!(cmap.contains("<01> <65E5>") && cmap.contains("<02> <672C>"), "{cmap}");
        // Arabic is written in visual order: its last letter (alef) comes first on the page.
        assert!(cmap.contains("<03> <0627>") && cmap.contains("<07> <0645>"), "{cmap}");
        // Beyond the BMP: a UTF-16 surrogate pair.
        assert!(cmap.contains("<08> <D83DDE00>"), "{cmap}");
        assert!(s.contains("<0102> Tj") && s.contains("<0304050607> Tj") && s.contains("<08> Tj"), "{s}");
    }

    #[test]
    fn many_characters_spill_into_more_fonts_with_free_names() {
        let text: String = (0..300).map(|i| char::from_u32(0x4E00 + i).unwrap()).collect();
        let w = PlacedWord { text, origin: [0.0, 0.0], across: [500.0, 0.0], up: [0.0, 10.0] };
        let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
        // A font the page already has keeps its name.
        fonts.set(b"PCOcr".to_vec(), Object::Int(0));
        let s = String::from_utf8(text_layer(&mut doc, &mut fonts, &[w])).unwrap();
        assert!(s.contains("/PCOcr1 1 Tf") && s.contains("/PCOcr2 1 Tf"), "{s}");
        assert_eq!(fonts.get(b"PCOcr"), Some(&Object::Int(0)));
        assert!(to_unicode(&doc, &fonts, "PCOcr2").contains("<2D> <4F2B>"), "the 300th character is code 45 of the second font");
        // 255 entries are split into blocks of at most 100.
        assert_eq!(to_unicode(&doc, &fonts, "PCOcr1").matches("beginbfchar").count(), 3);
    }

    /// A word box is tight around the ink, so the text's size and baseline come from the letters:
    /// capitals, lowercase without ascenders, descenders.
    #[test]
    fn the_text_is_as_tall_as_the_scanned_word() {
        assert_eq!(ink_extent("DT"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("402337"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("sun"), (X_HEIGHT, 0.0));
        assert_eq!(ink_extent("gap"), (X_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("Gallus.Ze@camtel.cm"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("Corporation"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("--"), (BOX_EM - DESCENT, -DESCENT));
        let w = PlacedWord { text: "gap".into(), origin: [0.0, 100.0], across: [20.0, 0.0], up: [0.0, 7.3] };
        let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
        let s = String::from_utf8(text_layer(&mut doc, &mut fonts, &[w])).unwrap();
        let em = 7.3 / (X_HEIGHT + DESCENT);
        assert!(s.contains(&format!(" 0 0 {} 0 {} Tm /PCOcr 1 Tf <010203> Tj", num(em), num(100.0 + em * DESCENT))), "{s}");
    }

    #[test]
    fn rotated_pages_keep_the_reading_direction() {
        // A page turned 90°: image x runs up the page.
        let to_user = |x: f32, y: f32| [y as f64, x as f64];
        let w = PlacedWord::place(&Word { text: "Up".into(), rect: [0.0, 0.0, 50.0, 10.0] }, to_user);
        assert_eq!(w.across, [0.0, 50.0]);
        assert_eq!(w.up, [-10.0, 0.0]);
    }

    /// #103: every release package's layout puts the models where the installed app looks.
    /// Mirrors packaging/{windows/package.ps1,macos/package.sh,linux/package.sh,freebsd/package.sh}.
    #[test]
    fn finds_the_models_in_every_release_package_layout() {
        let base = std::env::temp_dir().join(format!("pdfcraft-ocr-layouts-{}", std::process::id()));
        let layouts = [
            ("windows", "PdfCraft", "PdfCraft/models"),
            ("macos", "PdfCraft.app/Contents/MacOS", "PdfCraft.app/Contents/Resources/models"),
            ("linux-deb-rpm", "usr/bin", "usr/share/pdfcraft/models"),
            ("linux-tar", "pdfcraft-1.0.0-linux-x86_64/bin", "pdfcraft-1.0.0-linux-x86_64/share/pdfcraft/models"),
            ("appimage", "PdfCraft.AppDir/usr/bin", "PdfCraft.AppDir/usr/share/pdfcraft/models"),
            ("flatpak", "app/bin", "app/share/pdfcraft/models"),
            ("freebsd", "usr/local/bin", "usr/local/share/pdfcraft/models"),
        ];
        for (name, exe_dir, models_dir) in layouts {
            let root = base.join(name);
            let (exe_dir, models_dir) = (root.join(exe_dir), root.join(models_dir));
            std::fs::create_dir_all(&exe_dir).unwrap();
            std::fs::create_dir_all(&models_dir).unwrap();
            let found = || Models::dirs_beside_exe(&exe_dir).iter().find_map(|d| Models::in_dir(d));
            assert_eq!(found(), None, "{name}: no models yet");
            std::fs::write(models_dir.join(RECOGNITION_MODEL), b"model").unwrap();
            let models = found().unwrap_or_else(|| panic!("{name}: models in {} not found", models_dir.display()));
            assert_eq!(models.recognition.canonicalize().unwrap(), models_dir.join(RECOGNITION_MODEL).canonicalize().unwrap(), "{name}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_models_message_is_for_end_users() {
        let message = OcrError::NoModels.to_string();
        assert!(!message.contains("cargo") && message.contains("PDFCRAFT_MODELS"), "{message}");
    }

    #[test]
    fn missing_runtime_message_explains_how_to_install_it() {
        assert!(OcrError::NoRuntime.to_string().contains("PDFCRAFT_TESSERACT"));
    }

    /// End to end on rendered text, when the models are installed (`cargo xtask models`).
    #[test]
    fn reads_rendered_text() {
        if let Some(reason) = Ocr::unavailable_reason() {
            eprintln!("skipped: {reason}");
            return;
        }
        let Some(models) = Models::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        let ocr = Ocr::load(&models).unwrap();
        let (w, h, px) = test_image::hello();
        let lines = ocr.recognize(&px, w, h).unwrap();
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert!(text.iter().any(|t| t.contains("HELL") && t.contains("WOR")), "{text:?}");
        let word = lines.iter().flat_map(|l| &l.words).find(|w| w.text.contains("HELL")).unwrap();
        assert!(word.rect[0] >= 10.0 && word.rect[0] < 60.0, "{word:?}");
    }

    mod test_image {
        /// "HELLO WORLD" drawn with blocky 5×7 letters, 4 pixels per dot, black on white.
        pub fn hello() -> (u32, u32, Vec<u8>) {
            const GLYPHS: &[(char, [&str; 7])] = &[
                ('H', ["X...X", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X"]),
                ('E', ["XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "XXXXX"]),
                ('L', ["X....", "X....", "X....", "X....", "X....", "X....", "XXXXX"]),
                ('O', [".XXX.", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX."]),
                ('W', ["X...X", "X...X", "X...X", "X.X.X", "X.X.X", "XX.XX", ".X.X."]),
                ('R', ["XXXX.", "X...X", "X...X", "XXXX.", "X.X..", "X..X.", "X...X"]),
                ('D', ["XXXX.", "X...X", "X...X", "X...X", "X...X", "X...X", "XXXX."]),
            ];
            let text = "HELLO WORLD";
            let (dot, x0, y0) = (4u32, 40u32, 40u32);
            let (w, h) = (x0 * 2 + text.len() as u32 * 6 * dot, y0 * 2 + 7 * dot);
            let mut px = vec![255u8; (w * h * 4) as usize];
            for (i, c) in text.chars().enumerate() {
                let Some((_, rows)) = GLYPHS.iter().find(|g| g.0 == c) else { continue };
                for (ry, row) in rows.iter().enumerate() {
                    for (rx, b) in row.bytes().enumerate() {
                        if b != b'X' {
                            continue;
                        }
                        for dy in 0..dot {
                            for dx in 0..dot {
                                let x = x0 + (i as u32 * 6 + rx as u32) * dot + dx;
                                let y = y0 + ry as u32 * dot + dy;
                                let o = ((y * w + x) * 4) as usize;
                                px[o..o + 3].copy_from_slice(&[0, 0, 0]);
                            }
                        }
                    }
                }
            }
            (w, h, px)
        }
    }
}
