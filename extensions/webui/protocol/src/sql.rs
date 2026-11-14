//! A SQL lexer, just precise enough to colour an editor and to find where statements end.
//!
//! The extension uses [`split_statements`] to get the text of a cell's last statement,
//! which it wraps in a subquery to sort, filter or count that statement's rows.
//! The application uses [`tokenize`] to highlight the editor.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Whitespace,
    Comment,
    Keyword,
    Identifier,
    QuotedIdentifier,
    String,
    Number,
    Operator,
    Semicolon,
}

/// A token and the byte range it covers; the ranges of all tokens cover the text exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

pub fn tokenize(sql: &str) -> Vec<Token> {
    let bytes = sql.as_bytes();
    let mut tokens = Vec::new();
    let mut position = 0;

    while position < bytes.len() {
        let rest = &sql[position..];
        let first = rest.chars().next().unwrap();

        let (kind, length) = if first.is_whitespace() {
            (TokenKind::Whitespace, span_while(rest, char::is_whitespace))
        } else if rest.starts_with("--") {
            (TokenKind::Comment, rest.find('\n').unwrap_or(rest.len()))
        } else if rest.starts_with("/*") {
            (TokenKind::Comment, block_comment(rest))
        } else if first == '\'' {
            (TokenKind::String, quoted(rest, '\''))
        } else if first == '"' {
            (TokenKind::QuotedIdentifier, quoted(rest, '"'))
        } else if first == '$' {
            match dollar_quoted(rest) {
                Some(length) => (TokenKind::String, length),
                // `$1` and `$name` are parameters.
                None => (TokenKind::Operator, 1 + span_while(&rest[1..], is_word)),
            }
        } else if first.is_ascii_digit() {
            (TokenKind::Number, span_while(rest, |c| is_word(c) || c == '.'))
        } else if first.is_alphabetic() || first == '_' {
            let length = span_while(rest, is_word);
            match is_keyword(&rest[..length]) {
                true => (TokenKind::Keyword, length),
                false => (TokenKind::Identifier, length),
            }
        } else if first == ';' {
            (TokenKind::Semicolon, 1)
        } else {
            (TokenKind::Operator, first.len_utf8())
        };

        tokens.push(Token { kind, start: position, end: position + length });
        position += length;
    }
    tokens
}

