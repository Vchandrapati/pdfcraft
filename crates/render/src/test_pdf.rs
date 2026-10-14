//! A minimal PDF writer for test fixtures: objects as PDF syntax, numbered in the order they are
//! added (or reserved first, for a parent its children point to), under a classic xref table.

pub(crate) struct TestPdf {
    objects: Vec<Option<Vec<u8>>>,
}

impl TestPdf {
    pub(crate) fn new() -> Self {
        Self { objects: Vec::new() }
    }

    /// A number for an object written later with [`TestPdf::set`].
    pub(crate) fn reserve(&mut self) -> u32 {
        self.objects.push(None);
        self.objects.len() as u32
    }

    /// Add an object written as PDF syntax; returns its number.
    pub(crate) fn add(&mut self, body: impl AsRef<str>) -> u32 {
        self.objects.push(Some(body.as_ref().as_bytes().to_vec()));
        self.objects.len() as u32
    }

    /// Add a stream with dictionary entries `dict` (written inside `<< >>`, without /Length).
    pub(crate) fn add_stream(&mut self, dict: &str, data: &[u8]) -> u32 {
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.objects.push(Some(body));
        self.objects.len() as u32
    }

    /// Write the reserved object `number`.
    pub(crate) fn set(&mut self, number: u32, body: impl AsRef<str>) {
        self.objects[number as usize - 1] = Some(body.as_ref().as_bytes().to_vec());
    }

    /// The file, with `root` as its catalog.
    pub(crate) fn finish(self, root: u32) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body.as_deref().unwrap_or(b"null"));
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n", self.objects.len() + 1).as_bytes());
        out
    }
}
