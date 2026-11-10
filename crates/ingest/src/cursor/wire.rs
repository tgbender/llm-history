//! The protobuf fields needed to follow the selected message projection.
use anyhow::{Context, Result, bail, ensure};
use serde_json::value::RawValue;

fn varint(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first().context("truncated Cursor wire value")?;
        *bytes = rest;
        ensure!(shift != 63 || byte <= 1, "invalid Cursor wire value");
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    bail!("invalid Cursor wire value")
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    let (head, rest) = bytes
        .split_at_checked(n)
        .context("truncated Cursor wire field")?;
    *bytes = rest;
    Ok(head)
}

fn fields<'a>(
    mut bytes: &'a [u8],
    mut visit: impl FnMut(u64, &'a [u8]) -> Result<()>,
) -> Result<()> {
    while !bytes.is_empty() {
        let tag = varint(&mut bytes)?;
        ensure!(tag >> 3 != 0, "invalid Cursor wire field");
        match tag & 7 {
            0 => {
                varint(&mut bytes)?;
            }
            1 => {
                take(&mut bytes, 8)?;
            }
            2 => {
                let len = usize::try_from(varint(&mut bytes)?)?;
                visit(tag >> 3, take(&mut bytes, len)?)?;
            }
            5 => {
                take(&mut bytes, 4)?;
            }
            _ => bail!("unsupported Cursor wire type"),
        }
    }
    Ok(())
}

pub(super) fn references(bytes: &[u8], limit: usize) -> Result<Vec<(bool, [u8; 32])>> {
    let mut out = Vec::new();
    fields(bytes, |field, body| {
        if matches!(field, 1 | 2)
            && let Ok(hash) = body.try_into()
        {
            ensure!(out.len() < limit, "Cursor message reference limit exceeded");
            out.push((field == 2, hash));
        }
        Ok(())
    })?;
    Ok(out)
}

pub(super) fn message(bytes: &[u8]) -> Result<&RawValue> {
    if let Ok(raw) = serde_json::from_slice::<&RawValue>(bytes)
        && raw.get().starts_with('{')
    {
        return Ok(raw);
    }
    let mut json = None;
    fields(bytes, |field, body| {
        if field == 4
            && let Ok(raw) = serde_json::from_slice::<&RawValue>(body)
            && raw.get().starts_with('{')
        {
            json = Some(raw);
        }
        Ok(())
    })?;
    json.context("unsupported Cursor message blob")
}
