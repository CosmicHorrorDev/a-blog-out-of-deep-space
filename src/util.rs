use std::{mem::size_of, sync::Arc};

use axum::{body::Bytes, http::HeaderValue};

pub trait TotalSize {
    fn total_size(&self) -> usize;
}

impl<T: TotalSize> TotalSize for Arc<T> {
    fn total_size(&self) -> usize {
        size_of::<Self>() + T::total_size(&self)
    }
}

impl TotalSize for HeaderValue {
    fn total_size(&self) -> usize {
        // probably slightly less than the actual size
        size_of::<Self>() + self.as_bytes().len()
    }
}

impl TotalSize for Bytes {
    fn total_size(&self) -> usize {
        // probably slightly less than the actual size
        size_of::<Self>() + self.len()
    }
}

pub mod disp {
    use std::{
        fmt,
        time::{Duration as StdDuration, SystemTime},
    };

    pub struct Duration(pub StdDuration);

    impl fmt::Display for Duration {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{:.2?}", self.0)
        }
    }

    pub struct Time(pub SystemTime);

    impl fmt::Display for Time {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{:.2?}", self.0)
        }
    }

    pub struct HumanBytes(pub usize);

    impl fmt::Display for HumanBytes {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            enum Suffix {
                B,
                Kib,
                Mib,
            }

            impl fmt::Display for Suffix {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    let s = match self {
                        Self::B => "B",
                        Self::Kib => "KiB",
                        Self::Mib => "MiB",
                    };
                    f.write_str(s)
                }
            }

            let mut size = self.0 as f32;
            let mut suffix = Suffix::B;
            while size > 1_000.0 {
                suffix = match suffix {
                    Suffix::B => Suffix::Kib,
                    Suffix::Kib => Suffix::Mib,
                    Suffix::Mib => break,
                };
                size /= 1_024.0;
            }

            write!(f, "{size:.2} {suffix}")
        }
    }
}
