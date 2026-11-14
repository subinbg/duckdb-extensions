// A SQL lexer, just precise enough to colour the editor and to complete the word at the caret.

/** Words the editor colours and offers for completion. */
export const KEYWORDS = `
  ALL ALTER ANALYZE AND ANTI ANY AS ASC ASOF ATTACH BEGIN BETWEEN BY CALL CASE CAST CHECK COLUMN COLUMNS COMMIT COPY
  CREATE CROSS CUBE CURRENT DATABASE DEFAULT DELETE DESC DESCRIBE DETACH DISTINCT DROP ELSE END EXCEPT EXCLUDE EXISTS
  EXPLAIN EXPORT FALSE FILTER FIRST FOLLOWING FOR FOREIGN FROM FULL FUNCTION GROUP GROUPING HAVING IF ILIKE IMPORT IN
  INDEX INNER INSERT INSTALL INTERSECT INTERVAL INTO IS JOIN KEY LAST LATERAL LEFT LIKE LIMIT LOAD MACRO MERGE NATURAL
  NOT NULL NULLS OFFSET ON OR ORDER OUTER OVER PARTITION PIVOT POSITIONAL PRAGMA PRECEDING PRIMARY QUALIFY RANGE
  RECURSIVE REFERENCES REPLACE RETURNING RIGHT ROLLBACK ROLLUP ROW ROWS SAMPLE SCHEMA SELECT SEMI SEQUENCE SET SETS SHOW
  SIMILAR SUMMARIZE TABLE TEMP TEMPORARY THEN TO TRANSACTION TRUE TRUNCATE TYPE UNBOUNDED UNION UNIQUE UNPIVOT UPDATE
  USE USING VALUES VIEW WHEN WHERE WINDOW WITH WITHIN`.trim().split(/\s+/);

const keywords = new Set(KEYWORDS);

export const isKeyword = (word) => keywords.has(word.toUpperCase());

// One alternative per kind of token. A string, quoted name or comment that is not closed runs to the end.
const TOKEN = new RegExp(
  [
    /(?<space>\s+)/,
    /(?<comment>--[^\n]*|\/\*[\s\S]*?(?:\*\/|$))/,
    /(?<string>'(?:[^']|'')*'?|"(?:[^"]|"")*"?|\$(?<tag>(?!\d)[\p{L}\p{N}_]*)\$[\s\S]*?(?:\$\k<tag>\$|$))/,
    /(?<number>\d[\p{L}\p{N}_.]*)/,
    /(?<word>[\p{L}_][\p{L}\p{N}_]*)/,
    /(?<other>[\s\S])/,
  ]
    .map((kind) => kind.source)
    .join("|"),
  "gu",
);

/** The tokens of `sql`, each with its kind and the range of the text it covers; together they cover all of it. */
export function tokenize(sql) {
  return Array.from(sql.matchAll(TOKEN), (match) => {
    const [kind] = Object.entries(match.groups).find(([kind, text]) => kind !== "tag" && text !== undefined);
    const start = match.index;
    return { kind: kind === "word" && isKeyword(match[0]) ? "keyword" : kind, start, end: start + match[0].length };
  });
}
