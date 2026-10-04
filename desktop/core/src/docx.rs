use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::SystemTime;

/// Page count Word stored in the .docx at its last save (docProps/app.xml <Pages>).
/// Used to suggest units for per-page drafting tariffs.
pub fn page_count(path: &Path) -> Option<u32> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ext != "docx" && ext != "docm" {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut zip = zip::ZipArchive::new(file).ok()?;
    let mut xml = String::new();
    zip.by_name("docProps/app.xml").ok()?.take(256 * 1024).read_to_string(&mut xml).ok()?;
    let start = xml.find("<Pages>")? + "<Pages>".len();
    let end = start + xml[start..].find("</Pages>")?;
    xml[start..end].trim().parse().ok()
}

/// Avoids re-opening the same document every few seconds; re-reads after each save.
#[derive(Default)]
pub struct PageCache {
    entries: HashMap<String, (Option<SystemTime>, Option<u32>)>,
}

impl PageCache {
    pub fn get(&mut self, path: &str) -> Option<u32> {
        let p = Path::new(path);
        if !p.is_absolute() {
            return None;
        }
        let modified = std::fs::metadata(p).and_then(|m| m.modified()).ok();
        if let Some((m, pages)) = self.entries.get(path) {
            if *m == modified {
                return *pages;
            }
        }
        let pages = page_count(p);
        if self.entries.len() > 500 {
            self.entries.clear();
        }
        self.entries.insert(path.to_string(), (modified, pages));
        pages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_pages_from_app_xml() {
        let dir = std::env::temp_dir().join(format!("tt-docx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Settlement.docx");
        let f = std::fs::File::create(&path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        z.start_file("docProps/app.xml", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(b"<?xml version=\"1.0\"?><Properties><Pages>12</Pages><Words>3400</Words></Properties>")
            .unwrap();
        z.finish().unwrap();
        assert_eq!(page_count(&path), Some(12));
        assert_eq!(PageCache::default().get(path.to_str().unwrap()), Some(12));
        assert_eq!(page_count(&dir.join("missing.docx")), None);
        std::fs::remove_dir_all(dir).ok();
    }
}
