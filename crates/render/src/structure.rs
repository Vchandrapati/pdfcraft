//! Read-only structure inspection backed by the lazy COS reader, in the inspector's object
//! vocabulary ([`crate::pdfobj`]), loading only the objects its queries use.

use std::collections::{BTreeMap, HashSet};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

use crate::pdfobj::{Dictionary, Error, Object, ObjectId, Result, Stream, StringFormat};
use pdfcraft_cos::{Document, Object as CosObject};

pub(crate) trait Structure {
    fn trailer(&self) -> &Dictionary;
    fn get_object(&self, id: ObjectId) -> Result<&Object>;
    fn encrypted(&self) -> bool;

    fn dereference<'a>(&'a self, mut object: &'a Object) -> Result<(Option<ObjectId>, &'a Object)> {
        let mut id = None;
        for _ in 0..128 {
            match object {
                Object::Reference(reference) => {
                    id = Some(*reference);
                    object = self.get_object(*reference)?;
                }
                _ => return Ok((id, object)),
            }
        }
        Err(Error::ReferenceLimit)
    }

    fn get_dictionary(&self, id: ObjectId) -> Result<&Dictionary> {
        self.get_object(id).and_then(|o| self.dereference(o)).and_then(|(_, o)| o.as_dict())
    }

    fn catalog(&self) -> Result<&Dictionary> {
        self.trailer().get(b"Root").and_then(Object::as_reference).and_then(|id| self.get_dictionary(id))
    }

    fn page_ids(&self) -> Vec<ObjectId> {
        let mut pages = Vec::new();
        let Ok(root) = self.catalog().and_then(|c| c.get(b"Pages")).and_then(Object::as_reference) else { return pages };
        let mut pending = vec![(root, 0usize)];
        let mut seen = HashSet::new();
        while let Some((id, depth)) = pending.pop() {
            if depth > 256 || !seen.insert(id) {
                continue;
            }
            let Ok(dict) = self.get_dictionary(id) else { continue };
            match dict.get_type() {
                Ok(b"Page") => {
                    // Keep the effective u32 page-count bound without trusting /Count.
                    if u32::try_from(pages.len()).ok().and_then(|n| n.checked_add(1)).is_none() {
                        return pages;
                    }
                    pages.push(id);
                }
                Ok(b"Pages") => {
                    if let Ok(kids) = dict.get(b"Kids").and_then(|o| self.dereference(o)).and_then(|(_, o)| o.as_array()) {
                        pending.extend(kids.iter().rev().filter_map(|k| k.as_reference().ok()).map(|id| (id, depth + 1)));
                    }
                }
                _ => {}
            }
        }
        pages
    }

    // Font-resource precedence: a page's direct resources, then its indirect resources and
    // ancestors. Stop cycles and excessive inheritance depth.
    fn get_page_fonts(&self, page: ObjectId) -> Result<BTreeMap<Vec<u8>, &Dictionary>> {
        let mut resources = Vec::new();
        let mut current = Some(page);
        let mut seen = HashSet::new();
        while let Some(id) = current {
            if !seen.insert(id) {
                return Err(Error::ReferenceCycle(id));
            }
            if seen.len() > 100 {
                return Err(Error::RecursionLimit);
            }
            let dict = self.get_dictionary(id)?;
            if let Ok(resource) = dict.get(b"Resources") {
                match resource {
                    Object::Dictionary(dict) if id == page => resources.push(dict),
                    Object::Reference(id) => {
                        if let Ok(dict) = self.get_dictionary(*id) {
                            resources.push(dict);
                        }
                    }
                    _ => {}
                }
            }
            current = dict.get(b"Parent").and_then(Object::as_reference).ok();
        }
        let mut fonts = BTreeMap::new();
        for resources in resources {
            if let Ok(dict) = resources.get(b"Font").and_then(|o| self.dereference(o)).and_then(|(_, o)| o.as_dict()) {
                for (name, font) in dict.iter() {
                    if let Ok(font) = self.dereference(font).and_then(|(_, o)| o.as_dict()) {
                        fonts.entry(name.clone()).or_insert(font);
                    }
                }
            }
        }
        Ok(fonts)
    }
}

