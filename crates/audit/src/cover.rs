//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A single closed `m`/`l` subpath
//! whose four corners form an axis-aligned rectangle in user space is the same shape and counts
//! too (#1510). Graphics state tracked here is deliberately small: the CTM stack (`q`/`Q`/`cm`),
//! the non-stroking color (`g`/`rg`/`k`) and the non-stroking alpha from ExtGState (`gs` reading
//! `/ca`). Form XObjects are followed read-only with a depth cap. Anything fancier (other filled paths, shadings, patterns) is
//! out of scope and documented as such.
//!
//! Paint order: a page's content streams are one stream in pieces (ISO 32000-2 §7.8.2), so they
//! are parsed joined and walked once with a single graphics state — state set in an earlier
//! stream (a `cm`, a fill color) is still in force in the later ones. Each rectangle records the
//! position of the fill operator that painted it, as `(content-stream index, operator index in
//! the joined stream)`; rectangles inside a form XObject carry the `Do` operator's position.
//! Positions compare lexicographically in paint order.

use pdfcraft_content::{Matrix, Op, Pieces, parse};
use pdfcraft_cos::{Dict, Document, Object};

use crate::AuditError;

/// Maximum form-XObject nesting followed. Deeper nesting (including a cycle) fails the audit: a
/// cover drawn below the cut could otherwise go unseen and the page be reported clean.
const MAX_DEPTH: usize = 32;
/// Bound on total operators walked per page, so a hostile stream cannot spin forever.
/// Hitting it fails the audit: a truncated walk must never report "clean".
const MAX_OPS: usize = 2_000_000;

/// One opaque filled rectangle, in page user space.
#[derive(Clone, Debug, PartialEq)]
pub struct FilledRect {
    /// `[x0, y0, x1, y1]`, normalized so `x0 <= x1` and `y0 <= y1`.
    pub rect: [f64; 4],
    /// The fill color as sRGB-ish `[r, g, b]` (CMYK is converted approximately).
    pub rgb: [f64; 3],
    /// Paint position: `(content-stream index, operator index in the joined stream)`.
    /// Rectangles inside a form XObject carry the `Do` operator's position.
    pub pos: (usize, usize),
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    fill: [f64; 3],
    /// Components of the non-stroking colour space when it is gray, RGB or CMYK-like (1, 3 or
    /// 4); `None` for any other space, whose colours are left as the space's initial black.
    fill_space: Option<usize>,
    alpha: f64,
}

/// The path under construction, as far as the audit cares about it.
enum Path {
    Empty,
    /// A `re` rectangle, in user space.
    Rect([f64; 4]),
    /// One `m`/`l` subpath, corners already mapped to user space; `true` once closed by `h`.
    Lines(Vec<[f64; 2]>, bool),
    /// Anything else: curves, a second subpath, a CTM change mid-path.
    Other,
}

impl Path {
    /// The rectangle a fill of this path paints, if the path is one.
    fn rect(self) -> Option<[f64; 4]> {
        match self {
            Path::Rect(r) => Some(r),
            Path::Lines(pts, _) => lines_rect(pts),
            Path::Empty | Path::Other => None,
        }
    }
}

/// The bounding box of a closed polygon that is an axis-aligned rectangle, or `None`. A repeated
/// start point is dropped first; the remaining four corners must alternate horizontal and
/// vertical edges.
fn lines_rect(mut pts: Vec<[f64; 2]>) -> Option<[f64; 4]> {
    // CTM products leave rounding noise, so corners compare with a tolerance (in points).
    const EPS: f64 = 1e-6;
    let same = |a: f64, b: f64| (a - b).abs() <= EPS;
    if pts.len() == 5 && same(pts[0][0], pts[4][0]) && same(pts[0][1], pts[4][1]) {
        pts.pop();
    }
    let [a, b, c, d] = <[[f64; 2]; 4]>::try_from(pts).ok()?;
    let fits = |first_horizontal: bool| {
        [(a, b), (b, c), (c, d), (d, a)]
            .iter()
            .enumerate()
            .all(|(i, (p, q))| if (i % 2 == 0) == first_horizontal { same(p[1], q[1]) } else { same(p[0], q[0]) })
    };
    if !(fits(true) || fits(false)) {
        return None;
    }
    let (xs, ys) = ([a[0], b[0], c[0], d[0]], [a[1], b[1], c[1], d[1]]);
    let r = [
        xs.into_iter().fold(f64::INFINITY, f64::min),
        ys.into_iter().fold(f64::INFINITY, f64::min),
        xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
        ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
    ];
    (r[2] - r[0] > EPS && r[3] - r[1] > EPS).then_some(r)
}

impl Default for Gs {
    fn default() -> Self {
        Gs { ctm: Matrix::IDENTITY, fill: [0.0, 0.0, 0.0], fill_space: Some(1), alpha: 1.0 }
    }
}

struct Walker<'a> {
    doc: &'a Document,
    pieces: &'a Pieces,
    out: Vec<FilledRect>,
    ops_seen: usize,
}

