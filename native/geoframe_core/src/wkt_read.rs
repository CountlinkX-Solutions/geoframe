//! WKT read as GEOS reads it, on top of the `wkt` crate.
//!
//! The `wkt` crate takes an untagged geometry as XY. GEOS (3.12 and later; Shapely 2.2 runs
//! 3.14) takes its dimension from its first coordinate instead: three ordinates make it Z, four
//! ZM, and every other coordinate of that geometry must have as many. Text the crate rejects is
//! read again with that dimension written out (` Z` or ` ZM` after the geometry keyword), so
//! text the crate already reads is never reinterpreted, and a coordinate count that disagrees
//! with the first one stays an error, as in GEOS.
//!
//! GEOS also reads an ordinate with `strtod`, so `NaN`, `inf` and `Infinity` (any case) are
//! numbers. The crate's tokenizer only reads them with a sign (`-inf`), since a bare word is
//! not a number to it; the retry writes `+` before a bare one, which Rust reads as GEOS does.

use std::str::FromStr;

/// Parses one WKT geometry as GEOS does: untagged dimensions inferred, non-finite ordinates read.
pub(crate) fn parse(text: &str) -> Result<wkt::Wkt<f64>, &'static str> {
    wkt::Wkt::from_str(text).or_else(|error| match as_geos_reads_it(text) {
        Some(tagged) => wkt::Wkt::from_str(&tagged),
        None => Err(error),
    })
}

/// A token of the `wkt` crate's tokenizer: an atom is a word or a number, `start` and `end` its
/// byte offsets.
#[derive(Clone, Copy)]
enum Token<'a> {
    Open,
    Close,
    Comma,
    Atom {
        text: &'a str,
        start: usize,
        end: usize,
    },
}

/// Splits `text` as the `wkt` crate does: parentheses and commas are tokens, whitespace
/// separates atoms, and a NUL ends the input.
fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        let separator = match c {
            '(' => Some(Token::Open),
            ')' => Some(Token::Close),
            ',' => Some(Token::Comma),
            '\0' | ' ' | '\n' | '\r' | '\t' => None,
            _ => {
                start.get_or_insert(i);
                continue;
            }
        };
        if let Some(s) = start.take() {
            out.push(Token::Atom {
                text: &text[s..i],
                start: s,
                end: i,
            });
        }
        if c == '\0' {
            return out;
        }
        out.extend(separator);
    }
    if let Some(s) = start {
        out.push(Token::Atom {
            text: &text[s..],
            start: s,
            end: text.len(),
        });
    }
    out
}

/// The geometry keywords whose dimension GEOS infers. A collection is left alone: its
/// dimension is the union of its members' (see `mixed_dims`), each inferred on its own.
fn infers_dimension(word: &str) -> bool {
    [
        "POINT",
        "LINESTRING",
        "POLYGON",
        "MULTIPOINT",
        "MULTILINESTRING",
        "MULTIPOLYGON",
    ]
    .iter()
    .any(|k| word.eq_ignore_ascii_case(k))
}

fn is_dimension_tag(word: &str) -> bool {
    ["Z", "M", "ZM"]
        .iter()
        .any(|t| word.eq_ignore_ascii_case(t))
}

/// A non-finite number GEOS reads (`strtod`) that the crate's tokenizer takes for a word.
fn is_bare_non_finite(word: &str) -> bool {
    ["NaN", "inf", "infinity"]
        .iter()
        .any(|n| word.eq_ignore_ascii_case(n))
}

/// `text` as GEOS reads it: ` Z` or ` ZM` after every untagged geometry keyword whose first
/// coordinate has three or four ordinates, `+` before every bare `NaN`, `inf` or `Infinity`,
/// and a NaN point for every `EMPTY` member of a parenthesized `MULTIPOINT` (which the crate
/// cannot read, and which GeoArrow stores as a NaN point); `None` when nothing needs rewriting.
fn as_geos_reads_it(text: &str) -> Option<String> {
    let tokens = tokens(text);
    // `(start, end, replacement)`; an insertion has `start == end`.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let Token::Atom {
            text: word,
            start,
            end,
        } = *token
        else {
            continue;
        };
        if is_bare_non_finite(word) {
            edits.push((start, start, "+".to_string()));
            continue;
        }
        if !infers_dimension(word) {
            continue;
        }
        let tagged = match tokens.get(i + 1) {
            Some(Token::Atom { text: next, .. }) if is_dimension_tag(next) => Some(*next),
            _ => None,
        };
        let inferred = match tagged {
            Some(_) => None,
            None => first_coordinate_tag(&tokens[i + 1..]),
        };
        if let Some(tag) = inferred {
            edits.push((end, end, tag.to_string()));
        }
        if word.eq_ignore_ascii_case("MULTIPOINT") {
            let ordinates = match (tagged, inferred) {
                (Some(t), _) if t.eq_ignore_ascii_case("ZM") => 4,
                (Some(_), _) => 3,
                (None, Some(" ZM")) => 4,
                (None, Some(_)) => 3,
                (None, None) => 2,
            };
            let body = i + 1 + usize::from(tagged.is_some());
            edits.extend(empty_member_points(&tokens[body..], ordinates));
        }
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|e| e.0);
    let mut out = String::with_capacity(text.len() + 16 * edits.len());
    let mut from = 0;
    for (start, end, replacement) in edits {
        out.push_str(&text[from..start]);
        out.push_str(&replacement);
        from = end;
    }
    out.push_str(&text[from..]);
    Some(out)
}

