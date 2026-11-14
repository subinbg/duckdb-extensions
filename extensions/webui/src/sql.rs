//! Finds where the statements of a SQL text end.
//!
//! The extension wraps the text of a cell's last statement in a subquery, to sort, filter or count its rows.
//! DuckDB splits the text too, but hands back prepared statements and not their text.

/// The statements of `sql`, each without the whitespace, comments and semicolon around it.
pub fn split_statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut range: Option<(usize, usize)> = None;
    let mut position = 0;

    while let Some(first) = sql[position..].chars().next() {
        let rest = &sql[position..];
        // How long the piece that starts here is, and whether it is part of a statement.
        let (length, content) = match first {
            _ if first.is_whitespace() => (first.len_utf8(), false),
            '-' if rest.starts_with("--") => (rest.find('\n').unwrap_or(rest.len()), false),
            '/' if rest.starts_with("/*") => (block_comment(rest), false),
            '\'' | '"' => (quoted(rest, first), true),
            '$' => (dollar_quoted(rest).unwrap_or(1), true),
            ';' => {
                statements.extend(range.take().map(|(start, end)| &sql[start..end]));
                (1, false)
            }
            _ => (first.len_utf8(), true),
        };

        if content {
            range = Some((range.map_or(position, |(start, _)| start), position + length));
        }
        position += length;
    }

    statements.extend(range.map(|(start, end)| &sql[start..end]));
    statements
}

/// Length of the `/* ... */` comment that `text` starts with; such comments nest.
fn block_comment(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut position = 0;

    while position < bytes.len() {
        if bytes[position..].starts_with(b"/*") {
            depth += 1;
            position += 2;
        } else if bytes[position..].starts_with(b"*/") {
            depth -= 1;
            position += 2;
            if depth == 0 {
                return position;
            }
        } else {
            position += 1;
        }
    }
    text.len()
}

/// Length of the quoted text that `text` starts with; a doubled quote inside it is an escaped quote.
fn quoted(text: &str, quote: char) -> usize {
    let mut characters = text.char_indices().skip(1).peekable();

    while let Some((position, c)) = characters.next() {
        if c != quote {
            continue;
        }
        match characters.peek() {
            Some((_, next)) if *next == quote => {
                characters.next();
            }
            _ => return position + 1,
        }
    }
    text.len()
}

/// Length of the `$tag$ ... $tag$` string that `text` starts with, if it starts with one.
/// `$1` and `$name` are parameters.
fn dollar_quoted(text: &str) -> Option<usize> {
    let tag_length = text[1..].find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(text.len() - 1);
    let tag = &text[1..1 + tag_length];
    if tag.starts_with(|c: char| c.is_ascii_digit()) || !text[1 + tag_length..].starts_with('$') {
        return None;
    }

    let delimiter = &text[..tag_length + 2];
    let body = &text[delimiter.len()..];
    Some(body.find(delimiter).map_or(text.len(), |end| delimiter.len() + end + delimiter.len()))
}

#[cfg(test)]
mod tests {
    use super::split_statements;

    #[test]
    fn splits_outside_quotes_and_comments() {
        assert_eq!(
            split_statements(
                "CREATE TABLE t AS SELECT ';' AS a; -- first; \n /* ; /* ; */ */ SELECT * FROM \"a;b\" ;; "
            ),
            vec!["CREATE TABLE t AS SELECT ';' AS a", "SELECT * FROM \"a;b\""]
        );
        assert_eq!(
            split_statements("SELECT 'it''s; fine', $1; SELECT $tag$ ; $tag$"),
            vec!["SELECT 'it''s; fine', $1", "SELECT $tag$ ; $tag$"]
        );
        assert_eq!(split_statements("SELECT 한글 -- trailing"), vec!["SELECT 한글"]);
        assert_eq!(split_statements("  -- nothing here\n ; "), Vec::<&str>::new());
    }

    #[test]
    fn unterminated_pieces_run_to_the_end() {
        assert_eq!(split_statements("SELECT 'open; SELECT 2"), vec!["SELECT 'open; SELECT 2"]);
        assert_eq!(split_statements("SELECT 1 /* open; SELECT 2"), vec!["SELECT 1"]);
        assert_eq!(split_statements("SELECT $q$ open; SELECT 2"), vec!["SELECT $q$ open; SELECT 2"]);
    }
}
