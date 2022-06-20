//! XDR runtime encoding/decoding
//!
//! This crate provides runtime support for encoding and decoding XDR
//! data. It is intended to be used with code generated by the
//! "xdrgen" crate, but it can also be used with hand-written code.
//!
//! It provides two key traits - `Pack` and `Unpack` - which all
//! encodable types must implement. It also provides the helper
//! functions `pack()` and `unpack()` to simplify the API.
//!
//! By default, this does not implement codecs for `i8` or `u8`. This is because
//! encoding individual bytes is quite inefficient, as they're all padded up to
//! 32 bits (4 bytes). This doesn't matter for individual items, but arrays of
//! bytes should be represented by opaque arrays (static size) or flex arrays
//! (dynamic size) (or strings for character data).
//!
//! However, some protocols are mis-specified to use byte arrays (I'm looking at
//! you, gluster), so the option to support the exists. You can enable byte codec
//! with the `bytecodec` feature.
#![crate_type = "lib"]

extern crate byteorder;

pub use std::io::{Read, Write};
use std::ops::Deref;
use std::cmp::min;
use std::borrow::{Borrow, Cow};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

pub mod record;

mod error;
pub use error::*;

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod test;

static PADDING: [u8; 4] = [0; 4];

/// Compute XDR padding.
///
/// Return slice of zero padding needed to bring `sz` up to a multiple of 4. If no padding is needed,
/// it will be a zero-sized slice.
#[inline]
pub fn padding(sz: usize) -> &'static [u8] {
    &PADDING[..(4 - (sz % 4)) % 4]
}

/// Wrapper for XDR opaque data.
///
/// In XDR terms, "opaque data" is a plain array of bytes, packed as tightly as possible, and then
/// padded to a 4 byte offset. This is different from an array of bytes, where each byte would be
/// padded to 4 bytes when emitted into the array.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Opaque<'a>(pub Cow<'a, [u8]>);

impl<'a> Opaque<'a> {
    pub fn owned(v: Vec<u8>) -> Opaque<'a> {
        Opaque(Cow::Owned(v))
    }
    pub fn borrowed(v: &'a [u8]) -> Opaque<'a> {
        Opaque(Cow::Borrowed(v))
    }
}

impl<'a> Deref for Opaque<'a> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.0.deref()
    }
}

impl<'a> From<&'a [u8]> for Opaque<'a> {
    fn from(v: &'a [u8]) -> Self {
        Opaque::borrowed(v)
    }
}

/// Serialization (packing) helper.
///
/// Helper to serialize any type implementing `Pack` into an implementation of `std::io::Write`.
pub fn pack<Out: Write, T: Pack<Out>>(val: &T, out: &mut Out) -> Result<()> {
    val.pack(out).map(|_| ())
}

/// Pack a fixed-size array.
///
/// As the size is fixed, it doesn't need to be encoded. `sz` is in units of array elements.
/// If the `val` is too large, it is truncated; it is too small, then the array is padded out with
/// default values (if provided). If the array is too small and there's no pad/default value, then it fails
/// with `Error::InvalidLen`.
pub fn pack_array<Out, T>(val: &[T], sz: usize, out: &mut Out, defl: Option<&T>) -> Result<usize>
where
    Out: Write,
    T: Pack<Out>,
{
    let mut vsz = 0;
    let val = &val[..min(sz, val.len())];

    for v in val {
        vsz += v.pack(out)?;
    }
    assert!(vsz % 4 == 0);

    if val.len() < sz {
        if let Some(defl) = defl {
            for _ in val.len()..sz {
                vsz += defl.pack(out)?;
            }
        } else {
            return Err(Error::InvalidLen(sz));
        }
    }
    Ok(vsz)
}

/// Pack a fixed-size byte array
///
/// As size is fixed, it doesn't need to be encoded. `sz` is in bytes (and array elements, which are u8)
/// If the array is too large, it is truncated; if its too small its padded with `0x00`.
pub fn pack_opaque_array<Out: Write>(val: &[u8], sz: usize, out: &mut Out) -> Result<usize> {
    let mut vsz;
    let val = &val[..min(sz, val.len())];

    vsz = val.len();
    out.write_all(val)?;

    let p = padding(sz);
    for _ in val.len()..(sz + p.len()) {
        out.write_u8(0)?;
        vsz += 1;
    }

    Ok(vsz)
}

