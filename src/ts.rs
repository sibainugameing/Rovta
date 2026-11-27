//! Lightweight TypeScript → JavaScript transform (type erasure only).
//!
//! Types are replaced with spaces, so line/column positions in error messages stay identical to
//! the source the user is editing. Supported: type annotations, optional params, `as`/`satisfies`,
//! non-null `!`, generics on functions/calls, `interface`, `type`, `declare`, `import type`,
//! `implements`. Not supported: `enum`, `namespace`, decorators, class parameter properties.

#[derive(Clone, Copy, PartialEq, Debug)]
enum K { Ident, Num, Str, Punct }

#[derive(Clone, Debug)]
struct Tok { k: K, s: usize, e: usize }

fn is_id_start(c: u8) -> bool { c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80 }
fn is_id(c: u8) -> bool { is_id_start(c) || c.is_ascii_digit() }

fn scan_str(b: &[u8], mut i: usize) -> usize {
    let q = b[i];
    i += 1;
    while i < b.len() && b[i] != q {
        if b[i] == b'\\' { i += 1; }
        i += 1;
    }
    (i + 1).min(b.len())
}

fn scan_template(b: &[u8], mut i: usize) -> usize {
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'`' => return i + 1,
            b'$' if b.get(i + 1) == Some(&b'{') => {
                i += 2;
                let mut d = 1;
                while i < b.len() && d > 0 {
                    match b[i] {
                        b'{' => { d += 1; i += 1; }
                        b'}' => { d -= 1; i += 1; }
                        b'`' => i = scan_template(b, i + 1),
                        b'"' | b'\'' => i = scan_str(b, i),
                        _ => i += 1,
                    }
                }
            }
            _ => i += 1,
        }
    }
    b.len()
}

fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut t: Vec<Tok> = vec![];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() { i += 1; continue; }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' { i += 1; }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') { i += 1; }
            i = (i + 2).min(b.len());
            continue;
        }
        let s = i;
        if is_id_start(c) {
            while i < b.len() && is_id(b[i]) { i += 1; }
            t.push(Tok { k: K::Ident, s, e: i });
        } else if c.is_ascii_digit() {
            while i < b.len() && (is_id(b[i]) || b[i] == b'.') { i += 1; }
            t.push(Tok { k: K::Num, s, e: i });
        } else if c == b'"' || c == b'\'' {
            i = scan_str(b, i);
            t.push(Tok { k: K::Str, s, e: i });
        } else if c == b'`' {
            i = scan_template(b, i + 1);
            t.push(Tok { k: K::Str, s, e: i });
        } else if c == b'/' && regex_allowed(src, t.last()) {
            i += 1;
            let mut cls = false;
            while i < b.len() && b[i] != b'\n' {
                match b[i] {
                    b'\\' => i += 1,
                    b'[' => cls = true,
                    b']' => cls = false,
                    b'/' if !cls => break,
                    _ => {}
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            while i < b.len() && is_id(b[i]) { i += 1; }
            t.push(Tok { k: K::Str, s, e: i });
        } else {
            let rest = &src[i..];
            let multi = ["===", "!==", "...", "=>", "==", "!=", "<=", ">=", "&&", "||", "??", "?."];
            let len = multi.iter().find(|m| rest.starts_with(**m)).map(|m| m.len()).unwrap_or(1);
            // `?.` followed by a digit is a ternary, not optional chaining
            let len = if len == 2 && rest.starts_with("?.") && rest.as_bytes().get(2).map_or(false, |d| d.is_ascii_digit()) { 1 } else { len };
            let len = if rest.is_char_boundary(len) { len } else { 1 };
            i += len;
            t.push(Tok { k: K::Punct, s, e: i });
        }
    }
    t
}

fn regex_allowed(src: &str, prev: Option<&Tok>) -> bool {
    match prev {
        None => true,
        Some(p) => {
            let s = &src[p.s..p.e];
            match p.k {
                K::Punct => !matches!(s, ")" | "]" | "}"),
                K::Ident => matches!(s, "return" | "typeof" | "case" | "in" | "of" | "delete" | "void" | "throw" | "yield" | "await"),
                _ => false,
            }
        }
    }
}

const NOT_VALUE: [&str; 14] = ["return", "typeof", "case", "in", "of", "delete", "void", "throw", "yield", "await", "new", "else", "do", "instanceof"];
const NOT_FN: [&str; 11] = ["if", "for", "while", "switch", "catch", "with", "return", "typeof", "else", "do", "in"];

struct S<'a> {
    src: &'a str,
    t: Vec<Tok>,
    blank: Vec<(usize, usize)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Paren { Arrow, Decl, Catch }