/// A NaN point in place of each `EMPTY` member of the `MULTIPOINT` body starting at `rest`,
/// when every member is parenthesized or `EMPTY` (GEOS rejects a mix with the bare form).
fn empty_member_points(rest: &[Token<'_>], ordinates: usize) -> Vec<(usize, usize, String)> {
    let point = format!("({})", vec!["+NaN"; ordinates].join(" "));
    let mut out = Vec::new();
    let mut depth = 0usize;
    for token in rest {
        match *token {
            Token::Open => depth += 1,
            Token::Close if depth <= 1 => break,
            Token::Close => depth -= 1,
            Token::Comma => {}
            Token::Atom { text, start, end } if depth == 1 => {
                if !text.eq_ignore_ascii_case("EMPTY") {
                    return Vec::new();
                }
                out.push((start, end, point.clone()));
            }
            // `MULTIPOINT EMPTY`, or an atom inside a member.
            Token::Atom { .. } if depth == 0 => return Vec::new(),
            Token::Atom { .. } => {}
        }
    }
    out
}

/// The tag the first coordinate of the geometry starting at `rest` (the tokens after its
/// keyword) implies: ` Z` for three ordinates, ` ZM` for four, `None` for any other count or
/// when the geometry has no coordinate (`EMPTY`, or parts that are all `EMPTY`).
fn first_coordinate_tag(rest: &[Token<'_>]) -> Option<&'static str> {
    let mut depth = 0usize;
    let mut iter = rest.iter().peekable();
    while let Some(token) = iter.next() {
        match *token {
            Token::Open => depth += 1,
            // The geometry's own closing parenthesis: no coordinate in it.
            Token::Close if depth <= 1 => return None,
            Token::Close => depth -= 1,
            Token::Comma => {}
            Token::Atom { text, .. } if text.eq_ignore_ascii_case("EMPTY") => {
                if depth == 0 {
                    return None;
                }
            }
            // An atom before the opening parenthesis: not WKT this function can tag.
            Token::Atom { .. } if depth == 0 => return None,
            Token::Atom { .. } => {
                let mut ordinates = 1;
                while matches!(iter.peek(), Some(Token::Atom { .. })) {
                    iter.next();
                    ordinates += 1;
                }
                return match ordinates {
                    3 => Some(" Z"),
                    4 => Some(" ZM"),
                    _ => None,
                };
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::as_geos_reads_it;

    #[test]
    fn tags_are_inserted_after_each_untagged_keyword() {
        assert_eq!(as_geos_reads_it("POINT (1 2)"), None);
        assert_eq!(as_geos_reads_it("POINT EMPTY"), None);
        assert_eq!(
            as_geos_reads_it("POINT(1 2 3)").as_deref(),
            Some("POINT Z(1 2 3)")
        );
        assert_eq!(
            as_geos_reads_it("GEOMETRYCOLLECTION (POINT EMPTY, point (1 2 3 4))").as_deref(),
            Some("GEOMETRYCOLLECTION (POINT EMPTY, point ZM (1 2 3 4))")
        );
        assert_eq!(
            as_geos_reads_it("MULTIPOINT (EMPTY, (1 2 3)), POINT Z (1 2 3)").as_deref(),
            Some("MULTIPOINT Z ((+NaN +NaN +NaN), (1 2 3)), POINT Z (1 2 3)")
        );
        assert_eq!(
            as_geos_reads_it("POINT (1 2 NaN)").as_deref(),
            Some("POINT Z (1 2 +NaN)")
        );
        assert_eq!(
            as_geos_reads_it("POINT Z (inf -inf Infinity)").as_deref(),
            Some("POINT Z (+inf -inf +Infinity)")
        );
    }
}