/// Pack a dynamically sized array, with size limit check.
///
/// This packs an array of packable objects, and also applies an optional size limit.
#[inline]
pub fn pack_flex<Out: Write, T: Pack<Out>>(
    val: &[T],
    maxsz: Option<usize>,
    out: &mut Out,
) -> Result<usize> {
    if maxsz.map_or(false, |m| val.len() > m) {
        return Err(Error::InvalidLen(maxsz.unwrap()));
    }

    val.pack(out)
}

/// Pack a dynamically sized opaque array, with size limit check.
///
/// This packs an array of packable objects, and also applies an optional size limit.
#[inline]
pub fn pack_opaque_flex<Out: Write>(
    val: &[u8],
    maxsz: Option<usize>,
    out: &mut Out,
) -> Result<usize> {
    if maxsz.map_or(false, |m| val.len() > m) {
        return Err(Error::invalidlen(maxsz.unwrap()));
    }

    Opaque::borrowed(val).pack(out)
}

/// Pack a string with size limit check.
#[inline]
pub fn pack_string<Out: Write>(val: &str, maxsz: Option<usize>, out: &mut Out) -> Result<usize> {
    pack_opaque_flex(val.as_bytes(), maxsz, out)
}

/// Unpack a fixed-sized array
///
/// Unpack a fixed-size array of elements. The results are placed in `array`, but the actual wire-size of
/// the array is `arraysz`. If the supplied `array` is too large, the remainer is filled in with the
/// default value (if provided); if it is too small, the excess elements are discarded.
///
/// If the provided array is too large and there is no default, then decoding fails with an `InvalidLen` error.
/// All the elements in `array` will be initialized after a successful return.
pub fn unpack_array<In, T>(
    input: &mut In,
    array: &mut [T],
    arraysz: usize,
    defl: Option<&T>,
) -> Result<usize>
where
    In: Read,
    T: Unpack<In> + Clone,
{
    #[inline]
    fn set<T>(p: &mut T, v: T) { *p = v }
    #[inline]
    fn drop<T>(_: &mut T) { }

    unpack_array_with(input, array, arraysz, set, drop, defl)
}

/// Specialized variant of `unpack_array` which initializes the element via a callback. This is primarily
/// so that the array can be uninitialized, and we initialize it element at a time with `ptr::write()`.
#[inline]
pub fn unpack_array_with<In, T>(
    input: &mut In,
    array: &mut [T],
    arraysz: usize,
    set: fn (&mut T, T),
    drop: fn(&mut T),
    defl: Option<&T>,
) -> Result<usize>
where
    In: Read,
    T: Unpack<In> + Clone,
{
    let mut rsz = 0;
    let sz = min(arraysz, array.len());
 
    // If we fail part way through then return the error and the index we got up to
    // so we can clean up the entries we did initialize.
    let res = (|| {
            for (idx, elem) in (&mut array[..sz]).into_iter().enumerate() {
                let (v, sz) = match Unpack::unpack(input) {
                    Ok(v) => v,
                    Err(e) => return Some((idx, e)),
                };
                rsz += sz;
                set(elem, v);
            }
            None
        })();
    if let Some((idx, err)) = res {
        for elem in &mut array[..idx] {
            drop(elem)
        };
        return Err(err);
    }

    // Fill in excess array entries with default values
    if arraysz < array.len() {
        if let Some(defl) = defl {
            for elem in &mut array[arraysz..] {
                set(elem, defl.clone());
            }
        } else {
            return Err(Error::InvalidLen(arraysz));
        }
    }

    // Mop up unused array entries on the wire
    if arraysz > array.len() {
        for _ in array.len()..arraysz {
            let (_, sz) = T::unpack(input)?;
            rsz += sz;
        }
    }
    assert!(rsz % 4 == 0);

    Ok(rsz)
}

