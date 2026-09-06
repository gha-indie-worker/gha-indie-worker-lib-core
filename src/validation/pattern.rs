#![forbid(unsafe_code)]

//! A tiny backtracking matcher for the `pattern` subset the contract authorities
//! may use. No regex crate, no allocation during matching beyond the compiled
//! program.
//!
//! Supported: `^` and `$` anchors, literal characters, `.`, character classes
//! (`[a-z0-9_-]`, `[^@ ]`, a literal `-` at either edge), the escapes `\d` `\D`
//! `\w` `\W` `\s` `\S` and `\<punct>`, and the postfix quantifiers `*`, `+`, `?`.
//!
//! Deliberately absent: groups, alternation, `{n,m}`, backreferences, lookaround.
//! The toolkit's TypeSpec parser cannot carry `{` or `}` inside a model body, so
//! an authority that used `{n,m}` would already fail the parity gate; anything
//! else outside the list is rejected at parse time with a reason, and the caller
//! turns that into an `unsupported-pattern` violation rather than guessing.
//!
//! Matching is unanchored unless the pattern says otherwise, exactly like JSON
//! Schema `pattern` (which is a *search*, not a full match).

/// A compiled pattern.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    anchored_start: bool,
    anchored_end: bool,
    atoms: Vec<Atom>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Atom {
    item: Item,
    min: usize,
    max: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Item {
    Any,
    Literal(char),
    Class {
        negated: bool,
        parts: Vec<ClassPart>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ClassPart {
    Char(char),
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

impl ClassPart {
    fn matches(&self, c: char) -> bool {
        match self {
            ClassPart::Char(x) => *x == c,
            ClassPart::Range(lo, hi) => *lo <= c && c <= *hi,
            ClassPart::Digit(positive) => c.is_ascii_digit() == *positive,
            ClassPart::Word(positive) => (c.is_alphanumeric() || c == '_') == *positive,
            ClassPart::Space(positive) => c.is_whitespace() == *positive,
        }
    }
}

impl Item {
    fn matches(&self, c: char) -> bool {
        match self {
            Item::Any => c != '\n',
            Item::Literal(x) => *x == c,
            Item::Class { negated, parts } => parts.iter().any(|p| p.matches(c)) != *negated,
        }
    }
}

fn escape_part(c: char) -> Result<ClassPart, String> {
    Ok(match c {
        'd' => ClassPart::Digit(true),
        'D' => ClassPart::Digit(false),
        'w' => ClassPart::Word(true),
        'W' => ClassPart::Word(false),
        's' => ClassPart::Space(true),
        'S' => ClassPart::Space(false),
        'n' => ClassPart::Char('\n'),
        'r' => ClassPart::Char('\r'),
        't' => ClassPart::Char('\t'),
        c if c.is_ascii_punctuation() => ClassPart::Char(c),
        other => return Err(format!("unsupported escape \\{other}")),
    })
}

impl Pattern {
    /// Compile a pattern.
    ///
    /// # Errors
    ///
    /// Returns a human-readable reason when the pattern uses anything outside the
    /// supported subset, so the caller can report `unsupported-pattern` instead
    /// of silently accepting the instance.
    pub fn parse(source: &str) -> Result<Self, String> {
        let chars: Vec<char> = source.chars().collect();
        let mut i = 0;
        let anchored_start = chars.first() == Some(&'^');
        if anchored_start {
            i = 1;
        }
        let mut anchored_end = false;
        let mut atoms: Vec<Atom> = Vec::new();

        while i < chars.len() {
            let c = chars[i];
            if c == '$' && i + 1 == chars.len() {
                anchored_end = true;
                break;
            }
            let item = match c {
                '(' | ')' | '|' | '{' | '}' => {
                    return Err(format!("`{c}` is outside the supported pattern subset"));
                }
                '^' | '$' => return Err(format!("`{c}` is only supported as an anchor")),
                '.' => {
                    i += 1;
                    Item::Any
                }
                '\\' => {
                    let Some(next) = chars.get(i + 1).copied() else {
                        return Err("trailing backslash".to_owned());
                    };
                    i += 2;
                    match escape_part(next)? {
                        ClassPart::Char(ch) => Item::Literal(ch),
                        part => Item::Class {
                            negated: false,
                            parts: vec![part],
                        },
                    }
                }
                '[' => {
                    let (item, next) = parse_class(&chars, i)?;
                    i = next;
                    item
                }
                other => {
                    i += 1;
                    Item::Literal(other)
                }
            };
            let (min, max) = match chars.get(i) {
                Some('*') => {
                    i += 1;
                    (0, usize::MAX)
                }
                Some('+') => {
                    i += 1;
                    (1, usize::MAX)
                }
                Some('?') => {
                    i += 1;
                    (0, 1)
                }
                _ => (1, 1),
            };
            atoms.push(Atom { item, min, max });
        }

        Ok(Pattern {
            anchored_start,
            anchored_end,
            atoms,
        })
    }

    /// Whether `text` matches. Unanchored patterns are searched, as JSON Schema
    /// specifies.
    #[must_use]
    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        if self.anchored_start {
            return self.match_from(&chars, 0, 0);
        }
        (0..=chars.len()).any(|start| self.match_from(&chars, 0, start))
    }

    fn match_from(&self, text: &[char], atom: usize, pos: usize) -> bool {
        let Some(current) = self.atoms.get(atom) else {
            return !self.anchored_end || pos == text.len();
        };
        let mut count = 0;
        let mut cursor = pos;
        while count < current.min {
            if cursor < text.len() && current.item.matches(text[cursor]) {
                cursor += 1;
                count += 1;
            } else {
                return false;
            }
        }
        // greedy, then give characters back one at a time
        let mut ends = vec![cursor];
        while count < current.max && cursor < text.len() && current.item.matches(text[cursor]) {
            cursor += 1;
            count += 1;
            ends.push(cursor);
        }
        ends.iter()
            .rev()
            .any(|end| self.match_from(text, atom + 1, *end))
    }
}

fn parse_class(chars: &[char], open: usize) -> Result<(Item, usize), String> {
    let mut i = open + 1;
    let negated = chars.get(i) == Some(&'^');
    if negated {
        i += 1;
    }
    let mut parts = Vec::new();
    let mut first = true;
    while i < chars.len() {
        let c = chars[i];
        if c == ']' && !first {
            return Ok((Item::Class { negated, parts }, i + 1));
        }
        first = false;
        if c == '\\' {
            let Some(next) = chars.get(i + 1).copied() else {
                return Err("trailing backslash in character class".to_owned());
            };
            parts.push(escape_part(next)?);
            i += 2;
            continue;
        }
        // `-` is a range only when it sits between two literals
        let is_range = chars.get(i + 1) == Some(&'-')
            && chars.get(i + 2).is_some_and(|n| *n != ']')
            && chars.get(i + 2) != Some(&'\\');
        if is_range {
            let hi = chars[i + 2];
            if hi < c {
                return Err(format!("inverted range {c}-{hi}"));
            }
            parts.push(ClassPart::Range(c, hi));
            i += 3;
        } else {
            parts.push(ClassPart::Char(c));
            i += 1;
        }
    }
    Err("unterminated character class".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, text: &str) -> bool {
        Pattern::parse(pattern).expect(pattern).is_match(text)
    }

    #[test]
    fn slug_pattern_from_the_identity_authority() {
        let p = "^[a-z0-9][a-z0-9-]*[a-z0-9]$";
        assert!(matches(p, "indie-labs"));
        assert!(matches(p, "ab"));
        assert!(!matches(p, "a"));
        assert!(!matches(p, "-indie"));
        assert!(!matches(p, "indie-"));
        assert!(!matches(p, "Indie"));
        assert!(!matches(p, "indie labs"));
    }

    #[test]
    fn repository_pattern_from_the_runs_authority() {
        let p = "^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$";
        assert!(matches(p, "gha-indie-worker/gha-clone-server.rs"));
        assert!(!matches(p, "gha-clone-server.rs"));
        assert!(!matches(p, "owner/repo/extra"));
        assert!(!matches(p, "/repo"));
    }

    #[test]
    fn hex_and_digest_patterns() {
        assert!(matches("^[0-9a-f]+$", &"9f".repeat(20)));
        assert!(!matches("^[0-9a-f]+$", "main"));
        assert!(matches(
            "^sha256-[0-9a-f]+$",
            &format!("sha256-{}", "ab".repeat(32))
        ));
        assert!(!matches("^sha256-[0-9a-f]+$", "latest"));
    }

    #[test]
    fn negated_classes_and_the_email_pattern() {
        let p = "^[^@ ]+@[^@ ]+[.][^@ ]+$";
        assert!(matches(p, "dana@indie-labs.example"));
        assert!(!matches(p, "dana@example"));
        assert!(!matches(p, "dana example.com"));
        assert!(!matches(p, "a@b@c.d"));
    }

    #[test]
    fn quantifiers_backtrack() {
        assert!(matches("^a*ab$", "aaab"));
        assert!(matches("^.*z$", "abcz"));
        assert!(!matches("^a+b$", "b"));
        assert!(matches("^a?b$", "b"));
        assert!(matches("^a?b$", "ab"));
    }

    #[test]
    fn unanchored_patterns_search() {
        assert!(matches("abc", "xxabcxx"));
        assert!(!matches("^abc", "xxabc"));
        assert!(matches("abc$", "xxabc"));
    }

    #[test]
    fn escapes_are_supported() {
        assert!(matches(r"^\d+$", "12345"));
        assert!(!matches(r"^\d+$", "12a45"));
        assert!(matches(r"^\w+$", "a_1"));
        assert!(matches(r"^a\.b$", "a.b"));
        assert!(!matches(r"^a\.b$", "axb"));
    }

    #[test]
    fn literal_dash_at_class_edges() {
        assert!(matches("^[-a-z]+$", "a-b"));
        assert!(matches("^[a-z-]+$", "a-b"));
    }

    #[test]
    fn unsupported_constructs_are_rejected_with_a_reason() {
        for bad in ["^(a|b)$", "^a{2,3}$", "^a|b$", "^[a-z"] {
            let err = Pattern::parse(bad).unwrap_err();
            assert!(!err.is_empty(), "{bad}");
        }
    }

    #[test]
    fn inverted_range_is_rejected() {
        assert!(Pattern::parse("^[z-a]$").is_err());
    }
}
