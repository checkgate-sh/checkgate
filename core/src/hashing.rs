use std::io::Cursor;

/// MurmurHash3 (x86_32) implementation for zero-allocation deterministic hashing.
/// This guarantees consistent evaluation across platforms.
///
/// # Panics
///
/// Never in practice. `Cursor<&[u8]>` implements `Read` infallibly, so
/// `murmur3_32` can only return `Err` if that in-memory read fails. The `expect`
/// is deliberate: the alternative — falling back to a fixed hash — would put
/// every affected user in bucket 0 and silently skew every rollout and variant
/// split, which is far worse than failing loudly on a genuinely impossible path.
#[allow(
    clippy::expect_used,
    reason = "infallible: Read on an in-memory Cursor cannot fail; see # Panics"
)]
pub fn murmurhash3_x86_32(key: &[u8], seed: u32) -> u32 {
    let mut cursor = Cursor::new(key);
    murmur3::murmur3_32(&mut cursor, seed)
        .expect("murmur3_32 on in-memory Cursor<&[u8]> is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_murmurhash3_x86_32() {
        // Test vectors for MurmurHash3 x86_32
        assert_eq!(murmurhash3_x86_32(b"hello", 0), 613153351);
        assert_eq!(murmurhash3_x86_32(b"hello world", 42), 3926694905);
        assert_eq!(murmurhash3_x86_32(b"", 0), 0);
    }
}
