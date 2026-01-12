use super::{LCodec, RCodec, ThuboCodec, WCodec};
use crate::buffers::{
    reader::{DidntRead, Reader},
    writer::{DidntWrite, Writer},
};

// [u8; N]
macro_rules! array_impl {
    ($n:expr) => {
        impl<W> WCodec<[u8; $n], &mut W> for ThuboCodec
        where
            W: Writer,
        {
            type Output = Result<(), DidntWrite>;

            fn write(self, writer: &mut W, x: [u8; $n]) -> Self::Output {
                writer.write_exact(x.as_slice())
            }
        }

        impl<W> WCodec<&[u8; $n], &mut W> for ThuboCodec
        where
            W: Writer,
        {
            type Output = Result<(), DidntWrite>;

            fn write(self, writer: &mut W, x: &[u8; $n]) -> Self::Output {
                self.write(writer, *x)
            }
        }

        impl<R> RCodec<[u8; $n], &mut R> for ThuboCodec
        where
            R: Reader,
        {
            type Error = DidntRead;

            fn read(self, reader: &mut R) -> Result<[u8; $n], Self::Error> {
                let mut x = [0u8; $n];
                reader.read_exact(&mut x)?;
                Ok(x)
            }
        }

        impl LCodec<[u8; $n]> for ThuboCodec {
            fn w_len(self, _: [u8; $n]) -> usize {
                $n
            }
        }
    };
}

array_impl!(1);
array_impl!(2);
array_impl!(3);
array_impl!(4);
array_impl!(5);
array_impl!(6);
array_impl!(7);
array_impl!(8);
array_impl!(9);
array_impl!(10);
array_impl!(11);
array_impl!(12);
array_impl!(13);
array_impl!(14);
array_impl!(15);
array_impl!(16);
