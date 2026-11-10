//! Convert DuckDB values to ordinary JSON values for SQL inspection.
use super::*;
use duckdb::types::{TimeUnit, Value as SqlValue};
use std::time::{Duration, Instant};

pub(super) fn cell(value: SqlValue, redact: bool, zoned: bool) -> Result<Value> {
    let time_only = matches!(&value, SqlValue::Time64(..));
    let text = |s: &str| json!(bounded(s, usize::MAX, redact));
    Ok(match value {
        SqlValue::Null => Value::Null,
        SqlValue::Boolean(v) => json!(v),
        SqlValue::TinyInt(v) => json!(v),
        SqlValue::SmallInt(v) => json!(v),
        SqlValue::Int(v) => json!(v),
        SqlValue::BigInt(v) => json!(v),
        SqlValue::HugeInt(v) => serde_json::Number::from_i128(v)
            .map(Value::Number)
            .unwrap_or_else(|| text(&v.to_string())),
        SqlValue::UTinyInt(v) => json!(v),
        SqlValue::USmallInt(v) => json!(v),
        SqlValue::UInt(v) => json!(v),
        SqlValue::UBigInt(v) => json!(v),
        SqlValue::UHugeInt(v) => serde_json::Number::from_u128(v)
            .map(Value::Number)
            .unwrap_or_else(|| text(&v.to_string())),
        SqlValue::Float(v) => json!(v),
        SqlValue::Double(v) => json!(v),
        SqlValue::Decimal(v) => text(&v.to_string()),
        SqlValue::Text(v) | SqlValue::Enum(v) => text(&v),
        SqlValue::Blob(v) | SqlValue::Geometry(v) => text(&format!("\\x{}", hex::encode(v))),
        SqlValue::Timestamp(unit, value) | SqlValue::Time64(unit, value) => {
            let scale = match unit {
                TimeUnit::Second => 1,
                TimeUnit::Millisecond => 1_000,
                TimeUnit::Microsecond => 1_000_000,
                TimeUnit::Nanosecond => 1_000_000_000,
            };
            let time = chrono::DateTime::from_timestamp(
                value.div_euclid(scale),
                (value.rem_euclid(scale) * (1_000_000_000 / scale)) as u32,
            )
            .context("timestamp outside supported range")?;
            if time_only {
                text(&time.time().to_string())
            } else if zoned {
                text(&format!("{}+00:00", time.naive_utc()))
            } else {
                text(&time.naive_utc().to_string())
            }
        }
        SqlValue::Date32(days) => {
            let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .unwrap()
                .checked_add_signed(chrono::Duration::days(i64::from(days)))
                .context("date outside supported range")?;
            text(&date.to_string())
        }
        SqlValue::Interval {
            months,
            days,
            nanos,
        } => text(&format!("{months} months {days} days {nanos} nanoseconds")),
        SqlValue::List(values) | SqlValue::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|v| cell(v, redact, false))
                .collect::<Result<_>>()?,
        ),
        SqlValue::Struct(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), cell(value.clone(), redact, false)?)))
                .collect::<Result<_>>()?,
        ),
        SqlValue::Map(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let key = cell(key.clone(), redact, false)?;
                    Ok((
                        key.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| key.to_string()),
                        cell(value.clone(), redact, false)?,
                    ))
                })
                .collect::<Result<_>>()?,
        ),
        SqlValue::Union(v) => cell(*v, redact, false)?,
        _ => bail!("unsupported SQL output type"),
    })
}

/// Bound one complete rendered cell, including structured SQL values.
pub(super) fn bound_cell(value: Value, max_bytes: usize) -> Result<(Value, bool)> {
    let text = match &value {
        Value::Null | Value::Bool(_) | Value::Number(_) => return Ok((value, false)),
        Value::String(text) => text.clone(),
        _ => serde_json::to_string(&value)?,
    };
    if text.len() <= max_bytes {
        return Ok((value, false));
    }
    let mut end = max_bytes.saturating_sub(3);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut text = text[..end].to_owned();
    if max_bytes >= 3 {
        text.push('…');
    }
    Ok((Value::String(text), true))
}

/// Truncated labels also become row keys, so colliding prefixes need distinct suffixes.
fn headers(names: &[String], max_bytes: usize) -> Result<(Vec<String>, usize)> {
    let mut used = std::collections::HashSet::new();
    let mut output = Vec::with_capacity(names.len());
    let mut truncated = 0;
    for (index, name) in names.iter().enumerate() {
        let (label, _) = bound_cell(Value::String(name.clone()), max_bytes)?;
        let mut label = label.as_str().unwrap().to_owned();
        let mut ordinal = index + 1;
        while used.contains(&label) {
            let suffix = format!("_{ordinal}");
            if suffix.len() > max_bytes {
                bail!(
                    "SQL header budget is too small for distinct column names; raise the output limits"
                );
            }
            let (prefix, _) = bound_cell(Value::String(name.clone()), max_bytes - suffix.len())?;
            label = format!("{}{suffix}", prefix.as_str().unwrap());
            ordinal += 1;
        }
        truncated += usize::from(label != *name);
        used.insert(label.clone());
        output.push(label);
    }
    Ok((output, truncated))
}