/// The statements of `sql`, each without the whitespace, comments and semicolon around it.
pub fn split_statements(sql: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut range: Option<(usize, usize)> = None;

    for token in tokenize(sql) {
        match token.kind {
            TokenKind::Semicolon => statements.extend(range.take().map(|(start, end)| &sql[start..end])),
            TokenKind::Whitespace | TokenKind::Comment => {}
            _ => range = Some((range.map_or(token.start, |(start, _)| start), token.end)),
        }
    }

    statements.extend(range.map(|(start, end)| &sql[start..end]));
    statements
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Bytes at the start of `text` whose characters all satisfy `keep`.
fn span_while(text: &str, keep: impl Fn(char) -> bool) -> usize {
    text.find(|c| !keep(c)).unwrap_or(text.len())
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
fn dollar_quoted(text: &str) -> Option<usize> {
    let tag_length = span_while(&text[1..], is_word);
    let tag = &text[1..1 + tag_length];

    if tag.starts_with(|c: char| c.is_ascii_digit()) || !text[1 + tag_length..].starts_with('$') {
        return None;
    }

    let delimiter = &text[..tag_length + 2];
    let body = &text[delimiter.len()..];
    Some(body.find(delimiter).map_or(text.len(), |end| delimiter.len() + end + delimiter.len()))
}

pub fn is_keyword(word: &str) -> bool {
    word.len() <= 12 && KEYWORDS.binary_search(&word.to_ascii_uppercase().as_str()).is_ok()
}

/// Words the editor colours and offers for completion, sorted.
#[rustfmt::skip]
pub const KEYWORDS: &[&str] = &[
    "ALL", "ALTER", "ANALYZE", "AND", "ANTI", "ANY", "AS", "ASC", "ASOF", "ATTACH", "BEGIN", "BETWEEN", "BY", "CALL",
    "CASE", "CAST", "CHECK", "COLUMN", "COLUMNS", "COMMIT", "COPY", "CREATE", "CROSS", "CUBE", "CURRENT", "DATABASE",
    "DEFAULT", "DELETE", "DESC", "DESCRIBE", "DETACH", "DISTINCT", "DROP", "ELSE", "END", "EXCEPT", "EXCLUDE",
    "EXISTS", "EXPLAIN", "EXPORT", "FALSE", "FILTER", "FIRST", "FOLLOWING", "FOR", "FOREIGN", "FROM", "FULL",
    "FUNCTION", "GROUP", "GROUPING", "HAVING", "IF", "ILIKE", "IMPORT", "IN", "INDEX", "INNER", "INSERT", "INSTALL",
    "INTERSECT", "INTERVAL", "INTO", "IS", "JOIN", "KEY", "LAST", "LATERAL", "LEFT", "LIKE", "LIMIT", "LOAD", "MACRO",
    "MERGE", "NATURAL", "NOT", "NULL", "NULLS", "OFFSET", "ON", "OR", "ORDER", "OUTER", "OVER", "PARTITION", "PIVOT",
    "POSITIONAL", "PRAGMA", "PRECEDING", "PRIMARY", "QUALIFY", "RANGE", "RECURSIVE", "REFERENCES", "REPLACE",
    "RETURNING", "RIGHT", "ROLLBACK", "ROLLUP", "ROW", "ROWS", "SAMPLE", "SCHEMA", "SELECT", "SEMI", "SEQUENCE",
    "SET", "SETS", "SHOW", "SIMILAR", "SUMMARIZE", "TABLE", "TEMP", "TEMPORARY", "THEN", "TO", "TRANSACTION", "TRUE",
    "TRUNCATE", "TYPE", "UNBOUNDED", "UNION", "UNIQUE", "UNPIVOT", "UPDATE", "USE", "USING", "VALUES", "VIEW", "WHEN",
    "WHERE", "WINDOW", "WITH", "WITHIN",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str) -> Vec<(TokenKind, &str)> {
        tokenize(sql)
            .into_iter()
            .filter(|token| token.kind != TokenKind::Whitespace)
            .map(|token| (token.kind, &sql[token.start..token.end]))
            .collect()
    }

    #[test]
    fn keywords_are_sorted_for_binary_search() {
        assert!(KEYWORDS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(is_keyword("select") && is_keyword("Qualify") && !is_keyword("selects"));
    }

    #[test]
    fn tokens_cover_the_text_exactly() {
        let sql =
            "SELECT 'a;b' AS \"x\"\"y\", 1.5e3, $1 -- note\nFROM t /* c /* nested */ */ WHERE 한글 = $tag$ ; $tag$;";
        let tokens = tokenize(sql);

        assert_eq!(tokens.first().unwrap().start, 0);
        assert_eq!(tokens.last().unwrap().end, sql.len());
        assert!(tokens.windows(2).all(|pair| pair[0].end == pair[1].start));
    }

    #[test]
    fn classifies_tokens() {
        use TokenKind::*;

        assert_eq!(
            kinds("select a_1, 'it''s', \"T\", 42 from t; -- done"),
            vec![
                (Keyword, "select"),
                (Identifier, "a_1"),
                (Operator, ","),
                (String, "'it''s'"),
                (Operator, ","),
                (QuotedIdentifier, "\"T\""),
                (Operator, ","),
                (Number, "42"),
                (Keyword, "from"),
                (Identifier, "t"),
                (Semicolon, ";"),
                (Comment, "-- done"),
            ]
        );
        assert_eq!(kinds("$$a;b$$ $1 $x$ ' $x$"), vec![(String, "$$a;b$$"), (Operator, "$1"), (String, "$x$ ' $x$")]);
    }

    #[test]
    fn unterminated_tokens_run_to_the_end() {
        assert_eq!(kinds("'open"), vec![(TokenKind::String, "'open")]);
        assert_eq!(kinds("/* open"), vec![(TokenKind::Comment, "/* open")]);
        assert_eq!(kinds("$q$ open"), vec![(TokenKind::String, "$q$ open")]);
    }

    #[test]
    fn splits_statements_outside_quotes_and_comments() {
        assert_eq!(
            split_statements("CREATE TABLE t AS SELECT ';' AS a; -- first; \n /* ; */ SELECT * FROM t ;; "),
            vec!["CREATE TABLE t AS SELECT ';' AS a", "SELECT * FROM t"]
        );
        assert_eq!(split_statements("  -- nothing here\n ; "), Vec::<&str>::new());
        assert_eq!(split_statements("SELECT $$ ; $$"), vec!["SELECT $$ ; $$"]);
        assert_eq!(split_statements("SELECT 1 -- trailing"), vec!["SELECT 1"]);
    }
}