/// Unpack a fixed-sized opaque array
///
/// Unpack a fixed-size array of raw bytes. The results are placed in `bytes`, but the actual wire-size of
/// the array is `bytesz`. If the supplied `bytes` is too large, the remainer is filled in with 0x00;
/// if it is too small, the excess elements are discarded.
///
/// All the bytes in `bytes` will be initialized after a successful call.
pub fn unpack_opaque_array<In: Read>(
    input: &mut In,
    bytes: &mut [u8],
    bytesz: usize,
) -> Result<usize> {
    let sz = min(bytesz, bytes.len());
    let mut rsz = 0;

    while rsz < sz {
        let r = input.read(&mut bytes[rsz..])?;
        rsz += r;
    }

    // Fill in excess
    if sz < bytes.len() {
        for b in &mut bytes[sz..] {
            *b = 0;
        }
    }

    // Mop up unused data on the wire and padding
    let p = padding(bytesz).len();
    if bytes.len() < bytesz + p {
        for _ in bytes.len()..(bytesz + p) {
            let _ = input.read_u8()?;
            rsz += 1;
        }
    }

    Ok(rsz)
}

/// Unpack a (perhaps) length-limited array
pub fn unpack_flex<In: Read, T: Unpack<In>>(
    input: &mut In,
    maxsz: Option<usize>,
) -> Result<(Vec<T>, usize)> {
    let (elems, mut sz) = Unpack::unpack(input)?;

    if maxsz.map_or(false, |m| elems > m) {
        return Err(Error::InvalidLen(maxsz.unwrap()));
    }

    let mut out = Vec::with_capacity(elems);

    for _ in 0..elems {
        let (e, esz) = Unpack::unpack(input)?;
        out.push(e);
        sz += esz;
    }

    let p = padding(sz);
    for _ in 0..p.len() {
        let _ = input.read_u8()?;
    }
    sz += p.len();

    Ok((out, sz))
}

/// Unpack a (perhaps) length-limited opaque array
///
/// Unpack an XDR encoded array of bytes, with an optional maximum length.
pub fn unpack_opaque_flex<In: Read>(
    input: &mut In,
    maxsz: Option<usize>,
) -> Result<(Vec<u8>, usize)> {
    let (elems, mut sz) = Unpack::unpack(input)?;

    if maxsz.map_or(false, |m| elems > m) {
        return Err(Error::invalidlen(maxsz.unwrap()));
    }

    let mut out = Vec::with_capacity(elems);

    sz += input.take(elems as u64).read_to_end(&mut out)?;

    let p = padding(sz);
    for _ in 0..p.len() {
        let _ = input.read_u8()?;
    }
    sz += p.len();

    Ok((out, sz))
}

/// Unpack (perhaps) length-limited string
pub fn unpack_string<In: Read>(input: &mut In, maxsz: Option<usize>) -> Result<(String, usize)> {
    let (v, sz) = unpack_opaque_flex(input, maxsz)?;

    String::from_utf8(v).map_err(Error::from).map(|s| (s, sz))
}

/// Basic packing trait.
///
/// This trait is used to implement XDR packing any Rust type into a
/// `Write` stream. It returns the number of bytes the encoding took.
///
/// This crate provides a number of implementations for all the basic
/// XDR types, and generated code will generally compose them to pack
/// structures, unions, etc.
///
/// Streams generated by `Pack` can be consumed by `Unpack`.
pub trait Pack<Out: Write> {
    fn pack(&self, out: &mut Out) -> Result<usize>;
}

#[cfg(feature = "bytecodec")]
impl<Out: Write> Pack<Out> for u8 {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        out.write_u32::<BigEndian>(*self as u32)
            .map_err(Error::from)
            .map(|_| 4)
    }
}

#[cfg(feature = "bytecodec")]
impl<Out: Write> Pack<Out> for i8 {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        out.write_i32::<BigEndian>(*self as i32)
            .map_err(Error::from)
            .map(|_| 4)
    }
}

impl<Out: Write> Pack<Out> for u32 {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        out.write_u32::<BigEndian>(*self).map_err(Error::from).map(
            |_| 4,
        )
    }
}

impl<Out: Write> Pack<Out> for i32 {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        out.write_i32::<BigEndian>(*self).map_err(Error::from).map(
            |_| 4,
        )
    }
}