type Slot = (u32, OnceLock<Option<Box<Object>>>);

pub(crate) struct LazyStructure {
    source: Document,
    trailer: Dictionary,
    // Sorted slots keep returned references stable, without allocating by the largest
    // input object number (which can be sparse or hostile). Only queried objects fill slots.
    objects: Vec<Slot>,
    failed: AtomicBool,
}

impl LazyStructure {
    /// Read `bytes` for inspection. A damaged file is read as COS repairs it (its cross-reference
    /// data rebuilt from the object headers); `repair_log` says what was done.
    pub(crate) fn new(bytes: Arc<Vec<u8>>, password: Option<&str>) -> std::result::Result<Self, String> {
        let source = Document::open_with_stream_limit(bytes, password, 256 << 20).map_err(|e| e.to_string())?;
        let trailer = convert_dict(source.trailer(), 0).map_err(|e| e.to_string())?;
        let objects = source.object_numbers().into_iter().map(|n| (n, OnceLock::new())).collect();
        Ok(Self { source, trailer, objects, failed: AtomicBool::new(false) })
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// The decoded data of stream `id`, at most `max` bytes (a decompression bomb is refused).
    pub(crate) fn stream_data(&self, id: ObjectId, max: usize) -> std::result::Result<Vec<u8>, String> {
        if self.source.generation(id.0) != id.1 {
            return Err(Error::ObjectNotFound(id).to_string());
        }
        match self.source.try_get(id.0).map_err(|e| e.to_string())?.as_ref() {
            CosObject::Stream(stream) => stream.decoded_within(max).map_err(|e| e.to_string()),
            _ => Err("not a stream".into()),
        }
    }
}

impl Structure for LazyStructure {
    fn trailer(&self) -> &Dictionary {
        &self.trailer
    }
    fn encrypted(&self) -> bool {
        self.source.security().is_some() || self.trailer.has(b"Encrypt")
    }
    fn get_object(&self, id: ObjectId) -> Result<&Object> {
        let missing = || Error::ObjectNotFound(id);
        let slot = self.objects.binary_search_by_key(&id.0, |(n, _)| *n).ok().and_then(|i| self.objects.get(i)).ok_or_else(missing)?;
        if self.source.generation(id.0) != id.1 {
            return Err(missing());
        }
        slot.1
            .get_or_init(|| {
                let result = self.source.try_get(id.0).ok().and_then(|o| convert(&o, 0).ok()).map(Box::new);
                if result.is_none() {
                    self.failed.store(true, Ordering::Relaxed);
                }
                result
            })
            .as_deref()
            .ok_or_else(missing)
    }
}

fn convert_dict(dict: &pdfcraft_cos::Dict, depth: usize) -> Result<Dictionary> {
    if depth > 100 {
        return Err(Error::RecursionLimit);
    }
    dict.iter().map(|(key, value)| Ok((key.clone(), convert(value, depth + 1)?))).collect::<Result<Vec<_>>>().map(|items| items.into_iter().collect())
}

fn convert(object: &CosObject, depth: usize) -> Result<Object> {
    if depth > 100 {
        return Err(Error::RecursionLimit);
    }
    Ok(match object {
        CosObject::Null => Object::Null,
        CosObject::Bool(value) => Object::Boolean(*value),
        CosObject::Int(value) => Object::Integer(*value),
        CosObject::Real(value) => Object::Real(*value as f32),
        CosObject::Name(value) => Object::Name(value.clone()),
        CosObject::String(value) => Object::String(value.bytes.clone(), if value.hex { StringFormat::Hexadecimal } else { StringFormat::Literal }),
        CosObject::Ref(value) => Object::Reference((value.num, value.generation)),
        CosObject::Array(value) => Object::Array(value.iter().map(|o| convert(o, depth + 1)).collect::<Result<_>>()?),
        CosObject::Dict(value) => Object::Dictionary(convert_dict(value, depth + 1)?),
        CosObject::Stream(value) => Object::Stream(Stream { dict: convert_dict(&value.dict, depth + 1)?, content: value.raw.to_vec() }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDF holding `objects` (number, body) under a cross-reference table with free entries for
    /// the numbers left out, and `root` as its catalog.
    fn pdf_with(objects: &[(u32, String)], root: u32) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = std::collections::BTreeMap::new();
        for (n, body) in objects {
            offsets.insert(*n, out.len());
            out.extend_from_slice(format!("{n} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let size = offsets.keys().max().map_or(1, |n| n + 1);
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
        for n in 0..size {
            let line = match offsets.get(&n) {
                Some(offset) => format!("{offset:010} 00000 n \n"),
                None => "0000000000 65535 f \n".to_string(),
            };
            out.extend_from_slice(line.as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {size} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        out
    }

    /// A hostile page-tree /Count must not abort the process (fuzz finding: /Count 4294967295 made
    /// a reader that trusted it pre-allocate 48 GiB): the bounded walk counts the real pages.
    #[test]
    fn a_hostile_page_count_does_not_abort_inspection() {
        let pdf = pdf_with(
            &[
                (1, "<< /Type /Catalog /Pages 2 0 R >>".into()),
                (2, "<< /Type /Pages /Kids [3 0 R] /Count 4294967295 >>".into()),
                (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>".into()),
            ],
            1,
        );
        let doc = LazyStructure::new(Arc::new(pdf), None).unwrap();
        assert_eq!(doc.page_ids(), [(3, 0)], "the walk counts the one real page, not /Count");
    }

    fn nested_sparse_page_tree(repeated_and_cyclic: bool) -> Vec<u8> {
        let nested_kids = if repeated_and_cyclic { "[12 0 R 5 0 R 31 0 R 42 0 R]" } else { "[12 0 R 42 0 R]" };
        let (count, nested_count) = if repeated_and_cyclic { (5, 4) } else { (3, 2) };
        let mut objects = vec![
            (17, "<< /Type /Catalog /Pages 5 0 R >>".to_string()),
            (5, format!("<< /Type /Pages /Kids [31 0 R 88 0 R] /Count {count} >>")),
            (88, format!("<< /Type /Pages /Parent 5 0 R /Kids {nested_kids} /Count {nested_count} >>")),
        ];
        for (id, parent) in [(31, 5), (12, 88), (42, 88)] {
            objects.push((id, format!("<< /Type /Page /Parent {parent} 0 R >>")));
        }
        pdf_with(&objects, 17)
    }

    #[test]
    fn page_walk_keeps_nested_sparse_order_and_first_visit_semantics() {
        let expected = [(31, 0), (12, 0), (42, 0)];
        for repeated_and_cyclic in [false, true] {
            let doc = LazyStructure::new(Arc::new(nested_sparse_page_tree(repeated_and_cyclic)), None).unwrap();
            assert_eq!(doc.page_ids(), expected, "repeated and cyclic kids: {repeated_and_cyclic}");
        }
    }

    #[test]
    fn inspection_loads_only_requested_objects_in_a_compressed_document() {
        // Keep the unused objects reachable when the full writer removes orphan objects, through a
        // catalog extension that the panel inspector has no reason to read.
        let unused: String = (0..300).map(|i| format!("{} 0 R ", 4 + i)).collect();
        let mut objects = vec![
            (1, format!("<< /Type /Catalog /Pages 2 0 R /Uninspected [{unused}] >>")),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string()),
            (3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>".to_string()),
        ];
        objects.extend((0..300).map(|i| (4 + i, format!("<< /Unused {i} >>"))));
        let cos = Document::open(Arc::new(pdf_with(&objects, 1))).unwrap();
        let compressed = pdfcraft_cos::write_full(&cos, &pdfcraft_cos::SaveOptions::default()).unwrap();
        let view = LazyStructure::new(Arc::new(compressed), None).unwrap();
        assert_eq!(view.page_ids().len(), 1);
        assert!(!view.failed());
        assert!(
            view.objects.iter().filter(|(_, slot)| slot.get().is_some()).count() <= 3,
            "unused objects must not be materialized to inspect a page tree"
        );
        let unused = view.catalog().unwrap().get(b"Uninspected").unwrap().as_array().unwrap();
        let last = unused.last().unwrap().as_reference().unwrap();
        assert!(matches!(view.get_dictionary(last).unwrap().get(b"Unused"), Ok(Object::Integer(299))));
    }
}
