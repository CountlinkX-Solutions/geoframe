//! WKT read as GEOS reads it, on top of the `wkt` crate.
//!
//! The `wkt` crate takes an untagged geometry as XY. GEOS (3.12 and later; Shapely 2.2 runs
//! 3.14) takes its dimension from its first coordinate instead: three ordinates make it Z, four
//! ZM, and every other coordinate of that geometry must have as many. Text the crate rejects is
//! read again with that dimension written out (` Z` or ` ZM` after the geometry keyword), so
//! text the crate already reads is never reinterpreted, and a coordinate count that disagrees
//! with the first one stays an error, as in GEOS.

use std::str::FromStr;

/// Parses one WKT geometry, inferring the dimension of untagged geometries as GEOS does.
pub(crate) fn parse(text: &str) -> Result<wkt::Wkt<f64>, &'static str> {
    wkt::Wkt::from_str(text).or_else(|error| match with_inferred_dimensions(text) {
        Some(tagged) => wkt::Wkt::from_str(&tagged),
        None => Err(error),
    })
}

/// A token of the `wkt` crate's tokenizer: an atom is a word or a number, `end` its byte offset.
#[derive(Clone, Copy)]
enum Token<'a> {
    Open,
    Close,
    Comma,
    Atom { text: &'a str, end: usize },
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

/// `text` with ` Z` or ` ZM` after every untagged geometry keyword whose first coordinate has
/// three or four ordinates; `None` when no keyword needs one.
fn with_inferred_dimensions(text: &str) -> Option<String> {
    let tokens = tokens(text);
    let mut insertions: Vec<(usize, &str)> = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        let Token::Atom { text: word, end } = *token else {
            continue;
        };
        if !infers_dimension(word) {
            continue;
        }
        if matches!(tokens.get(i + 1), Some(Token::Atom { text: next, .. }) if is_dimension_tag(next))
        {
            continue;
        }
        if let Some(tag) = first_coordinate_tag(&tokens[i + 1..]) {
            insertions.push((end, tag));
        }
    }
    if insertions.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len() + 3 * insertions.len());
    let mut from = 0;
    for (at, tag) in insertions {
        out.push_str(&text[from..at]);
        out.push_str(tag);
        from = at;
    }
    out.push_str(&text[from..]);
    Some(out)
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
    use super::with_inferred_dimensions;

    #[test]
    fn tags_are_inserted_after_each_untagged_keyword() {
        assert_eq!(with_inferred_dimensions("POINT (1 2)"), None);
        assert_eq!(with_inferred_dimensions("POINT EMPTY"), None);
        assert_eq!(
            with_inferred_dimensions("POINT(1 2 3)").as_deref(),
            Some("POINT Z(1 2 3)")
        );
        assert_eq!(
            with_inferred_dimensions("GEOMETRYCOLLECTION (POINT EMPTY, point (1 2 3 4))")
                .as_deref(),
            Some("GEOMETRYCOLLECTION (POINT EMPTY, point ZM (1 2 3 4))")
        );
        assert_eq!(
            with_inferred_dimensions("MULTIPOINT (EMPTY, (1 2 3)), POINT Z (1 2 3)").as_deref(),
            Some("MULTIPOINT Z (EMPTY, (1 2 3)), POINT Z (1 2 3)")
        );
    }
}
