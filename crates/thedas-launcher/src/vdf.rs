//! Valve's text KeyValues format (`.vdf`, `.acf`), as Steam writes `libraryfolders.vdf` and
//! `appmanifest_<id>.acf`: quoted (or bare) keys, each followed by a quoted value or a braced
//! block of more pairs, with `//` comments.
//!
//! Only what finding a game needs is read. Bad input gives `None` or a partial tree, never a
//! panic; nesting is capped so a hostile file cannot exhaust the stack.

/// A value: a string, or a block of pairs in file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Vdf {
    Str(String),
    Block(Vec<(String, Vdf)>),
}

const MAX_DEPTH: usize = 32;

impl Vdf {
    /// The first value under `key` (keys compare case-insensitively, as Steam's do).
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Block(pairs) => pairs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Block(_) => None,
        }
    }

    pub fn pairs(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Block(p) => p,
            Vdf::Str(_) => &[],
        }
    }
}

/// Parses a whole file: its top-level pairs as one block.
pub fn parse(text: &str) -> Option<Vdf> {
    let mut p = Parser {
        chars: text.chars().collect(),
        pos: 0,
    };
    let block = p.block(0, true)?;
    Some(Vdf::Block(block))
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

enum Token {
    Str(String),
    Open,
    Close,
}

impl Parser {
    fn skip_space(&mut self) {
        loop {
            match self.chars.get(self.pos) {
                Some(c) if c.is_whitespace() || *c == '\u{feff}' => self.pos += 1,
                Some('/') if self.chars.get(self.pos + 1) == Some(&'/') => {
                    while let Some(c) = self.chars.get(self.pos) {
                        if *c == '\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn token(&mut self) -> Option<Token> {
        self.skip_space();
        let c = *self.chars.get(self.pos)?;
        match c {
            '{' => {
                self.pos += 1;
                Some(Token::Open)
            }
            '}' => {
                self.pos += 1;
                Some(Token::Close)
            }
            '"' => {
                self.pos += 1;
                let mut s = String::new();
                loop {
                    let c = *self.chars.get(self.pos)?;
                    self.pos += 1;
                    match c {
                        '"' => return Some(Token::Str(s)),
                        '\\' => {
                            let e = *self.chars.get(self.pos)?;
                            self.pos += 1;
                            s.push(match e {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        }
                        other => s.push(other),
                    }
                }
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = self.chars.get(self.pos) {
                    if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                        break;
                    }
                    s.push(c);
                    self.pos += 1;
                }
                Some(Token::Str(s))
            }
        }
    }

    /// Pairs up to the closing brace (or the end of the file at the top level).
    fn block(&mut self, depth: usize, top: bool) -> Option<Vec<(String, Vdf)>> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut pairs = Vec::new();
        loop {
            let key = match self.token() {
                None if top => return Some(pairs),
                None => return Some(pairs),
                Some(Token::Close) if !top => return Some(pairs),
                Some(Token::Close) | Some(Token::Open) => continue,
                Some(Token::Str(k)) => k,
            };
            match self.token() {
                Some(Token::Str(v)) => pairs.push((key, Vdf::Str(v))),
                Some(Token::Open) => {
                    let inner = self.block(depth + 1, false)?;
                    pairs.push((key, Vdf::Block(inner)));
                }
                Some(Token::Close) if !top => return Some(pairs),
                _ => return Some(pairs),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_folders_new_format() {
        let text = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps"
		{
			"228980"		"0"
		}
	}
	// a comment
	"1"
	{
		"path"		"D:\\SteamLibrary"
	}
}
"#;
        let v = parse(text).unwrap();
        let lib = v.get("LibraryFolders").unwrap();
        assert_eq!(
            lib.get("0").unwrap().get("path").unwrap().str(),
            Some(r"C:\Program Files (x86)\Steam")
        );
        assert_eq!(
            lib.get("1").unwrap().get("path").unwrap().str(),
            Some(r"D:\SteamLibrary")
        );
    }

    #[test]
    fn bad_input_does_not_panic() {
        for text in [
            "",
            "\"",
            "{{{{",
            "}}}}",
            "\"a\" {",
            "\"a\\",
            &"{\"a\" ".repeat(1000),
        ] {
            let _ = parse(text);
        }
    }

    #[test]
    fn nesting_is_capped() {
        let text = "\"a\" {".repeat(100);
        assert!(parse(&text).is_none());
    }
}
