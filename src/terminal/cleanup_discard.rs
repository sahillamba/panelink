use std::io::Read;

const DISCARD_BUFFER_BYTES: usize = 4096;
const DISCARD_READ_LIMIT: usize = 16;

/// Discard queued teardown output without letting a busy reader monopolize cleanup.
///
/// Returns `true` only when EOF was observed. Errors and an exhausted read
/// budget return `false`, allowing the caller to continue checking its child
/// status and cleanup deadline.
pub(super) fn discard_available(reader: &mut impl Read) -> bool {
    let mut buffer = [0_u8; DISCARD_BUFFER_BYTES];
    for _ in 0..DISCARD_READ_LIMIT {
        match reader.read(&mut buffer) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                return false;
            }
            Err(_) => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::discard_available;
    use std::io::{self, Read};

    struct AlwaysReadable {
        calls: usize,
        bytes: usize,
    }

    impl Read for AlwaysReadable {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            self.bytes += buffer.len();
            buffer.fill(b'x');
            Ok(buffer.len())
        }
    }

    struct Fragmented {
        remaining: usize,
        max_chunk: usize,
    }

    impl Read for Fragmented {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Ok(0);
            }
            let count = self.remaining.min(self.max_chunk).min(buffer.len());
            buffer[..count].fill(b'f');
            self.remaining -= count;
            Ok(count)
        }
    }

    struct FailsOnce {
        kind: io::ErrorKind,
        calls: usize,
    }

    impl Read for FailsOnce {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            Err(io::Error::from(self.kind))
        }
    }

    #[test]
    fn continuously_readable_source_stops_at_sixteen_reads() {
        let mut reader = AlwaysReadable { calls: 0, bytes: 0 };
        assert!(!discard_available(&mut reader));
        assert_eq!(reader.calls, 16);
        assert_eq!(reader.bytes, 65_536);
    }

    #[test]
    fn finite_output_is_discarded_until_eof() {
        let mut reader = io::Cursor::new(b"queued teardown output".to_vec());
        assert!(discard_available(&mut reader));
        assert_eq!(reader.position(), 22);
    }

    #[test]
    fn fragmented_output_reaches_eof() {
        let mut reader = Fragmented {
            remaining: 901,
            max_chunk: 137,
        };
        assert!(discard_available(&mut reader));
        assert_eq!(reader.remaining, 0);
    }

    #[test]
    fn read_errors_return_without_retrying() {
        for kind in [
            io::ErrorKind::WouldBlock,
            io::ErrorKind::Interrupted,
            io::ErrorKind::BrokenPipe,
        ] {
            let mut reader = FailsOnce { kind, calls: 0 };
            assert!(!discard_available(&mut reader), "{kind:?}");
            assert_eq!(reader.calls, 1, "{kind:?}");
        }
    }
}
