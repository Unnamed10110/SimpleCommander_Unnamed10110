//! Volume-wide filename indexing: NTFS MFT enumeration via the USN
//! journal (Everything-style) with a walkdir fallback for non-NTFS or
//! non-elevated sessions.

pub mod mft;
pub mod fallback;
pub mod search;
