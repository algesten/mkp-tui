use serde::{Deserialize, Serialize};

/// Encode a value as a length-prefixed MessagePack frame.
/// Format: [4 bytes BE length][msgpack payload]
pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    let payload = rmp_serde::to_vec_named(value)?;
    let len = payload.len() as u32;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Decode a MessagePack payload (without the length prefix).
pub fn decode_frame<'a, T: Deserialize<'a>>(
    payload: &'a [u8],
) -> Result<T, rmp_serde::decode::Error> {
    rmp_serde::from_slice(payload)
}

/// Read a complete length-prefixed frame from a buffer.
/// Returns `Some((value, bytes_consumed))` if a complete frame is available,
/// `None` if more data is needed.
pub fn try_decode<'a, T: Deserialize<'a>>(
    buf: &'a [u8],
) -> Result<Option<(T, usize)>, rmp_serde::decode::Error> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let total = 4 + len;
    if buf.len() < total {
        return Ok(None);
    }
    let value = decode_frame(&buf[4..total])?;
    Ok(Some((value, total)))
}

/// Check if a complete frame is available and return its total size (header + payload).
/// Returns `None` if more data is needed.
pub fn frame_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let total = 4 + len;
    if buf.len() < total {
        return None;
    }
    Some(total)
}
