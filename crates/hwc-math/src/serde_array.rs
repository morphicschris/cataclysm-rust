//! Serde support for fixed-size arrays of any length (serde's own impls stop at 32), for
//! `#[serde(with = "hwc_math::serde_array")]` on array fields of saved state.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{Error, SeqAccess, Visitor};
use serde::ser::SerializeTuple;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S: Serializer, T: Serialize, const N: usize>(a: &[T; N], s: S) -> Result<S::Ok, S::Error> {
    let mut t = s.serialize_tuple(N)?;
    for v in a {
        t.serialize_element(v)?;
    }
    t.end()
}

pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>, const N: usize>(d: D) -> Result<[T; N], D::Error> {
    struct V<T, const N: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for V<T, N> {
        type Value = [T; N];
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "an array of {N} elements")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<[T; N], A::Error> {
            let mut v = Vec::with_capacity(N);
            for i in 0..N {
                v.push(seq.next_element()?.ok_or_else(|| A::Error::invalid_length(i, &self))?);
            }
            v.try_into().map_err(|_| A::Error::custom("array length"))
        }
    }
    d.deserialize_tuple(N, V::<T, N>(PhantomData))
}
