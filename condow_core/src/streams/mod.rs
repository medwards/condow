//! Stream implememtations used by Condow
use std::fmt;

use crate::errors::IoError;
use bytes::Bytes;
use futures::stream::BoxStream;

mod chunk_stream;
mod part_stream;

pub use chunk_stream::*;
pub use part_stream::*;

/// A stream of [Bytes] (chunks) where there can be an error for each chunk of bytes
pub type BytesStream = BoxStream<'static, Result<Bytes, IoError>>;

/// Returns the bounds on the remaining bytes of the stream.
///
/// Specifically, `bytes_hint()` returns a tuple where the first element is
/// the lower bound, and the second element is the upper bound.
///
/// The second half of the tuple that is returned is an `Option<u64>`.
/// A None here means that either there is no known upper bound,
/// or the upper bound is larger than `u64`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BytesHint(u64, Option<u64>);

impl BytesHint {
    pub fn new(lower_bound: u64, upper_bound: Option<u64>) -> Self {
        BytesHint(lower_bound, upper_bound)
    }

    /// An exact number of bytes will be returned.
    pub fn new_exact(bytes: u64) -> Self {
        Self(bytes, Some(bytes))
    }

    /// Create a hint of `min=0` and `max=bytes` bytes
    pub fn new_at_max(bytes: u64) -> Self {
        Self(0, Some(bytes))
    }

    /// Creates a new hint which gives no hint at all.
    ///
    /// `(0, None)`
    pub fn new_no_hint() -> Self {
        Self(0, None)
    }

    /// Returns the lower bound
    pub fn lower_bound(&self) -> u64 {
        self.0
    }

    /// A `None` here means that either there is no known upper bound,
    /// or the upper bound is larger than usize.
    pub fn upper_bound(&self) -> Option<u64> {
        self.1
    }

    /// Returns true if this hint is an exact hint.
    ///
    /// This means that the lower bound must equal the upper bound: `(a, Some(a))`.
    pub fn is_exact(&self) -> bool {
        if let Some(upper) = self.1 {
            upper == self.0
        } else {
            false
        }
    }

    /// Returns the exact bytes if size hint specifies an exact amount of bytes.
    ///
    /// This means that the lower bound must equal the upper bound: `(a, Some(a))`.
    pub fn exact(&self) -> Option<u64> {
        if self.is_exact() {
            self.1
        } else {
            None
        }
    }

    /// Bytes have been send and `by` less will be received from now on
    pub fn reduce_by(&mut self, by: u64) {
        if by == 0 {
            return;
        }

        match self {
            BytesHint(0, None) => {}
            BytesHint(min, None) => {
                if *min >= by {
                    *min -= by;
                } else {
                    *self = BytesHint::new_no_hint()
                }
            }
            BytesHint(0, Some(max)) => {
                if *max >= by {
                    *max -= by;
                } else {
                    *self = BytesHint::new_no_hint()
                }
            }
            BytesHint(min, Some(max)) => {
                if *min >= by {
                    *min -= by;
                } else {
                    *min = 0;
                }

                if *max >= by {
                    *max -= by;
                } else {
                    self.1 = None
                }
            }
        }
    }

    pub fn combine(self, other: BytesHint) -> BytesHint {
        let (me_lower, me_upper) = self.into_inner();
        let (other_lower, other_upper) = other.into_inner();

        let lower_bound = me_lower + other_lower;

        let upper_bound = match (me_upper, other_upper) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(_), None) => None,
            (None, Some(_)) => None,
            (None, None) => None,
        };

        BytesHint::new(lower_bound, upper_bound)
    }

    /// Turns this into the inner tuple
    pub fn into_inner(self) -> (u64, Option<u64>) {
        (self.lower_bound(), self.upper_bound())
    }
}

impl fmt::Display for BytesHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (lower, upper) = self.into_inner();

        match upper {
            Some(upper) => write!(f, "[{}..{}]", lower, upper),
            None => write!(f, "[{}..?[", lower),
        }
    }
}
