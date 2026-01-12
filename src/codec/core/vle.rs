use super::{LCodec, RCodec, ThuboCodec, WCodec};
use crate::buffers::{
    reader::{DidntRead, Reader},
    writer::{DidntWrite, Writer},
};

const VLE_LEN_MAX: usize = vle_len(u64::MAX);

pub(crate) const fn vle_len(x: u64) -> usize {
    const B1: u64 = u64::MAX << 7;
    const B2: u64 = u64::MAX << (7 * 2);
    const B3: u64 = u64::MAX << (7 * 3);
    const B4: u64 = u64::MAX << (7 * 4);
    const B5: u64 = u64::MAX << (7 * 5);
    const B6: u64 = u64::MAX << (7 * 6);
    const B7: u64 = u64::MAX << (7 * 7);
    const B8: u64 = u64::MAX << (7 * 8);

    if (x & B1) == 0 {
        1
    } else if (x & B2) == 0 {
        2
    } else if (x & B3) == 0 {
        3
    } else if (x & B4) == 0 {
        4
    } else if (x & B5) == 0 {
        5
    } else if (x & B6) == 0 {
        6
    } else if (x & B7) == 0 {
        7
    } else if (x & B8) == 0 {
        8
    } else {
        9
    }
}

impl LCodec<u64> for ThuboCodec {
    fn w_len(self, x: u64) -> usize {
        vle_len(x)
    }
}

impl LCodec<usize> for ThuboCodec {
    fn w_len(self, x: usize) -> usize {
        self.w_len(x as u64)
    }
}

impl LCodec<u32> for ThuboCodec {
    fn w_len(self, x: u32) -> usize {
        self.w_len(x as u64)
    }
}

impl LCodec<u16> for ThuboCodec {
    fn w_len(self, x: u16) -> usize {
        self.w_len(x as u64)
    }
}

impl LCodec<u8> for ThuboCodec {
    fn w_len(self, _: u8) -> usize {
        1
    }
}

// u8
impl<W> WCodec<u8, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: u8) -> Self::Output {
        writer.write_u8(x)
    }
}

impl<R> RCodec<u8, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<u8, Self::Error> {
        reader.read_u8()
    }
}

// u64
impl<W> WCodec<u64, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, mut x: u64) -> Self::Output {
        let write = move |buffer: &mut [u8]| {
            let mut len = 0;
            while (x & !0x7f_u64) != 0 {
                // SAFETY: buffer is guaranteed to be VLE_LEN long where VLE_LEN is
                //         the maximum number of bytes a VLE can take once encoded.
                //         I.e.: x is shifted 7 bits to the right every iteration,
                //         the loop is at most VLE_LEN iterations.
                unsafe {
                    *buffer.get_unchecked_mut(len) = (x as u8) | 0x80_u8;
                }
                len += 1;
                x >>= 7;
            }
            // In case len == VLE_LEN then all the bits have already been written in the
            // latest iteration. Else we haven't written all the necessary bytes
            // yet.
            if len != VLE_LEN_MAX {
                // SAFETY: buffer is guaranteed to be VLE_LEN long where VLE_LEN is
                //         the maximum number of bytes a VLE can take once encoded.
                //         I.e.: x is shifted 7 bits to the right every iteration,
                //         the loop is at most VLE_LEN iterations.
                unsafe {
                    *buffer.get_unchecked_mut(len) = x as u8;
                }
                len += 1;
            }
            // The number of written bytes
            len
        };
        // SAFETY: write algorithm guarantees than returned length is lesser than or
        // equal to `VLE_LEN_MAX`.
        unsafe { writer.with_slot(self.w_len(x), write)? };
        Ok(())
    }
}

impl<R> RCodec<u64, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<u64, Self::Error> {
        let mut b = reader.read_u8()?;

        let mut v = 0;
        let mut i = 0;
        // 7 * VLE_LEN is beyond the maximum number of shift bits
        while (b & 0x80_u8) != 0 && i != 7 * (VLE_LEN_MAX - 1) {
            v |= ((b & 0x7f_u8) as u64) << i;
            b = reader.read_u8()?;
            i += 7;
        }
        v |= (b as u64) << i;
        Ok(v)
    }
}

// Derive impls
macro_rules! uint_impl {
    ($uint:ty) => {
        impl<W> WCodec<$uint, &mut W> for ThuboCodec
        where
            W: Writer,
        {
            type Output = Result<(), DidntWrite>;

            fn write(self, writer: &mut W, x: $uint) -> Self::Output {
                self.write(writer, x as u64)
            }
        }

        impl<R> RCodec<$uint, &mut R> for ThuboCodec
        where
            R: Reader,
        {
            type Error = DidntRead;

            fn read(self, reader: &mut R) -> Result<$uint, Self::Error> {
                let x: u64 = self.read(reader)?;
                Ok(x as $uint)
            }
        }
    };
}

uint_impl!(u16);
uint_impl!(u32);
uint_impl!(usize);

macro_rules! uint_ref_impl {
    ($uint:ty) => {
        impl<W> WCodec<&$uint, &mut W> for ThuboCodec
        where
            W: Writer,
        {
            type Output = Result<(), DidntWrite>;

            fn write(self, writer: &mut W, x: &$uint) -> Self::Output {
                self.write(writer, *x)
            }
        }
    };
}

uint_ref_impl!(u8);
uint_ref_impl!(u16);
uint_ref_impl!(u32);
uint_ref_impl!(u64);
uint_ref_impl!(usize);