impl<'a> S<'a> {
    fn txt(&self, i: usize) -> &str { self.t.get(i).map_or("", |t| &self.src[t.s..t.e]) }
    fn p(&self, i: usize, s: &str) -> bool { self.t.get(i).map_or(false, |t| t.k == K::Punct && &self.src[t.s..t.e] == s) }
    fn id(&self, i: usize, s: &str) -> bool { self.t.get(i).map_or(false, |t| t.k == K::Ident && &self.src[t.s..t.e] == s) }
    fn is_ident(&self, i: usize) -> bool { self.t.get(i).map_or(false, |t| t.k == K::Ident) }
    fn blank_tok(&mut self, a: usize, b: usize) {
        if a < b && b <= self.t.len() { self.blank.push((self.t[a].s, self.t[b - 1].e)); }
    }
    fn close(&self, i: usize) -> Option<usize> {
        let mut d = 0i32;
        for j in i..self.t.len() {
            if self.t[j].k != K::Punct { continue; }
            match self.txt(j) {
                "(" | "[" | "{" => d += 1,
                ")" | "]" | "}" => { d -= 1; if d == 0 { return Some(j); } }
                _ => {}
            }
        }
        None
    }
    fn angle(&self, i: usize) -> Option<usize> {
        let mut d = 0i32;
        let mut j = i;
        while j < self.t.len() {
            if self.t[j].k == K::Punct {
                match self.txt(j) {
                    "<" => d += 1,
                    ">" => { d -= 1; if d == 0 { return Some(j + 1); } }
                    "(" | "[" | "{" => { j = self.close(j)?; }
                    ";" | "=" | "&&" | "||" | ")" | "]" | "}" => return None,
                    _ => {}
                }
            }
            j += 1;
        }
        None
    }
    /// Parse a type starting at token `i`; returns the index after it.
    fn ty(&self, mut i: usize, arrow: bool) -> Option<usize> {
        if self.p(i, "|") || self.p(i, "&") { i += 1; }
        loop {
            i = self.prim(i, arrow)?;
            while self.p(i, "[") {
                i = self.close(i)? + 1;
            }
            if self.p(i, "|") || self.p(i, "&") { i += 1; continue; }
            break;
        }
        Some(i)
    }
    fn prim(&self, i: usize, arrow: bool) -> Option<usize> {
        let t = self.t.get(i)?;
        match t.k {
            K::Str | K::Num => Some(i + 1),
            K::Ident => {
                if matches!(self.txt(i), "typeof" | "keyof" | "readonly" | "unique" | "infer") { return self.prim(i + 1, arrow); }
                let mut j = i + 1;
                while self.p(j, ".") && self.is_ident(j + 1) { j += 2; }
                if self.p(j, "<") { j = self.angle(j)?; }
                Some(j)
            }
            K::Punct => match self.txt(i) {
                "(" => {
                    let mut j = self.close(i)? + 1;
                    if arrow && self.p(j, "=>") { j = self.ty(j + 1, true)?; }
                    Some(j)
                }
                "{" | "[" => Some(self.close(i)? + 1),
                "<" => {
                    let j = self.angle(i)?;
                    if self.p(j, "(") {
                        let k = self.close(j)? + 1;
                        if self.p(k, "=>") { return self.ty(k + 1, true); }
                    }
                    None
                }
                "-" if self.t.get(i + 1).map_or(false, |t| t.k == K::Num) => Some(i + 2),
                _ => None,
            },
        }
    }
    fn value_end(&self, i: usize) -> bool {
        let t = &self.t[i];
        match t.k {
            K::Ident => !NOT_VALUE.contains(&self.txt(i)),
            K::Num | K::Str => true,
            K::Punct => matches!(self.txt(i), ")" | "]"),
        }
    }
    fn fn_like_before(&self, i: usize) -> bool {
        if i == 0 { return false; }
        let b = i - 1;
        if self.id(b, "function") { return true; }
        if !self.is_ident(b) || NOT_FN.contains(&self.txt(b)) { return false; }
        if b == 0 { return true; }
        let a = self.txt(b - 1);
        matches!(a, "function" | "*" | "{" | "}" | "," | ";" | "async" | "get" | "set" | "static" | "public" | "private" | "protected" | "readonly")
    }
    fn classify(&self, i: usize) -> Option<Paren> {
        let c = self.close(i)?;
        if i > 0 && self.id(i - 1, "catch") { return Some(Paren::Catch); }
        if self.p(c + 1, "=>") { return Some(Paren::Arrow); }
        if self.p(c + 1, ":") {
            if let Some(e) = self.ty(c + 2, false) {
                if self.p(e, "=>") { return Some(Paren::Arrow); }
                if self.p(e, "{") && self.fn_like_before(i) { return Some(Paren::Decl); }
            }
            if let Some(e) = self.ty(c + 2, true) {
                if self.p(e, "{") && self.fn_like_before(i) { return Some(Paren::Decl); }
            }
        }
        if self.p(c + 1, "{") && self.fn_like_before(i) { return Some(Paren::Decl); }
        None
    }
    fn params(&mut self, a: usize, b: usize) {
        let mut j = a;
        while j < b {
            while matches!(self.txt(j), "public" | "private" | "protected" | "readonly") && self.is_ident(j + 1) { j += 1; }
            if self.p(j, "...") { j += 1; }
            if self.p(j, "{") || self.p(j, "[") {
                match self.close(j) { Some(c) => j = c + 1, None => return }
            } else if self.is_ident(j) {
                j += 1;
            } else {
                j += 1;
                continue;
            }
            if self.p(j, "?") { self.blank_tok(j, j + 1); j += 1; }
            if self.p(j, ":") {
                match self.ty(j + 1, true) {
                    Some(e) => { self.blank_tok(j, e); j = e; }
                    None => return,
                }
            }
            if self.p(j, "=") {
                j += 1;
                while j < b && !self.p(j, ",") {
                    if self.p(j, "(") || self.p(j, "[") || self.p(j, "{") {
                        match self.close(j) { Some(c) => j = c + 1, None => return }
                    } else { j += 1; }
                }
            }
            if self.p(j, ",") { j += 1; }
        }
    }
    fn stmt_end(&self, from: usize) -> usize {
        // end of a `declare`/`import type` statement: up to and including `;`, else the end of the line
        let line_of = |s: usize| self.src[..s].bytes().filter(|&c| c == b'\n').count();
        let l0 = line_of(self.t[from].s);
        let mut j = from;
        while j < self.t.len() {
            if self.p(j, ";") { return j + 1; }
            if j > from && line_of(self.t[j].s) > l0 { return j; }
            j += 1;
        }
        j
    }
    fn var_decl(&mut self, mut j: usize) -> usize {
        loop {
            if self.p(j, "{") || self.p(j, "[") {
                match self.close(j) { Some(c) => j = c + 1, None => return j }
            } else if self.is_ident(j) {
                j += 1;
            } else { return j; }
            if self.p(j, "!") { self.blank_tok(j, j + 1); j += 1; }
            if self.p(j, ":") {
                match self.ty(j + 1, true) {
                    Some(e) => { self.blank_tok(j, e); j = e; }
                    None => return j,
                }
            }
            if self.p(j, ",") { j += 1; continue; }
            return j;
        }
    }
    fn run(&mut self) {
        let n = self.t.len();
        let mut i = 0;
        while i < n {
            if self.t[i].k == K::Ident && !(i > 0 && self.p(i - 1, ".")) {
                let w = self.txt(i).to_string();
                let w = w.as_str();
                let start = if i > 0 && self.id(i - 1, "export") { i - 1 } else { i };
                match w {
                    "import" | "export" if self.p(i + 1, "{") => { i = self.close(i + 1).map_or(i + 1, |c| c + 1); continue; }
                    "import" | "export" if self.id(i + 1, "type") && (self.p(i + 2, "{") || self.p(i + 2, "*") || (w == "import" && self.is_ident(i + 2))) => {
                        let e = self.stmt_end(i);
                        self.blank_tok(i, e); i = e; continue;
                    }
                    "interface" if self.is_ident(i + 1) => {
                        if let Some(o) = (i + 1..n).find(|&j| self.p(j, "{")) {
                            if let Some(c) = self.close(o) { self.blank_tok(start, c + 1); i = c + 1; continue; }
                        }
                    }
                    "type" if self.is_ident(i + 1) && (self.p(i + 2, "=") || self.p(i + 2, "<")) => {
                        let j = if self.p(i + 2, "<") { self.angle(i + 2).unwrap_or(i + 2) } else { i + 2 };
                        if self.p(j, "=") {
                            if let Some(k) = self.ty(j + 1, true) {
                                let e = if self.p(k, ";") { k + 1 } else { k };
                                self.blank_tok(start, e); i = e; continue;
                            }
                        }
                    }
                    "declare" if self.is_ident(i + 1) => {
                        let e = self.stmt_end(i);
                        self.blank_tok(i, e); i = e; continue;
                    }
                    "as" | "satisfies" if i > 0 && self.value_end(i - 1) => {
                        if let Some(e) = self.ty(i + 1, true) { self.blank_tok(i, e); i = e; continue; }
                    }
                    "const" | "let" | "var" => { i = self.var_decl(i + 1); continue; }
                    "implements" => {
                        if let Some(o) = (i..n).find(|&j| self.p(j, "{")) { self.blank_tok(i, o); i = o; continue; }
                    }
                    "function" => {
                        let mut j = i + 1;
                        if self.p(j, "*") { j += 1; }
                        if self.is_ident(j) { j += 1; }
                        if self.p(j, "<") {
                            if let Some(e) = self.angle(j) { self.blank_tok(j, e); j = e; }
                        }
                        if self.p(j, "(") { i = self.paren(j, Paren::Decl); continue; }
                    }
                    _ => {}
                }
                // generic call: foo<T>(…)
                if self.p(i + 1, "<") && !NOT_FN.contains(&w) {
                    if let Some(e) = self.angle(i + 1) {
                        if self.p(e, "(") { self.blank_tok(i + 1, e); i = e; continue; }
                    }
                }
                i += 1;
                continue;
            }
            if self.p(i, "(") {
                if let Some(kind) = self.classify(i) { i = self.paren(i, kind); continue; }
            } else if self.p(i, "!") && i > 0 && self.value_end(i - 1) && !self.p(i + 1, "=") {
                if matches!(self.txt(i + 1), "." | ")" | "," | ";" | "]" | "[" | "(" | "}" | "+" | "-" | "*" | "/" | "%" | "&&" | "||" | "??" | "==" | "===" | "!=" | "!==" | "<" | ">" | "<=" | ">=" | "?" | ":" | "?.") || self.t.get(i + 1).is_none() {
                    self.blank_tok(i, i + 1);
                }
            }
            i += 1;
        }
    }
    fn paren(&mut self, i: usize, kind: Paren) -> usize {
        let c = match self.close(i) { Some(c) => c, None => return i + 1 };
        self.params(i + 1, c);
        let mut j = c + 1;
        if kind != Paren::Catch && self.p(j, ":") {
            if let Some(e) = self.ty(j + 1, kind == Paren::Decl) {
                self.blank_tok(j, e);
                j = e;
            }
        }
        j
    }
}

