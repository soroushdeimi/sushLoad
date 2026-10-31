//! Byte-range segmentation: equal split, IDM-style dynamic split and layout invariants.

#![allow(dead_code)]

use crate::model::Segment;

/// Never create a segment smaller than this many bytes.
pub const MIN_SEGMENT_SIZE: u64 = 1024 * 1024;
/// Only split a segment while it still has at least this much left to download.
pub const MIN_SPLIT_REMAINING: u64 = 2 * MIN_SEGMENT_SIZE;

/// Split `total` bytes into at most `connections` contiguous, inclusive ranges of roughly equal
/// size, never smaller than `MIN_SEGMENT_SIZE` (unless the whole file is smaller).
pub fn split_equal(total: u64, connections: u32) -> Vec<Segment> {
    if total == 0 {
        return Vec::new();
    }
    let wanted = connections.max(1) as u64;
    let max_parts = (total / MIN_SEGMENT_SIZE).max(1);
    let parts = wanted.min(max_parts);

    let base = total / parts;
    let extra = total % parts;
    let mut segments = Vec::with_capacity(parts as usize);
    let mut start = 0u64;
    for i in 0..parts {
        let len = base + if i < extra { 1 } else { 0 };
        let end = start + len - 1;
        segments.push(Segment {
            start,
            end,
            received: 0,
        });
        start = end + 1;
    }
    segments
}

/// Find the segment with the most bytes remaining and split its *unwritten tail* in half.
/// Returns the index of the newly appended segment, or `None` when nothing is worth splitting.
///
/// The segment serving the shrunk range observes the changed `end` through shared state; the
/// caller hands the returned segment to a free connection.
pub fn split_largest(segments: &mut Vec<Segment>) -> Option<usize> {
    let (index, remaining) = segments
        .iter()
        .enumerate()
        .map(|(i, s)| (i, s.remaining()))
        .max_by_key(|(_, remaining)| *remaining)?;

    if remaining < MIN_SPLIT_REMAINING {
        return None;
    }

    let from = segments[index].start + segments[index].received;
    let to = segments[index].end;
    let half = remaining / 2;
    let new_start = from + half;
    if new_start > to {
        return None;
    }

    segments[index].end = new_start - 1;
    let new_segment = Segment {
        start: new_start,
        end: to,
        received: 0,
    };
    segments.push(new_segment);
    Some(segments.len() - 1)
}

