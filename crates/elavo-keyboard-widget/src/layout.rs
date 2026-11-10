//! Key rows for each page, in the iPadOS landscape arrangement.
//!
//! Every row is `COLUMNS` grid columns wide. A letter key spans 2, so wide
//! keys (delete, return, shift, space) are sized in half-letter steps.

/// Grid columns per row.
pub const COLUMNS: i32 = 23;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Letters,
    Numbers,
    Symbols,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Types `lower`, or `upper` while shift is on.
    Char {
        lower: &'static str,
        upper: &'static str,
    },
    Backspace,
    Return,
    Shift,
    Space,
    Goto(Page),
    Hide,
    /// Empty space that keeps a row's offset.
    Gap,
}

#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub kind: Kind,
    pub span: i32,
    pub label: &'static str,
}

impl Key {
    /// A stable name for tests and tooling: the character, or the key's role.
    pub fn id(&self) -> String {
        match self.kind {
            Kind::Char { lower, .. } => lower.to_owned(),
            Kind::Backspace => "backspace".into(),
            Kind::Return => "return".into(),
            Kind::Shift => "shift".into(),
            Kind::Space => "space".into(),
            Kind::Goto(Page::Letters) => "page-letters".into(),
            Kind::Goto(Page::Numbers) => "page-numbers".into(),
            Kind::Goto(Page::Symbols) => "page-symbols".into(),
            Kind::Hide => "hide".into(),
            Kind::Gap => "gap".into(),
        }
    }

    /// Letter-colored keys; the rest use the darker function color.
    pub fn is_char_style(&self) -> bool {
        matches!(self.kind, Kind::Char { .. } | Kind::Space)
    }
}

const fn ch(lower: &'static str, upper: &'static str) -> Key {
    Key {
        kind: Kind::Char { lower, upper },
        span: 2,
        label: lower,
    }
}

const fn same(c: &'static str) -> Key {
    ch(c, c)
}

const fn key(kind: Kind, span: i32, label: &'static str) -> Key {
    Key { kind, span, label }
}

const BACKSPACE: Key = key(Kind::Backspace, 3, "\u{232B}");
const RETURN: Key = key(Kind::Return, 4, "return");
const SPACE: Key = key(Kind::Space, 14, "");
const HIDE: Key = key(Kind::Hide, 3, "\u{2328}");
const GAP: Key = key(Kind::Gap, 1, "");

fn letters(row: &[(&'static str, &'static str)]) -> Vec<Key> {
    row.iter().map(|&(lower, upper)| ch(lower, upper)).collect()
}

const TOP: &[(&str, &str)] = &[
    ("q", "Q"),
    ("w", "W"),
    ("e", "E"),
    ("r", "R"),
    ("t", "T"),
    ("y", "Y"),
    ("u", "U"),
    ("i", "I"),
    ("o", "O"),
    ("p", "P"),
];
const HOME: &[(&str, &str)] = &[
    ("a", "A"),
    ("s", "S"),
    ("d", "D"),
    ("f", "F"),
    ("g", "G"),
    ("h", "H"),
    ("j", "J"),
    ("k", "K"),
    ("l", "L"),
];
const BOTTOM: &[(&str, &str)] = &[
    ("z", "Z"),
    ("x", "X"),
    ("c", "C"),
    ("v", "V"),
    ("b", "B"),
    ("n", "N"),
    ("m", "M"),
];

fn symbols(row: &'static str) -> Vec<Key> {
    row.split(' ').map(same).collect()
}

fn row(parts: Vec<Vec<Key>>) -> Vec<Key> {
    parts.into_iter().flatten().collect()
}

pub fn rows(page: Page) -> Vec<Vec<Key>> {
    match page {
        Page::Letters => vec![
            row(vec![letters(TOP), vec![BACKSPACE]]),
            row(vec![vec![GAP], letters(HOME), vec![RETURN]]),
            row(vec![
                vec![key(Kind::Shift, 3, "\u{21E7}")],
                letters(BOTTOM),
                vec![ch(",", "!"), ch(".", "?")],
                vec![key(Kind::Shift, 2, "\u{21E7}")],
            ]),
            vec![
                key(Kind::Goto(Page::Numbers), 3, ".?123"),
                SPACE,
                key(Kind::Goto(Page::Numbers), 3, ".?123"),
                HIDE,
            ],
        ],
        Page::Numbers => vec![
            row(vec![symbols("1 2 3 4 5 6 7 8 9 0"), vec![BACKSPACE]]),
            row(vec![vec![GAP], symbols("@ # $ & * ( ) ' \""), vec![RETURN]]),
            row(vec![
                vec![key(Kind::Goto(Page::Symbols), 3, "#+=")],
                symbols("% - + = / ; : , ."),
                vec![key(Kind::Goto(Page::Symbols), 2, "#+=")],
            ]),
            vec![
                key(Kind::Goto(Page::Letters), 3, "ABC"),
                SPACE,
                key(Kind::Goto(Page::Letters), 3, "ABC"),
                HIDE,
            ],
        ],
        Page::Symbols => vec![
            row(vec![symbols("[ ] { } # % ^ * + ="), vec![BACKSPACE]]),
            row(vec![
                vec![GAP],
                symbols("_ \\ | ~ < > \u{20AC} \u{00A3} \u{00A5}"),
                vec![RETURN],
            ]),
            row(vec![
                vec![key(Kind::Goto(Page::Numbers), 3, "123")],
                symbols(". , ? ! ' \" ` \u{00B0} \u{2022}"),
                vec![key(Kind::Goto(Page::Numbers), 2, "123")],
            ]),
            vec![
                key(Kind::Goto(Page::Letters), 3, "ABC"),
                SPACE,
                key(Kind::Goto(Page::Letters), 3, "ABC"),
                HIDE,
            ],
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_fills_the_grid() {
        for page in [Page::Letters, Page::Numbers, Page::Symbols] {
            let rows = rows(page);
            assert_eq!(rows.len(), 4, "{page:?}");
            for (i, r) in rows.iter().enumerate() {
                let total: i32 = r.iter().map(|k| k.span).sum();
                assert_eq!(total, COLUMNS, "{page:?} row {i}");
            }
        }
    }

    #[test]
    fn shift_turns_letters_upper() {
        let first = &rows(Page::Letters)[0][0];
        assert_eq!(
            first.kind,
            Kind::Char {
                lower: "q",
                upper: "Q"
            }
        );
    }
}
