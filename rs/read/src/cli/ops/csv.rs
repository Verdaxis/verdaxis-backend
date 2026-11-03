//! A bounded reader for operator CSV files: the Python `csv` default dialect
//! (comma, double quote, `""` inside quotes, `\r\n`, `\n` or `\r` records,
//! blank lines skipped) that `scripts/ingest_market_signals.py` read through
//! `csv.DictReader`.
//!
//! Python accepts some malformed input quietly; this reader refuses it
//! instead: text after a closing quote, a quote left open at the end, and a
//! record whose field count differs from the header's.
use std::collections::BTreeMap;

/// One data record by header name, with its 1-based data row number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub row_number: usize,
    pub fields: BTreeMap<String, String>,
}

/// Why a file was refused as a whole. The reason never quotes file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvRefusal(pub &'static str);

/// Parse records: the first non-blank record is the header, the rest are
/// data. A data record with the wrong field count is listed in
/// `malformed_rows` by row number, so the caller reports it as one row error.
pub struct Parsed {
    pub header: Vec<String>,
    pub records: Vec<Record>,
    pub malformed_rows: Vec<usize>,
}

pub fn parse(text: &str, max_rows: usize) -> Result<Parsed, CsvRefusal> {
    let mut rows = split_records(text)?.into_iter();
    let header = rows
        .next()
        .ok_or(CsvRefusal("the file has no header row"))?;
    if header.iter().any(String::is_empty) {
        return Err(CsvRefusal("the header has a blank column name"));
    }
    let mut seen = std::collections::BTreeSet::new();
    if !header.iter().all(|name| seen.insert(name.as_str())) {
        return Err(CsvRefusal("the header repeats a column name"));
    }
    let mut records = Vec::new();
    let mut malformed_rows = Vec::new();
    for (index, row) in rows.enumerate() {
        let row_number = index + 1;
        if row_number > max_rows {
            return Err(CsvRefusal("the file has too many rows"));
        }
        if row.len() != header.len() {
            malformed_rows.push(row_number);
            continue;
        }
        records.push(Record {
            row_number,
            fields: header.iter().cloned().zip(row).collect(),
        });
    }
    Ok(Parsed {
        header,
        records,
        malformed_rows,
    })
}

/// Split the text into records of fields; blank lines yield no record.
fn split_records(text: &str) -> Result<Vec<Vec<String>>, CsvRefusal> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    // Whether the current record has any field yet (a blank line has none).
    let mut started = false;
    while let Some(character) = chars.next() {
        match character {
            '"' if field.is_empty() => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') if chars.peek() == Some(&'"') => {
                            chars.next();
                            field.push('"');
                        }
                        Some('"') => break,
                        Some(other) => field.push(other),
                        None => return Err(CsvRefusal("a quoted field is not closed")),
                    }
                }
                match chars.peek() {
                    None | Some(',') | Some('\n') | Some('\r') => {}
                    Some(_) => return Err(CsvRefusal("text follows a closing quote")),
                }
            }
            ',' => {
                started = true;
                record.push(std::mem::take(&mut field));
            }
            '\r' | '\n' => {
                if character == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                if started || !field.is_empty() {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                started = false;
            }
            other => {
                started = true;
                field.push(other);
            }
        }
    }
    if started || !field.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Vec<Vec<String>> {
        split_records(text).unwrap()
    }

    #[test]
    fn reads_the_python_default_dialect() {
        assert_eq!(rows("a,b\r\n1,2\n"), [vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(rows("a,b\r1,2"), [vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(rows("a,b\n\n1,\n"), [vec!["a", "b"], vec!["1", ""]]);
        assert_eq!(
            rows("\"x,y\",\"say \"\"hi\"\"\"\n"),
            [vec!["x,y", "say \"hi\""]]
        );
        assert_eq!(rows("\"line\none\",2\n"), [vec!["line\none", "2"]]);
        assert_eq!(rows("a\"b,c\n"), [vec!["a\"b", "c"]]);
        assert_eq!(rows("\"\",x\n"), [vec!["", "x"]]);
    }

    #[test]
    fn refuses_what_python_would_accept_quietly() {
        assert!(split_records("\"open\n").is_err());
        assert!(split_records("\"ab\"c,d\n").is_err());
        assert!(parse("a,,b\n1,2,3\n", 10).is_err());
        assert!(parse("a,a\n1,2\n", 10).is_err());
        assert!(parse("", 10).is_err());
    }

    #[test]
    fn reports_a_wrong_field_count_by_row_and_bounds_the_rows() {
        let parsed = parse("a,b\n1,2\n3\n4,5,6\n7,8\n", 10).unwrap();
        assert_eq!(parsed.malformed_rows, [2, 3]);
        assert_eq!(
            parsed
                .records
                .iter()
                .map(|record| record.row_number)
                .collect::<Vec<_>>(),
            [1, 4]
        );
        assert_eq!(parsed.records[0].fields["b"], "2");
        assert_eq!(
            parse("a\n1\n2\n3\n", 2).err(),
            Some(CsvRefusal("the file has too many rows"))
        );
    }
}
