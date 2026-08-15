//! Everything-style path queries: space-separated terms are AND, `|` is OR,
//! `!` is NOT, `*`/`?` are wildcards, `/` and `\` are equivalent, and quotes
//! group a phrase. Matching is case-insensitive against a full path.

use crate::sort::wildcard_match;

/// A compiled Everything-style filename/path query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// AND of OR-groups.
    clauses: Vec<Clause>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Clause {
    alts: Vec<Term>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Term {
    Include(Atom),
    Exclude(Atom),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Atom {
    /// Lowercased, slashes normalized to `/`.
    Substring(String),
    /// Lowercased glob bytes (`*` / `?`).
    Wildcard(Vec<u8>),
}

impl Query {
    pub fn parse(input: &str) -> Self {
        let mut clauses = Vec::new();
        for token in tokenize(input) {
            if token.is_empty() {
                continue;
            }
            let alts: Vec<Term> = token
                .split('|')
                .filter(|s| !s.is_empty())
                .map(parse_term)
                .collect();
            if !alts.is_empty() {
                clauses.push(Clause { alts });
            }
        }
        Self { clauses }
    }

    pub fn is_empty(&self) -> bool {
        self.clauses.is_empty()
    }

    /// True if `path` (file name or full path) satisfies every AND clause.
    pub fn matches(&self, path: &str) -> bool {
        if self.clauses.is_empty() {
            return true;
        }
        let norm = normalize(path);
        self.clauses.iter().all(|c| c.matches(&norm))
    }

    /// If `Some`, matching entries' names must end with this lowercase suffix
    /// (used to skip MFT path resolution for queries like `*.txt`).
    pub fn required_name_suffix(&self) -> Option<String> {
        for clause in &self.clauses {
            if clause.alts.len() != 1 {
                continue;
            }
            if let Term::Include(Atom::Wildcard(pat)) = &clause.alts[0] {
                if let Some(suf) = leading_star_suffix(pat) {
                    if !suf.is_empty()
                        && !suf.contains(&b'*')
                        && !suf.contains(&b'?')
                        && !suf.contains(&b'/')
                    {
                        return Some(String::from_utf8_lossy(suf).into_owned());
                    }
                }
            }
        }
        None
    }
}

impl Clause {
    fn matches(&self, norm: &str) -> bool {
        self.alts.iter().any(|t| t.matches(norm))
    }
}

impl Term {
    fn matches(&self, norm: &str) -> bool {
        match self {
            Term::Include(a) => a.matches(norm),
            Term::Exclude(a) => !a.matches(norm),
        }
    }
}

impl Atom {
    fn matches(&self, norm: &str) -> bool {
        match self {
            Atom::Substring(s) => norm.contains(s),
            Atom::Wildcard(p) => wildcard_match(p, norm.as_bytes()),
        }
    }
}

fn parse_term(raw: &str) -> Term {
    let (neg, body) = if let Some(rest) = raw.strip_prefix('!') {
        (true, rest)
    } else {
        (false, raw)
    };
    let atom = parse_atom(body);
    if neg {
        Term::Exclude(atom)
    } else {
        Term::Include(atom)
    }
}

fn parse_atom(raw: &str) -> Atom {
    let norm = normalize(raw);
    if norm.contains('*') || norm.contains('?') {
        Atom::Wildcard(norm.into_bytes())
    } else {
        Atom::Substring(norm)
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\\' {
                '/'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Pattern is `*` + literal suffix with no further wildcards.
fn leading_star_suffix(pat: &[u8]) -> Option<&[u8]> {
    if pat.first() != Some(&b'*') {
        return None;
    }
    let rest = &pat[1..];
    if rest.contains(&b'*') || rest.contains(&b'?') {
        None
    } else {
        Some(rest)
    }
}

fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in input.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_path_query() {
        let q = Query::parse(r"c:/ troja/ *.txt");
        assert!(q.matches(r"c:\users\troja\docs\notes.txt"));
        assert!(q.matches(r"C:/foo/troja/bar/a.txt"));
        assert!(!q.matches(r"c:\users\trojan\file.txt"));
        assert!(!q.matches(r"c:\users\troja\docs\notes.rs"));
        assert!(!q.matches(r"d:\troja\a.txt"));
        assert!(!q.matches(r"c:\users\other\file.txt"));
        assert!(q.matches(r"c:\users\troja\deep\nested\x.txt"));
    }

    #[test]
    fn troja_slash_is_path_segment() {
        let q = Query::parse("troja/");
        assert!(q.matches(r"c:\users\troja\file.txt"));
        assert!(!q.matches(r"c:\users\trojan\file.txt"));
        let q = Query::parse("troja");
        assert!(q.matches(r"c:\users\trojan\file.txt"));
    }

    #[test]
    fn or_and_not_and_quotes() {
        let q = Query::parse(r"*.txt|*.rs !temp");
        assert!(q.matches(r"c:\src\main.rs"));
        assert!(q.matches(r"c:\docs\a.txt"));
        assert!(!q.matches(r"c:\temp\a.txt"));
        let q = Query::parse(r#""new folder" *.txt"#);
        assert!(q.matches(r"c:\docs\new folder\a.txt"));
        assert!(!q.matches(r"c:\docs\new\folder\a.txt"));
    }

    #[test]
    fn suffix_prefilter() {
        let q = Query::parse(r"c:/ *.txt");
        assert_eq!(q.required_name_suffix().as_deref(), Some(".txt"));
        let q = Query::parse("readme");
        assert_eq!(q.required_name_suffix(), None);
    }
}