pub fn strip_types(src: &str) -> String {
    let mut s = S { src, t: tokenize(src), blank: vec![] };
    s.run();
    let mut out = src.as_bytes().to_vec();
    for (a, b) in s.blank {
        for c in &mut out[a..b] {
            if *c != b'\n' && *c != b'\r' { *c = b' '; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn norm(s: &str) -> String { s.split_whitespace().collect::<String>() }
    fn check(src: &str, want: &str) { assert_eq!(norm(&strip_types(src)), norm(want), "src: {src}"); }

    #[test]
    fn basics() {
        check("const x: number = 5;", "const x = 5;");
        check("let a: number, b: string;", "let a, b;");
        check("function f(a: number, b?: string = 'x'): void { return; }", "function f(a, b = 'x') { return; }");
        check("export function f<T>(x: T[], o: {a: number} = {}): Array<[number, number]> { return []; }", "export function f(x, o = {}) { return []; }");
        check("const g = (a: number, { b }: { b: string }): number => a;", "const g = (a, { b }) => a;");
        check("const v = (x as any).y; const w = x! + 1;", "const v = (x).y; const w = x + 1;");
        check("interface A { x: number; y?: string }\nconst a = 1;", "\nconst a = 1;");
        check("export type Hit = [start: number, dur: number];\nconst a = 1;", "\nconst a = 1;");
        check("import type { A } from './a';\nimport { b as c } from './b';", "\nimport { b as c } from './b';");
        check("const o = { a: 1, b: x ? y : z };", "const o = { a: 1, b: x ? y : z };");
        check("const f: (a: number) => void = () => {};", "const f = () => {};");
        check("foo<number>(1); if (a < b && c > (d)) {}", "foo(1); if (a < b && c > (d)) {}");
        check("try {} catch (e: unknown) {}", "try {} catch (e) {}");
        check("class A { m(a: number): string { return `${a}`; } }", "class A { m(a) { return `${a}`; } }");
        check("const re = /a:b/g; const t = cond ? 1 : 2;", "const re = /a:b/g; const t = cond ? 1 : 2;");
    }

    #[test]
    fn keeps_lines() {
        let s = "interface X {\n a: number;\n}\nconst y: X = null as any;\n";
        let o = strip_types(s);
        assert_eq!(o.lines().count(), s.lines().count());
        assert_eq!(o.len(), s.len());
    }
}