fn response(
    names: &[String],
    rows: &[Value],
    reasons: &[&str],
    truncated_cells: usize,
    truncated_headers: usize,
) -> Value {
    let mut value = json!({"columns":names,"rows":rows,"truncated":!reasons.is_empty()});
    if !reasons.is_empty() {
        value["truncation_reasons"] = json!(reasons);
    }
    if truncated_cells > 0 {
        value["truncated_cells"] = json!(truncated_cells);
    }
    if truncated_headers > 0 {
        value["truncated_headers"] = json!(truncated_headers);
    }
    value
}

impl Store {
    pub fn sql_bounded(
        &self,
        query: &str,
        limit: usize,
        max_chars: usize,
        max_output_bytes: usize,
        redact: bool,
        timeout: Duration,
    ) -> Result<Value> {
        if !self.read_only {
            bail!("SQL inspection requires a read-only store connection");
        }
        self.database.with_query_timeout(timeout, |deadline| {
            read_sql(
                self.database.connection(),
                query,
                limit,
                max_chars,
                max_output_bytes,
                redact,
                deadline,
            )
        })
    }
}

fn read_sql(
    connection: &Connection,
    query: &str,
    limit: usize,
    max_chars: usize,
    max_output_bytes: usize,
    redact: bool,
    deadline: Instant,
) -> Result<Value> {
    connection.execute_batch("SET enable_external_access=false")?;
    let mut st = connection.prepare(query)?;
    let mut rows = st.query([])?;
    let names = rows
        .as_ref()
        .context("missing SQL statement")?
        .column_names();
    let zoned = (0..names.len())
        .map(|i| {
            matches!(
                rows.as_ref().unwrap().column_type(i),
                duckdb::arrow::datatypes::DataType::Timestamp(_, Some(_))
            )
        })
        .collect::<Vec<_>>();
    // Reserve the output-limit marker too, so even an empty capped result fits.
    let mut header_bytes = max_chars.min(max_output_bytes);
    let (names, truncated_headers, mut reasons) = loop {
        let (labels, count) = headers(&names, header_bytes)?;
        let mut reasons = Vec::new();
        if count > 0 {
            reasons.push("header_byte_limit");
        }
        let mut reserved = reasons.clone();
        reserved.push("output_byte_limit");
        let overhead = serde_json::to_vec(&response(&labels, &[], &reserved, 0, count))?.len();
        if overhead <= max_output_bytes {
            break (labels, count, reasons);
        }
        if header_bytes == 0 {
            bail!(
                "SQL output budget is too small for headers and truncation metadata; raise --max-output-bytes"
            );
        }
        header_bytes /= 2;
    };
    let mut out = Vec::new();
    let mut truncated_cells = 0usize;
    let mut row_bytes = 0usize;
    while let Some(row) = rows.next()? {
        if out.len() == limit {
            reasons.push("row_limit");
            break;
        }
        let mut reserved = reasons.clone();
        reserved.push("output_byte_limit");
        if truncated_cells > 0 {
            reserved.push("cell_byte_limit");
        }
        let overhead = serde_json::to_vec(&response(
            &names,
            &[],
            &reserved,
            truncated_cells,
            truncated_headers,
        ))?
        .len();
        let remaining = max_output_bytes
            .saturating_sub(overhead)
            .saturating_sub(row_bytes);
        let mut object = serde_json::Map::new();
        let mut row_truncated_cells = 0;
        for (i, name) in names.iter().enumerate() {
            anyhow::ensure!(Instant::now() < deadline, "SQL query deadline reached");
            let value = sql_output::cell(row.get(i)?, redact, zoned[i])?;
            let (value, cut) = sql_output::bound_cell(value, max_chars.min(remaining))?;
            row_truncated_cells += usize::from(cut);
            object.insert(name.clone(), value);
        }
        let cost = serde_json::to_vec(&object)?.len() + usize::from(!out.is_empty());
        let next_truncated_cells = truncated_cells + row_truncated_cells;
        if truncated_cells == 0 && next_truncated_cells > 0 {
            reserved.push("cell_byte_limit");
        }
        let overhead = serde_json::to_vec(&response(
            &names,
            &[],
            &reserved,
            next_truncated_cells,
            truncated_headers,
        ))?
        .len();
        if overhead.saturating_add(row_bytes).saturating_add(cost) > max_output_bytes {
            reasons.push("output_byte_limit");
            break;
        }
        row_bytes += cost;
        truncated_cells = next_truncated_cells;
        out.push(Value::Object(object));
    }
    if truncated_cells > 0 {
        reasons.push("cell_byte_limit");
    }
    Ok(response(
        &names,
        &out,
        &reasons,
        truncated_cells,
        truncated_headers,
    ))
}