impl<'a> Walker<'a> {
    fn bump(&mut self) -> Result<(), AuditError> {
        self.ops_seen += 1;
        if self.ops_seen > MAX_OPS {
            return Err(AuditError::TooManyOps);
        }
        Ok(())
    }

    /// Walk one operator slice with the given graphics state. `at_do` is the `Do` position to
    /// stamp on rectangles found inside a form XObject, or `None` at page level (where each
    /// fill operator's own position is used).
    fn walk_ops(
        &mut self,
        ops: &[Op],
        gs: &mut Gs,
        stack: &mut Vec<Gs>,
        resources: &Dict,
        depth: usize,
        at_do: Option<(usize, usize)>,
    ) -> Result<(), AuditError> {
        let mut path = Path::Empty;
        for (i, op) in ops.iter().enumerate() {
            self.bump()?;
            // Paint position of this operator: its own `(piece, joined index)` at page level,
            // the enclosing `Do`'s position inside a form XObject.
            let pos = match at_do {
                Some(p) => p,
                None => (self.pieces.pieces_of(op).0, i),
            };
            match op.op.as_slice() {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    if let Some(g) = stack.pop() {
                        *gs = g;
                    }
                    path = Path::Empty;
                }
                b"cm" => {
                    let before = gs.ctm;
                    if let Some(m) = Matrix::from_operands(&op.operands)
                        && m.0.iter().all(|v| v.is_finite())
                    {
                        gs.ctm = m.then(&gs.ctm);
                    }
                    // A `cm` that leaves the CTM unchanged (an identity matrix) cannot move the
                    // pending path, so keep it for the fill (#1333).
                    if gs.ctm != before && !matches!(path, Path::Empty) {
                        path = Path::Other;
                    }
                }
                b"g" | b"rg" | b"k" => {
                    let n = op.operands.len();
                    if let Some(rgb) = colour(&op.operands) {
                        gs.fill = rgb;
                    }
                    gs.fill_space = Some(n);
                }
                // A colour space and a colour in it (a white figure background painted as
                // `/Cs1 cs 1 sc` was taken for a black cover). Selecting a space sets its
                // initial colour, black; a colour in a space that isn't gray, RGB or CMYK-like
                // (a separation, an indexed palette, a pattern) stays black, so a possible
                // cover is still reported rather than missed.
                b"cs" => {
                    gs.fill_space = op.name(0).and_then(|name| space_components(self.doc, resources, name));
                    gs.fill = [0.0, 0.0, 0.0];
                }
                b"sc" | b"scn" => {
                    if gs.fill_space == Some(op.operands.len())
                        && let Some(rgb) = colour(&op.operands)
                    {
                        gs.fill = rgb;
                    }
                }
                b"gs" => {
                    if let Some(name) = op.name(0) {
                        gs.alpha = ext_alpha(self.doc, resources, name);
                    }
                }
                b"re" => {
                    let rect = op.nums::<4>().and_then(|[x, y, w, h]| {
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w != 0.0 && h != 0.0 {
                            let r = gs.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                            (r[2] > r[0] && r[3] > r[1]).then_some(r)
                        } else {
                            None
                        }
                    });
                    path = rect.map_or(Path::Other, Path::Rect);
                }
                b"m" => {
                    path = match (path, user_point(gs, op)) {
                        (Path::Empty, Some(p)) => Path::Lines(vec![p], false),
                        _ => Path::Other,
                    };
                }
                b"l" => {
                    path = match (path, user_point(gs, op)) {
                        // Past a repeated start point it can no longer be a rectangle.
                        (Path::Lines(mut pts, false), Some(p)) if pts.len() < 5 => {
                            pts.push(p);
                            Path::Lines(pts, false)
                        }
                        _ => Path::Other,
                    };
                }
                b"h" => {
                    path = match path {
                        Path::Lines(pts, _) => Path::Lines(pts, true),
                        _ => Path::Other,
                    };
                }
                b"f" | b"F" | b"f*" | b"B" | b"B*" => {
                    if let Some(r) = std::mem::replace(&mut path, Path::Empty).rect()
                        && gs.alpha >= 1.0
                    {
                        self.out.push(FilledRect { rect: r, rgb: gs.fill, pos });
                    }
                }
                b"S" | b"s" | b"n" => path = Path::Empty,
                b"c" | b"v" | b"y" | b"W" | b"W*" => path = Path::Other,
                b"Do" => {
                    if let Some(name) = op.name(0) {
                        if depth >= MAX_DEPTH {
                            return Err(AuditError::TooDeep);
                        }
                        self.walk_xobject(name, gs, resources, depth, pos)?;
                    }
                    path = Path::Empty;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Follow a form XObject read-only, composing its matrix onto the current CTM. State
    /// changes inside never leak out; every rectangle found is stamped with the `Do`
    /// position `pos`.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, resources: &Dict, depth: usize, pos: (usize, usize)) -> Result<(), AuditError> {
        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        let Some(entry) = xobjects.get(name) else { return Ok(()) };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return Ok(()) };
        let dict = &stream.dict;
        if dict.name(b"Subtype") != Some(b"Form") {
            return Ok(());
        }
        let matrix = dict
            .get(b"Matrix")
            .map(|o| self.doc.resolve(o))
            .and_then(|o| o.as_array().cloned())
            .and_then(|a| {
                let v: Vec<f64> = a.iter().filter_map(|o| self.doc.resolve(o).as_f64()).collect();
                (v.len() == 6 && v.iter().all(|x| x.is_finite())).then(|| Matrix([v[0], v[1], v[2], v[3], v[4], v[5]]))
            })
            .unwrap_or(Matrix::IDENTITY);
        let sub_resources = dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        // A form stream that cannot be decoded fails the audit: it might hide a cover.
        let data = stream.decoded_strict()?;
        let ops = parse(&data).ops;
        // The XObject matrix maps form space into the user space active at `Do`.
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        let mut sub_stack = Vec::new();
        self.walk_ops(&ops, &mut sub_gs, &mut sub_stack, &sub_resources, depth + 1, Some(pos))
    }
}

/// The `x y` operands of `m`/`l`, mapped to user space.
fn user_point(gs: &Gs, op: &Op) -> Option<[f64; 2]> {
    let [x, y] = op.nums::<2>().filter(|v| v.iter().all(|n| n.is_finite()))?;
    let (x, y) = gs.ctm.apply(x, y);
    Some([x, y])
}

/// The non-stroking alpha (`/ca`) of a named ExtGState, defaulting to opaque.
/// A gray, RGB or CMYK colour given by its operands, as RGB (`None` unless they are 1, 3 or 4
/// finite numbers).
fn colour(operands: &[Object]) -> Option<[f64; 3]> {
    let v: Vec<f64> = operands.iter().map(|o| o.as_f64().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0))).collect::<Option<_>>()?;
    match *v.as_slice() {
        [g] => Some([g, g, g]),
        [r, g, b] => Some([r, g, b]),
        // Naive CMYK -> RGB; good enough to tell black from white.
        [c, m, y, k] => Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]),
        _ => None,
    }
}

