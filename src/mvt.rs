//! Plan IGN vector tiles made readable by walkers, which skips any layer whose extent (its
//! coordinate resolution) is not 4096. IGN serves its heaviest tiles at 2048 or 1024, e.g.
//! Europe at level 3, which would stay blank. Their coordinates are scaled up to 4096.

use std::borrow::Cow;

/// Extent walkers reads.
const EXTENT: u64 = 4096;

/// The tile with every layer at extent 4096; as it is if already so, or if unreadable.
pub fn to_4096(tile: &[u8]) -> Cow<'_, [u8]> {
    let Some(fields) = fields(tile) else {
        return Cow::Borrowed(tile);
    };
    // Tile: layers are field 3.
    let layers = fields.iter().filter_map(|f| match f {
        (3, Field::Bytes(layer)) => Some(*layer),
        _ => None,
    });
    if layers.into_iter().all(|l| extent(l) == Some(EXTENT)) {
        return Cow::Borrowed(tile);
    }
    let mut out = Vec::with_capacity(tile.len());
    for (number, field) in fields {
        match field {
            Field::Bytes(layer) if number == 3 => put_bytes(&mut out, 3, &layer_4096(layer)),
            _ => put(&mut out, number, &field),
        }
    }
    Cow::Owned(out)
}

/// A layer's extent (field 5, 4096 when absent); `None` if unreadable.
fn extent(layer: &[u8]) -> Option<u64> {
    let extent = fields(layer)?.into_iter().find_map(|f| match f {
        (5, Field::Varint(e)) => Some(e),
        _ => None,
    });
    Some(extent.unwrap_or(EXTENT))
}

/// The layer at extent 4096: its features' geometry (field 2, then 4) scaled.
fn layer_4096(layer: &[u8]) -> Vec<u8> {
    let (Some(parsed), Some(extent)) = (fields(layer), extent(layer)) else {
        return layer.to_vec();
    };
    if extent == 0 || !EXTENT.is_multiple_of(extent) {
        return layer.to_vec();
    }
    let k = (EXTENT / extent) as i64;
    let mut out = Vec::with_capacity(layer.len());
    for (number, field) in parsed {
        match (number, &field) {
            (2, Field::Bytes(feature)) => {
                let mut scaled = Vec::with_capacity(feature.len());
                for (n, f) in fields(feature).unwrap_or_default() {
                    match f {
                        Field::Bytes(g) if n == 4 => put_bytes(&mut scaled, 4, &scale(g, k)),
                        _ => put(&mut scaled, n, &f),
                    }
                }
                put_bytes(&mut out, 2, &scaled);
            }
            (5, _) => put(&mut out, 5, &Field::Varint(EXTENT)),
            _ => put(&mut out, number, &field),
        }
    }
    out
}

/// Packed geometry commands with their coordinates times `k`. Each command is an id
/// (1 MoveTo, 2 LineTo, 7 ClosePath) and a count; MoveTo and LineTo carry `count`
/// zigzag-encoded (dx, dy) pairs.
fn scale(mut packed: &[u8], k: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(packed.len() + packed.len() / 4);
    while let Some(command) = varint(&mut packed) {
        put_varint(&mut out, command);
        let params = if command & 7 == 7 {
            0
        } else {
            2 * (command >> 3)
        };
        for _ in 0..params {
            let Some(p) = varint(&mut packed) else { break };
            let v = ((p >> 1) as i64 ^ -((p & 1) as i64)) * k;
            put_varint(&mut out, ((v << 1) ^ (v >> 63)) as u64);
        }
    }
    out
}

/// A protobuf field's value: only varints and length-delimited ones are looked into.
enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    /// Fixed 64 or 32 bits, with its wire type.
    Fixed(u64, &'a [u8]),
}

/// A message's fields, by number, in order; `None` if malformed.
fn fields(mut b: &[u8]) -> Option<Vec<(u64, Field<'_>)>> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let key = varint(&mut b)?;
        let field = match key & 7 {
            0 => Field::Varint(varint(&mut b)?),
            2 => {
                let n = varint(&mut b)? as usize;
                let (v, rest) = b.split_at_checked(n)?;
                b = rest;
                Field::Bytes(v)
            }
            wire @ (1 | 5) => {
                let (v, rest) = b.split_at_checked(if wire == 1 { 8 } else { 4 })?;
                b = rest;
                Field::Fixed(wire, v)
            }
            _ => return None,
        };
        out.push((key >> 3, field));
    }
    Some(out)
}

fn put(out: &mut Vec<u8>, number: u64, field: &Field) {
    match field {
        Field::Varint(v) => {
            put_varint(out, number << 3);
            put_varint(out, *v);
        }
        Field::Bytes(v) => put_bytes(out, number, v),
        Field::Fixed(wire, v) => {
            put_varint(out, number << 3 | wire);
            out.extend_from_slice(v);
        }
    }
}

fn put_bytes(out: &mut Vec<u8>, number: u64, v: &[u8]) {
    put_varint(out, number << 3 | 2);
    put_varint(out, v.len() as u64);
    out.extend_from_slice(v);
}

fn varint(b: &mut &[u8]) -> Option<u64> {
    let mut v = 0;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = b.split_first()?;
        *b = rest;
        v |= ((byte & 0x7f) as u64) << shift;
        if byte < 0x80 {
            return Some(v);
        }
    }
    None
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile of one layer at `extent`, holding a point at (10, -3).
    fn tile(extent: u64) -> Vec<u8> {
        let mut feature = Vec::new();
        put(&mut feature, 3, &Field::Varint(1)); // point
        put_bytes(&mut feature, 4, &[9, 20, 5]); // MoveTo 1: zigzag 10, -3
        let mut layer = Vec::new();
        put(&mut layer, 15, &Field::Varint(2));
        put_bytes(&mut layer, 1, b"a");
        put_bytes(&mut layer, 2, &feature);
        put(&mut layer, 5, &Field::Varint(extent));
        let mut tile = Vec::new();
        put_bytes(&mut tile, 3, &layer);
        tile
    }

    #[test]
    fn coarse_layers_are_scaled_to_4096() {
        let out = to_4096(&tile(2048)).into_owned();
        let reader = mvt_reader::Reader::new(out.clone()).unwrap();
        assert_eq!(reader.get_layer_metadata().unwrap()[0].extent, 4096);
        // The point doubled: (20, -6), zigzag 40 and 11.
        let layer = fields(&out).unwrap().remove(0).1;
        let Field::Bytes(layer) = layer else { panic!() };
        let feature = fields(layer)
            .unwrap()
            .into_iter()
            .find(|f| f.0 == 2)
            .unwrap()
            .1;
        let Field::Bytes(feature) = feature else {
            panic!()
        };
        let geometry = fields(feature)
            .unwrap()
            .into_iter()
            .find(|f| f.0 == 4)
            .unwrap()
            .1;
        let Field::Bytes(geometry) = geometry else {
            panic!()
        };
        assert_eq!(geometry, [9, 40, 11]);
    }

    #[test]
    fn tiles_at_4096_are_left_as_they_are() {
        let tile = tile(4096);
        assert!(matches!(to_4096(&tile), Cow::Borrowed(_)));
    }
}