impl<Out: Write> Pack<Out> for u64 {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        out.write_u64::<BigEndian>(*self).map_err(Error::from).map(
            |_| 8,
        )
    }
}

impl<Out: Write> Pack<Out> for i64 {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        out.write_i64::<BigEndian>(*self).map_err(Error::from).map(
            |_| 8,
        )
    }
}

impl<Out: Write> Pack<Out> for f32 {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        out.write_f32::<BigEndian>(*self).map_err(Error::from).map(
            |_| 4,
        )
    }
}

impl<Out: Write> Pack<Out> for f64 {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        out.write_f64::<BigEndian>(*self).map_err(Error::from).map(
            |_| 8,
        )
    }
}

impl<Out: Write> Pack<Out> for bool {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        (*self as u32).pack(out)
    }
}

impl<Out: Write> Pack<Out> for () {
    #[inline]
    fn pack(&self, _out: &mut Out) -> Result<usize> {
        Ok(0)
    }
}

impl<Out: Write> Pack<Out> for usize {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        (*self as u32).pack(out)
    }
}

impl<Out: Write, T: Pack<Out>> Pack<Out> for [T] {
    fn pack(&self, out: &mut Out) -> Result<usize> {
        let len = self.len();

        let mut sz = len.pack(out)?;
        for it in self {
            sz += it.pack(out)?;
        }

        let p = padding(sz);
        if p.len() > 0 {
            out.write_all(p)?;
            sz += p.len();
        }

        Ok(sz)
    }
}

impl<Out: Write, T: Pack<Out>> Pack<Out> for Vec<T> {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        if self.len() > u32::MAX as usize {
            return Err(Error::InvalidLen(self.len()));
        }

        (&self[..]).pack(out)
    }
}

impl<'a, Out: Write> Pack<Out> for Opaque<'a> {
    fn pack(&self, out: &mut Out) -> Result<usize> {
        let mut sz;
        let data: &[u8] = self.0.borrow();

        if data.len() > u32::max_value() as usize {
            return Err(Error::InvalidLen(data.len()));
        }

        sz = data.len().pack(out)?;

        out.write_all(data)?;
        sz += data.len();

        let p = padding(sz);
        if p.len() > 0 {
            out.write_all(p)?;
            sz += p.len();
        }

        Ok(sz)
    }
}

impl<Out: Write> Pack<Out> for str {
    #[inline]
    fn pack(&self, out: &mut Out) -> Result<usize> {
        Opaque::borrowed(self.as_bytes()).pack(out)
    }
}

impl<Out: Write, T: Pack<Out>> Pack<Out> for Option<T> {
    fn pack(&self, out: &mut Out) -> Result<usize> {
        match self {
            &None => false.pack(out),
            &Some(ref v) => {
                let sz = true.pack(out)? + v.pack(out)?;
                Ok(sz)
            }
        }
    }
}

impl<Out: Write, T: Pack<Out>> Pack<Out> for Box<T> {
    fn pack(&self, out: &mut Out) -> Result<usize> {
        let t: &T = self.borrow();
        t.pack(out)
    }
}

impl<'a, Out: Write, T> Pack<Out> for Cow<'a, T>
where
    T: 'a + Pack<Out> + ToOwned<Owned = T>,
{
    fn pack(&self, out: &mut Out) -> Result<usize> {
        let t: &T = self.borrow();
        t.pack(out)
    }
}

/// Deserialization (unpacking) helper function
///
/// This function will read encoded bytes from `input` (a `Read`
/// implementation) and return a fully constructed type (or an
/// error). This relies on type inference to determine which type is
/// to be unpacked, so its up to the calling envionment to clarify
/// this. (Generally it falls out quite naturally.)
pub fn unpack<In: Read, T: Unpack<In>>(input: &mut In) -> Result<T> {
    T::unpack(input).map(|(v, _)| v)
}

/// Basic unpacking trait
///
/// This trait is used to unpack a type from an XDR encoded byte
/// stream (encoded with `Pack`).  It returns the decoded instance and
/// the number of bytes consumed from the input.
///
/// This crate provides implementations for all the basic XDR types,
/// as well as for arrays.
pub trait Unpack<In: Read>: Sized {
    fn unpack(input: &mut In) -> Result<(Self, usize)>;
}