/// Verify the segment layout: every segment is well formed, `received` fits, and the segments
/// exactly cover `0..=total-1` with no overlap or gap (order in the vector is irrelevant).
pub fn check_invariants(segments: &[Segment], total: u64) -> Result<(), String> {
    if total == 0 {
        return if segments.is_empty() {
            Ok(())
        } else {
            Err("segments exist for an empty download".to_string())
        };
    }
    if segments.is_empty() {
        return Err("no segments for a non-empty download".to_string());
    }

    for (i, segment) in segments.iter().enumerate() {
        if segment.end < segment.start {
            return Err(format!(
                "segment {i} has end {} before start {}",
                segment.end, segment.start
            ));
        }
        if segment.received > segment.len() {
            return Err(format!(
                "segment {i} received {} exceeds length {}",
                segment.received,
                segment.len()
            ));
        }
    }

    let mut ranges: Vec<(u64, u64)> = segments.iter().map(|s| (s.start, s.end)).collect();
    ranges.sort_unstable();

    let mut expected = 0u64;
    for (i, (start, end)) in ranges.iter().copied().enumerate() {
        if start != expected {
            return Err(if start > expected {
                format!("gap before segment {i} at byte {expected}")
            } else {
                format!("overlap at segment {i} starting {start}")
            });
        }
        expected = end
            .checked_add(1)
            .ok_or_else(|| "segment end overflows u64".to_string())?;
    }
    if expected != total {
        return Err(format!("segments cover {expected} bytes, expected {total}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn splits_equal_ranges_covering_the_whole_file() {
        for (total, connections) in [
            (2 * MIB, 2),
            (10 * MIB, 8),
            (10 * MIB + 7, 4),
            (8 * MIB, 16),
        ] {
            let segments = split_equal(total, connections);
            assert_eq!(check_invariants(&segments, total), Ok(()));
            let expected = (connections as u64).min(total / MIN_SEGMENT_SIZE);
            assert_eq!(segments.len() as u64, expected);
            let longest = segments.iter().map(|s| s.len()).max().unwrap();
            let shortest = segments.iter().map(|s| s.len()).min().unwrap();
            assert!(longest - shortest <= 1);
        }
    }

    #[test]
    fn enforces_minimum_segment_size() {
        let segments = split_equal(MIB, 8);
        assert_eq!(segments.len(), 1);
        assert_eq!(check_invariants(&segments, MIB), Ok(()));

        let segments = split_equal(2 * MIB, 16);
        assert_eq!(segments.len(), 2);
        assert_eq!(check_invariants(&segments, 2 * MIB), Ok(()));

        let segments = split_equal(MIB - 1, 16);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].len(), MIB - 1);
    }

    #[test]
    fn handles_empty_and_zero_connection_inputs() {
        assert!(split_equal(0, 8).is_empty());
        assert_eq!(check_invariants(&[], 0), Ok(()));

        let segments = split_equal(3 * MIB, 0);
        assert_eq!(segments.len(), 1);
        assert_eq!(check_invariants(&segments, 3 * MIB), Ok(()));
    }

    #[test]
    fn dynamic_split_preserves_coverage_and_reduces_remaining() {
        let total = 8 * MIB;
        let mut segments = split_equal(total, 4);
        let before: u64 = segments.iter().map(|s| s.remaining()).max().unwrap();

        let index = split_largest(&mut segments).expect("split");
        assert_eq!(check_invariants(&segments, total), Ok(()));
        assert_eq!(segments.len(), 5);
        assert_eq!(segments[index].received, 0);
        assert!(segments[index].remaining() < before);
        assert!(segments[index].remaining() >= MIB);

        let largest_after = segments.iter().map(|s| s.remaining()).max().unwrap();
        assert!(largest_after <= before);
    }

    #[test]
    fn dynamic_split_only_touches_unwritten_tail() {
        let mut segments = vec![Segment {
            start: 0,
            end: 8 * MIB - 1,
            received: MIB,
        }];
        let index = split_largest(&mut segments).expect("split");
        assert_eq!(check_invariants(&segments, 8 * MIB), Ok(()));
        assert_eq!(segments[0].start, 0);
        assert_eq!(segments[0].received, MIB);
        assert!(segments[0].end >= segments[0].start + segments[0].received - 1);
        assert_eq!(segments[index].start, segments[0].end + 1);
        assert_eq!(segments[index].end, 8 * MIB - 1);
        assert!(segments[0].remaining() >= MIB);
        assert!(segments[index].remaining() >= MIB);
    }

    #[test]
    fn refuses_to_split_when_remaining_is_small() {
        let mut segments = vec![Segment {
            start: 0,
            end: MIN_SPLIT_REMAINING - 2,
            received: 0,
        }];
        assert_eq!(split_largest(&mut segments), None);
        assert_eq!(segments.len(), 1);
    }

    #[test]
    fn splits_only_the_segment_with_most_remaining() {
        let mut segments = vec![
            Segment {
                start: 0,
                end: 4 * MIB - 1,
                received: 4 * MIB,
            },
            Segment {
                start: 4 * MIB,
                end: 4 * MIB + 6 * MIB - 1,
                received: 0,
            },
            Segment {
                start: 10 * MIB,
                end: 10 * MIB + 2 * MIB - 1,
                received: 0,
            },
        ];
        let index = split_largest(&mut segments).expect("split");
        assert_eq!(segments[index].start, 4 * MIB + 3 * MIB);
        assert_eq!(segments[1].end, 4 * MIB + 3 * MIB - 1);
        assert_eq!(check_invariants(&segments, 12 * MIB), Ok(()));
    }

    #[test]
    fn invariant_checker_detects_bad_layouts() {
        let valid = [Segment {
            start: 0,
            end: 9,
            received: 0,
        }];
        assert!(check_invariants(&valid, 10).is_ok());

        let gap = [
            Segment {
                start: 0,
                end: 3,
                received: 0,
            },
            Segment {
                start: 5,
                end: 9,
                received: 0,
            },
        ];
        assert!(check_invariants(&gap, 10).is_err());

        let overlap = [
            Segment {
                start: 0,
                end: 5,
                received: 0,
            },
            Segment {
                start: 5,
                end: 9,
                received: 0,
            },
        ];
        assert!(check_invariants(&overlap, 10).is_err());

        let too_much_received = [Segment {
            start: 0,
            end: 9,
            received: 11,
        }];
        assert!(check_invariants(&too_much_received, 10).is_err());

        assert!(check_invariants(&valid, 11).is_err());
    }
}
