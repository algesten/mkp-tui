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

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use crate::Song;

    // Exact song shape understood before availability was added.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct LegacySong {
        id: String,
        title: String,
        artist_name: String,
        album_title: String,
        duration: f32,
        track_number: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artwork_url_small: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artwork_url_large: Option<String>,
    }

    #[test]
    fn song_availability_is_compatible_in_both_directions() {
        let legacy = LegacySong {
            id: "track".into(),
            title: "Track".into(),
            artist_name: "Artist".into(),
            album_title: "Album".into(),
            duration: 123.0,
            track_number: Some(1),
            url: None,
            artwork_url_small: None,
            artwork_url_large: None,
        };
        let old_frame = encode_frame(&legacy).unwrap();
        let (mut modern, _) = try_decode::<Song>(&old_frame).unwrap().unwrap();
        assert!(
            !modern.unavailable,
            "missing field must preserve legacy behavior"
        );
        // Playable songs keep the exact old wire representation.
        assert_eq!(encode_frame(&modern).unwrap(), old_frame);
        modern.unavailable = true;
        let new_frame = encode_frame(&modern).unwrap();
        let (decoded_old, _) = try_decode::<LegacySong>(&new_frame).unwrap().unwrap();
        assert_eq!(
            decoded_old, legacy,
            "old clients must ignore the added field"
        );
        let (decoded_new, _) = try_decode::<Song>(&new_frame).unwrap().unwrap();
        assert!(decoded_new.unavailable);
    }
}