#[cfg(feature = "bytecodec")]
impl<In: Read> Unpack<In> for u8 {
    #[inline]
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        input.read_u32::<BigEndian>().map_err(Error::from).map(
            |v| {
                (v as u8, 4)
            },
        )
    }
}

#[cfg(feature = "bytecodec")]
impl<In: Read> Unpack<In> for i8 {
    #[inline]
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        input.read_i32::<BigEndian>().map_err(Error::from).map(
            |v| {
                (v as i8, 4)
            },
        )
    }
}

impl<In: Read> Unpack<In> for u32 {
    #[inline]
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        input.read_u32::<BigEndian>().map_err(Error::from).map(
            |v| (v, 4),
        )
    }
}

impl<In: Read> Unpack<In> for i32 {
    #[inline]
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        input.read_i32::<BigEndian>().map_err(Error::from).map(
            |v| (v, 4),
        )
    }
}

impl<In: Read> Unpack<In> for u64 {
    #[inline]
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        input.read_u64::<BigEndian>().map_err(Error::from).map(
            |v| (v, 8),
        )
    }
}

impl<In: Read> Unpack<In> for i64 {
    #[inline]
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        input.read_i64::<BigEndian>().map_err(Error::from).map(
            |v| (v, 8),
        )
    }
}

impl<In: Read> Unpack<In> for f32 {
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        input.read_f32::<BigEndian>().map_err(Error::from).map(
            |v| (v, 4),
        )
    }
}

impl<In: Read> Unpack<In> for f64 {
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        input.read_f64::<BigEndian>().map_err(Error::from).map(
            |v| (v, 8),
        )
    }
}

impl<In: Read> Unpack<In> for bool {
    #[inline]
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        i32::unpack(input).and_then(|(v, sz)| match v {
            0 => Ok((false, sz)),
            1 => Ok((true, sz)),
            v => Err(Error::InvalidEnum(v)),
        })
    }
}

impl<In: Read> Unpack<In> for () {
    #[inline]
    fn unpack(_input: &mut In) -> Result<(Self, usize)> {
        Ok(((), 0))
    }
}

impl<In: Read> Unpack<In> for usize {
    #[inline]
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        u32::unpack(input).map(|(v, sz)| (v as usize, sz))
    }
}

impl<In: Read, T: Unpack<In>> Unpack<In> for Vec<T> {
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        unpack_flex(input, None)
    }
}

impl<In: Read> Unpack<In> for String {
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        let (v, sz) = unpack_opaque_flex(input, None)?;
        String::from_utf8(v).map_err(Error::from).map(|s| (s, sz))
    }
}

impl<'a, In: Read> Unpack<In> for Opaque<'a> {
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        let (len, mut sz) = usize::unpack(input)?;
        let mut v = Vec::new();
        sz += input.by_ref().take(len as u64).read_to_end(&mut v)?;

        let p = padding(sz);
        for _ in 0..p.len() {
            let _ = input.read_u8()?;
            sz += 1;
        }

        Ok((Opaque(Cow::Owned(v)), sz))
    }
}

impl<In: Read, T: Unpack<In>> Unpack<In> for Option<T> {
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        let (have, mut sz) = Unpack::unpack(input)?;
        let ret = if have {
            let (v, osz) = Unpack::unpack(input)?;
            sz += osz;
            Some(v)
        } else {
            None
        };
        Ok((ret, sz))
    }
}

impl<In: Read, T: Unpack<In>> Unpack<In> for Box<T> {
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        let (b, sz) = Unpack::unpack(input)?;
        Ok((Box::new(b), sz))
    }
}

impl<'a, In: Read, T> Unpack<In> for Cow<'a, T>
where
    T: 'a + Unpack<In> + ToOwned<Owned = T>,
{
    fn unpack(input: &mut In) -> Result<(Self, usize)> {
        let (b, sz) = Unpack::unpack(input)?;
        Ok((Cow::Owned(b), sz))
    }
}
