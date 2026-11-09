//! Import path aliases from `tsconfig.json` / `jsconfig.json` (`baseUrl`, `paths`, `extends`).
//!
//! `import { Button } from "@/components/Button"` is only resolvable when we know that
//! `"@/*"` maps to `src/*`. Everything here is best effort: a config we cannot read just
//! means fewer aliases, never an error.

use serde_json::Value;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
struct Rule {
    /// text before the `*` (the whole pattern when there is no `*`)
    prefix: String,
    /// text after the `*`; `None` = exact pattern without a wildcard
    suffix: Option<String>,
    /// replacement templates, relative to the project root (forward slashes)
    targets: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AliasMap {
    rules: Vec<Rule>,
    /// `baseUrl` relative to the project root: bare specifiers like `components/Button` resolve from here
    base_url: Option<String>,
    /// no config declared anything: guess the very common `@/` and `~/` => `src/`
    guess: bool,
}

impl AliasMap {
    /// Reads the config at the root of `root` (nothing is read above it).
    pub fn load(root: &Path) -> AliasMap {
        let mut map = AliasMap::default();
        for name in ["tsconfig.json", "jsconfig.json"] {
            let cfg = root.join(name);
            if cfg.is_file() {
                map.read_config(root, &cfg, 0);
                if !map.rules.is_empty() || map.base_url.is_some() {
                    break;
                }
            }
        }
        map.guess = map.rules.is_empty() && root.join("src").is_dir();
        map
    }

    fn read_config(&mut self, root: &Path, cfg: &Path, depth: usize) {
        if depth > 5 {
            return;
        }
        let Some(json) = std::fs::read_to_string(cfg).ok().and_then(|t| parse_jsonc(&t)) else { return };
        let dir = cfg.parent().unwrap_or(root);

        // `extends` is the base: our own options win over it
        if let Some(ext) = json.get("extends").and_then(Value::as_str) {
            if ext.starts_with('.') {
                let mut p = dir.join(ext);
                if p.extension().is_none() {
                    p.set_extension("json");
                }
                self.read_config(root, &p, depth + 1);
            }
        }

        let Some(opts) = json.get("compilerOptions") else { return };
        let base_dir = match opts.get("baseUrl").and_then(Value::as_str) {
            Some(b) => {
                let d = normalize(&dir.join(b));
                self.base_url = rel_to(root, &d);
                d
            }
            None => dir.to_path_buf(),
        };
        let Some(paths) = opts.get("paths").and_then(Value::as_object) else { return };
        // `paths` replaces the inherited one as a whole, like in TypeScript
        self.rules.clear();
        for (pattern, targets) in paths {
            let targets: Vec<String> = targets
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|t| rel_to(root, &normalize(&base_dir.join(t))))
                .collect();
            if targets.is_empty() {
                continue;
            }
            let (prefix, suffix) = match pattern.split_once('*') {
                Some((p, s)) => (p.to_string(), Some(s.to_string())),
                None => (pattern.clone(), None),
            };
            self.rules.push(Rule { prefix, suffix, targets });
        }
        // most specific (longest prefix) first, like TypeScript
        self.rules.sort_by_key(|r| std::cmp::Reverse(r.prefix.len()));
    }

    /// Possible project-relative paths (without extension handling) for a non-relative specifier.
    pub fn candidates(&self, spec: &str) -> Vec<String> {
        let mut out = Vec::new();
        for r in &self.rules {
            let hit = match &r.suffix {
                None => (spec == r.prefix).then_some(""),
                Some(sfx) => spec
                    .strip_prefix(r.prefix.as_str())
                    .and_then(|rest| rest.strip_suffix(sfx.as_str())),
            };
            if let Some(star) = hit {
                out.extend(r.targets.iter().map(|t| t.replacen('*', star, 1)));
            }
        }
        if let Some(b) = &self.base_url {
            out.push(if b.is_empty() { spec.to_string() } else { format!("{b}/{spec}") });
        }
        if self.guess {
            if let Some(rest) = spec.strip_prefix("@/").or_else(|| spec.strip_prefix("~/")) {
                out.push(format!("src/{rest}"));
            }
        }
        out
    }
}

/// Lexically resolves `.` and `..` (the files may not exist).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// `dir` below `root` as a forward-slash path (`""` for the root itself); `None` when outside it.
fn rel_to(root: &Path, dir: &Path) -> Option<String> {
    let root = normalize(root);
    let rel = dir.strip_prefix(&root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// tsconfig is JSON with comments and trailing commas.
fn parse_jsonc(text: &str) -> Option<Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                while let Some(&n) = chars.peek() {
                    if n == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    serde_json::from_str(&strip_trailing_commas(&out)).ok()
}

fn strip_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                i += 1;
                out.push(chars[i]);
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|n| !n.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                out.push(c);
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let v = parse_jsonc(
            "{\n // line\n \"a\": [1, 2,], /* block */ \"url\": \"http://x//y\",\n \"b\": {\"c\": 1,},\n}",
        )
        .unwrap();
        assert_eq!(v["a"].as_array().unwrap().len(), 2);
        assert_eq!(v["url"], "http://x//y");
        assert_eq!(v["b"]["c"], 1);
    }

    #[test]
    fn wildcard_exact_and_base_url() {
        let map = AliasMap {
            rules: vec![
                Rule { prefix: "@ui/".into(), suffix: Some("".into()), targets: vec!["src/ui/*".into()] },
                Rule { prefix: "config".into(), suffix: None, targets: vec!["src/config/index".into()] },
            ],
            base_url: Some("src".into()),
            guess: false,
        };
        assert_eq!(map.candidates("@ui/Button"), ["src/ui/Button", "src/@ui/Button"]);
        assert_eq!(map.candidates("config"), ["src/config/index", "src/config"]);
        assert_eq!(map.candidates("react"), ["src/react"]);
    }
}
