use std::io::{self, Read};

/// MurmurHash3 (x86_32) of one byte slice. The evaluator uses
/// [`murmurhash3_x86_32_concat`]; this is kept as the independent reference
/// the tests compare it against.
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
#[cfg(test)]
pub fn murmurhash3_x86_32(key: &[u8], seed: u32) -> u32 {
    let mut cursor = std::io::Cursor::new(key);
    murmur3::murmur3_32(&mut cursor, seed)
        .expect("murmur3_32 on in-memory Cursor<&[u8]> is infallible")
}

/// MurmurHash3 (x86_32) of the concatenation of `parts`, without building the
/// concatenation in memory — deterministic across platforms, zero allocations.
///
/// `murmurhash3_x86_32_concat(&[a, b, c], seed)` is bit-for-bit equal to
/// `murmurhash3_x86_32(&[a, b, c].concat(), seed)` — the reader below hands
/// `murmur3_32` the same byte stream, and `murmur3_32` fills each 4-byte block
/// across short reads, so where the part boundaries fall does not matter. That
/// equality is what keeps every user in the bucket they were already in.
///
/// # Panics
///
/// Never in practice, for the same reason as [`murmurhash3_x86_32`]: reading
/// from in-memory slices cannot fail.
#[allow(
    clippy::expect_used,
    reason = "infallible: Read over in-memory slices cannot fail; see # Panics"
)]
pub fn murmurhash3_x86_32_concat(parts: &[&[u8]], seed: u32) -> u32 {
    let mut reader = Concat {
        rest: parts.iter(),
        current: &[],
    };
    murmur3::murmur3_32(&mut reader, seed).expect("murmur3_32 over in-memory slices is infallible")
}

/// A `Read` over several byte slices in sequence (an allocation-free
/// `[..].concat()`).
struct Concat<'a> {
    rest: std::slice::Iter<'a, &'a [u8]>,
    current: &'a [u8],
}

impl Read for Concat<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.current.is_empty() {
                // `Read for &[u8]` copies a prefix and advances the slice.
                return self.current.read(buf);
            }
            match self.rest.next() {
                Some(next) => self.current = next,
                None => return Ok(0),
            }
        }
    }
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

    #[test]
    fn concat_matches_the_known_vectors() {
        assert_eq!(murmurhash3_x86_32_concat(&[b"hel", b"lo"], 0), 613153351);
        assert_eq!(
            murmurhash3_x86_32_concat(&[b"hello", b" ", b"world"], 42),
            3926694905
        );
        assert_eq!(murmurhash3_x86_32_concat(&[], 0), 0);
        assert_eq!(murmurhash3_x86_32_concat(&[b"", b"", b""], 0), 0);
    }

    /// Bucketing must not move anyone: for every split of every input length
    /// 0..=40 (so every tail length and every block-straddling boundary), the
    /// streamed hash equals the hash of the concatenated bytes.
    #[test]
    fn concat_equals_hash_of_concatenation_for_every_split() {
        let data: Vec<u8> = (0u8..=40).map(|i| i.wrapping_mul(37) ^ 0x5a).collect();
        for len in 0..=data.len() {
            let Some(input) = data.get(..len) else {
                continue;
            };
            let expected = murmurhash3_x86_32(input, 7);
            for i in 0..=len {
                for j in i..=len {
                    let (Some(a), Some(b), Some(c)) =
                        (input.get(..i), input.get(i..j), input.get(j..))
                    else {
                        continue;
                    };
                    assert_eq!(
                        murmurhash3_x86_32_concat(&[a, b, c], 7),
                        expected,
                        "len {len}, split at {i}/{j}"
                    );
                }
            }
        }
    }

    /// The exact key shapes the evaluator hashes.
    #[test]
    fn concat_matches_the_evaluator_key_formats() {
        for (flag, user) in [("checkout", "user-1"), ("", ""), ("ü-flag", "ユーザー")] {
            assert_eq!(
                murmurhash3_x86_32_concat(&[flag.as_bytes(), b":", user.as_bytes()], 0),
                murmurhash3_x86_32(format!("{flag}:{user}").as_bytes(), 0)
            );
            assert_eq!(
                murmurhash3_x86_32_concat(
                    &[flag.as_bytes(), b":", user.as_bytes(), b":variant"],
                    0
                ),
                murmurhash3_x86_32(format!("{flag}:{user}:variant").as_bytes(), 0)
            );
        }
    }
}