/// How many components the colour space `name` has when it is gray, RGB or CMYK-like: the
/// device spaces, CalGray, CalRGB and ICC-based spaces with 1, 3 or 4 components. `None` for
/// any other (separations, indexed, Lab, patterns) or one that can't be read.
fn space_components(doc: &Document, resources: &Dict, name: &[u8]) -> Option<usize> {
    match name {
        b"DeviceGray" | b"G" | b"CalGray" => return Some(1),
        b"DeviceRGB" | b"RGB" | b"CalRGB" => return Some(3),
        b"DeviceCMYK" | b"CMYK" => return Some(4),
        _ => {}
    }
    let space = resources.get(b"ColorSpace").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned())?.get(name).map(|o| doc.resolve(o))?;
    match &*space {
        Object::Name(n) if n.as_slice() != name => space_components(doc, &Dict::default(), n),
        Object::Array(a) => {
            let family = a.first()?.as_name()?;
            match family {
                b"CalGray" => Some(1),
                b"CalRGB" => Some(3),
                b"ICCBased" => {
                    let n = doc.resolve(a.get(1)?).as_dict()?.get(b"N").and_then(|o| doc.resolve(o).as_f64())?;
                    [1.0, 3.0, 4.0].contains(&n).then_some(n as usize)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn ext_alpha(doc: &Document, resources: &Dict, name: &[u8]) -> f64 {
    let alpha = resources
        .get(b"ExtGState")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(name).map(|o| doc.resolve(o)))
        .and_then(|o| o.as_dict().cloned())
        .and_then(|d| d.get(b"ca").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()));
    alpha.filter(|a| a.is_finite()).map(|a| a.clamp(0.0, 1.0)).unwrap_or(1.0)
}

/// Every opaque filled rectangle drawn by a page's content (and its form XObjects), with paint
/// positions. A page's content streams are walked once, joined, with one graphics state, so
/// state set in an earlier stream is still in force in the later ones. Any content stream that
/// cannot be decoded is an audit failure, never an empty result.
pub fn filled_rects(doc: &Document, page: &Dict) -> Result<Vec<FilledRect>, AuditError> {
    let resources = page.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let contents: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut streams = Vec::with_capacity(contents.len());
    for c in &contents {
        if let Object::Stream(s) = &*doc.resolve(c) {
            streams.push(s.decoded_strict()?);
        }
    }
    let pieces = Pieces::join(&streams);
    let parsed = pieces.parse();
    let mut w = Walker { doc, pieces: &pieces, out: Vec::new(), ops_seen: 0 };
    let mut gs = Gs::default();
    let mut stack = Vec::new();
    w.walk_ops(&parsed.ops, &mut gs, &mut stack, &resources, 0, None)?;
    Ok(w.out)
}
