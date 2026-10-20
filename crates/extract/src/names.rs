//! Mapping ISO 9660 names to valid, unique FAT32 long file names.

use std::collections::{HashMap, HashSet};

pub struct NameMap {
    used: HashSet<String>,
    /// Source directory path -> destination directory path.
    dirs: HashMap<String, String>,
}

impl NameMap {
    pub fn new() -> Self {
        Self {
            used: HashSet::new(),
            dirs: HashMap::new(),
        }
    }

    /// Return the destination path for `src`, allocating a unique name inside
    /// its (already mapped) parent directory.
    pub fn resolve(&mut self, src: &str, is_dir: bool) -> String {
        let (parent_src, base_src) = match src.rfind('/') {
            Some(pos) => (&src[..pos], &src[pos + 1..]),
            None => ("", src),
        };
        let parent_dest = if parent_src.is_empty() {
            String::new()
        } else {
            self.dirs
                .get(parent_src)
                .cloned()
                // The walker emits parents first; fall back to a sanitized
                // path if an image somehow violates that order.
                .unwrap_or_else(|| sanitize_path(parent_src))
        };

        let base = sanitize_component(base_src);
        let name = self.unique(&parent_dest, &base);
        let dest = join(&parent_dest, &name);
        if is_dir {
            self.dirs.insert(src.to_owned(), dest.clone());
        }
        dest
    }

    fn unique(&mut self, parent: &str, base: &str) -> String {
        let mut candidate = base.to_owned();
        let mut counter = 2u32;
        loop {
            let key = join(parent, &candidate).to_lowercase();
            if !self.used.contains(&key) {
                self.used.insert(key);
                return candidate;
            }
            candidate = add_suffix(base, counter);
            counter += 1;
            if counter > 9999 {
                candidate = format!("_{:x}_{counter}", self.used.len());
            }
        }
    }
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn sanitize_path(path: &str) -> String {
    path.split('/')
        .map(sanitize_component)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn sanitize_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        let ch = match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        };
        out.push(ch);
    }
    let mut out = out.trim_end_matches([' ', '.']).to_owned();
    if out.is_empty() || out == "." || out == ".." {
        out = "_".to_owned();
    }
    if is_reserved(&out) {
        out.insert(0, '_');
    }
    // FAT long names are limited to 255 UTF-16 units; keep room for a ~N
    // suffix and for path components added by the caller.
    if out.chars().count() > 120 {
        out = out.chars().take(120).collect();
    }
    out
}

fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let upper = stem.to_ascii_uppercase();
    if matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    let bytes = upper.as_bytes();
    bytes.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && bytes[3].is_ascii_digit()
        && bytes[3] != b'0'
}

fn add_suffix(base: &str, counter: u32) -> String {
    let (stem, ext) = match base.rfind('.') {
        Some(pos) if pos > 0 => (&base[..pos], &base[pos..]),
        _ => (base, ""),
    };
    let suffix = format!("~{counter}");
    let mut stem: String = stem.chars().take(110).collect();
    stem.push_str(&suffix);
    format!("{stem}{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_invalid_characters_and_reserved_names() {
        assert_eq!(sanitize_component("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("com1.txt"), "_com1.txt");
        assert_eq!(sanitize_component("trailing.  "), "trailing");
        assert_eq!(sanitize_component("..."), "_");
    }

    #[test]
    fn resolves_collisions_case_insensitively() {
        let mut names = NameMap::new();
        assert_eq!(names.resolve("README.TXT", false), "README.TXT");
        assert_eq!(names.resolve("readme.txt", false), "readme~2.txt");
        assert_eq!(names.resolve("casper", true), "casper");
        assert_eq!(names.resolve("casper/vmlinuz", false), "casper/vmlinuz");
        assert_eq!(names.resolve("casper/VMLINUZ", false), "casper/VMLINUZ~2");
    }

    #[test]
    fn keeps_parent_mapping() {
        let mut names = NameMap::new();
        assert_eq!(names.resolve("a:b", true), "a_b");
        assert_eq!(names.resolve("a:b/file", false), "a_b/file");
    }
}
