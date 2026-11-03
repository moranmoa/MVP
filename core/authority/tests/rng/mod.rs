//! A tiny seeded generator for the tests: SplitMix64, Fisher-Yates shuffle.
//! It replaces `rand` so the generated logs (and the bounds measured on
//! them) never change with a crate version. Not for anything but tests.
#![allow(dead_code)]

use std::ops::Range;

pub struct StdRng(u64);

impl StdRng {
    pub fn seed_from_u64(seed: u64) -> StdRng {
        StdRng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `range` up to a modulo bias that doesn't matter here.
    pub fn gen_range<T: Int>(&mut self, range: Range<T>) -> T {
        let (low, high) = (range.start.to_u64(), range.end.to_u64());
        assert!(low < high, "empty range");
        T::from_u64(low + self.next_u64() % (high - low))
    }

    pub fn gen_bool(&mut self, p: f64) -> bool {
        ((self.next_u64() >> 11) as f64) / ((1u64 << 53) as f64) < p
    }

    pub fn gen<T: Int>(&mut self) -> T {
        T::from_u64(self.next_u64())
    }
}

/// The integer types the tests draw.
pub trait Int: Copy {
    fn to_u64(self) -> u64;
    fn from_u64(value: u64) -> Self;
}

macro_rules! int {
    ($($t:ty),*) => {$(
        impl Int for $t {
            fn to_u64(self) -> u64 {
                self as u64
            }
            fn from_u64(value: u64) -> Self {
                value as $t
            }
        }
    )*};
}
int!(u8, u16, u32, u64, usize, i32, i64);

pub trait SliceRandom<T> {
    fn shuffle(&mut self, rng: &mut StdRng);
    fn choose(&self, rng: &mut StdRng) -> Option<&T>;
}

impl<T> SliceRandom<T> for [T] {
    fn shuffle(&mut self, rng: &mut StdRng) {
        for i in (1..self.len()).rev() {
            let j = rng.gen_range(0..i + 1);
            self.swap(i, j);
        }
    }

    fn choose(&self, rng: &mut StdRng) -> Option<&T> {
        if self.is_empty() {
            None
        } else {
            self.get(rng.gen_range(0..self.len()))
        }
    }
}
