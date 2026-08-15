use crate::entry::FsEntry;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SortKey {
    Name,
    Size,
    Type,
    Modified,
    Created,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SortSpec {
    pub key: SortKey,
    pub ascending: bool,
    pub dirs_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self { key: SortKey::Name, ascending: true, dirs_first: true }
    }
}

/// Natural, case-insensitive comparison: "file2" < "file10", ASCII fast path,
/// zero allocation.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ab = a.as_bytes();
    let mut bb = b.as_bytes();
    loop {
        match (ab.first(), bb.first()) {
            (None, None) => return case_tiebreak(a, b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ca), Some(&cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    // Compare full digit runs numerically.
                    let (na, ra) = take_digits(ab);
                    let (nb, rb) = take_digits(bb);
                    // Longer significant run wins; equal length compares lexically.
                    let sa = trim_zeros(na);
                    let sb = trim_zeros(nb);
                    let ord = sa
                        .len()
                        .cmp(&sb.len())
                        .then_with(|| sa.cmp(sb))
                        .then_with(|| na.len().cmp(&nb.len()));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                    ab = ra;
                    bb = rb;
                } else {
                    let la = ca.to_ascii_lowercase();
                    let lb = cb.to_ascii_lowercase();
                    if la != lb {
                        // Non-ASCII bytes fall back to byte order, which keeps
                        // UTF-8 sequences grouped; good enough and fast.
                        return la.cmp(&lb);
                    }
                    ab = &ab[1..];
                    bb = &bb[1..];
                }
            }
        }
    }
}

#[inline]
fn case_tiebreak(a: &str, b: &str) -> Ordering {
    a.cmp(b)
}

#[inline]
fn take_digits(s: &[u8]) -> (&[u8], &[u8]) {
    let n = s.iter().take_while(|c| c.is_ascii_digit()).count();
    s.split_at(n)
}

#[inline]
fn trim_zeros(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|&&c| c == b'0').count();
    if n == s.len() { &s[s.len() - 1..] } else { &s[n..] }
}

/// Compare two entries under a sort spec.
pub fn entry_cmp(a: &FsEntry, b: &FsEntry, spec: SortSpec) -> Ordering {
    if spec.dirs_first {
        match (a.is_dir(), b.is_dir()) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
    }
    let ord = match spec.key {
        SortKey::Name => natural_cmp(&a.name, &b.name),
        SortKey::Size => a.size.cmp(&b.size).then_with(|| natural_cmp(&a.name, &b.name)),
        SortKey::Type => cmp_ignore_ascii(a.ext(), b.ext())
            .then_with(|| natural_cmp(&a.name, &b.name)),
        SortKey::Modified => a
            .modified
            .cmp(&b.modified)
            .then_with(|| natural_cmp(&a.name, &b.name)),
        SortKey::Created => a
            .created
            .cmp(&b.created)
            .then_with(|| natural_cmp(&a.name, &b.name)),
    };
    if spec.ascending { ord } else { ord.reverse() }
}

fn cmp_ignore_ascii(a: &str, b: &str) -> Ordering {
    let mut ia = a.bytes();
    let mut ib = b.bytes();
    loop {
        match (ia.next(), ib.next()) {
            (Some(ca), Some(cb)) => {
                let o = ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

/// Build a sorted + filtered view (indices into `entries`).
/// `filter` is an Everything-style query (spaces = AND, `*`/`?` wildcards);
/// empty means "all".
pub fn build_view(entries: &[FsEntry], spec: SortSpec, filter: &str, show_hidden: bool) -> Vec<u32> {
    let filter = filter.trim();
    let matcher = if filter.is_empty() {
        None
    } else {
        Some(crate::query::Query::parse(filter))
    };
    let mut view: Vec<u32> = (0..entries.len() as u32)
        .filter(|&i| {
            let e = &entries[i as usize];
            if !show_hidden && e.is_hidden() {
                return false;
            }
            match &matcher {
                Some(m) => m.matches(&e.name),
                None => true,
            }
        })
        .collect();
    view.sort_unstable_by(|&x, &y| entry_cmp(&entries[x as usize], &entries[y as usize], spec));
    view
}

/// Case-insensitive wildcard matcher supporting `*` and `?`.
/// A pattern without wildcards behaves as substring search (XYplorer-style).
pub struct Wildcard {
    pattern: Vec<u8>,
    substring: bool,
}

impl Wildcard {
    pub fn new(pattern: &str) -> Self {
        let has_wild = pattern.contains('*') || pattern.contains('?');
        Self {
            pattern: pattern.to_ascii_lowercase().into_bytes(),
            substring: !has_wild,
        }
    }

    pub fn matches(&self, name: &str) -> bool {
        if self.substring {
            let needle = std::str::from_utf8(&self.pattern).unwrap_or("");
            return name.to_ascii_lowercase().contains(needle);
        }
        let lower = name.to_ascii_lowercase();
        wildcard_match(&self.pattern, lower.as_bytes())
    }
}

/// Iterative glob match with backtracking; O(n*m) worst case, no recursion.
pub(crate) fn wildcard_match(pat: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let (mut star_p, mut star_t) = (usize::MAX, 0usize);
    while t < text.len() {
        if p < pat.len() && (pat[p] == b'?' || pat[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == b'*' {
            star_p = p;
            star_t = t;
            p += 1;
        } else if star_p != usize::MAX {
            p = star_p + 1;
            star_t += 1;
            t = star_t;
        } else {
            return false;
        }
    }
    while p < pat.len() && pat[p] == b'*' {
        p += 1;
    }
    p == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("File2", "file2"), Ordering::Less); // case tiebreak
        assert_eq!(natural_cmp("a", "b"), Ordering::Less);
        assert_eq!(natural_cmp("a01", "a1"), Ordering::Greater);
    }

    #[test]
    fn wildcards() {
        assert!(Wildcard::new("*.rs").matches("main.rs"));
        assert!(!Wildcard::new("*.rs").matches("main.rc"));
        assert!(Wildcard::new("ma?n").matches("main"));
        assert!(Wildcard::new("main").matches("domain main.txt")); // substring mode
    }
}
