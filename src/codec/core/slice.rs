use super::{LCodec, RCodec, ThuboCodec, WCodec};
use crate::buffers::{
    reader::{DidntRead, Reader},
    writer::{DidntWrite, Writer},
};

// &[u8] / Vec<u8>
impl LCodec<&[u8]> for ThuboCodec {
    fn w_len(self, x: &[u8]) -> usize {
        self.w_len(x.len()) + x.len()
    }
}

impl<W> WCodec<&[u8], &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: &[u8]) -> Self::Output {
        self.write(&mut *writer, x.len())?;
        if x.is_empty() { Ok(()) } else { writer.write_exact(x) }
    }
}

impl<W> WCodec<Vec<u8>, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: Vec<u8>) -> Self::Output {
        self.write(&mut *writer, x.as_slice())
    }
}

impl<R> RCodec<Vec<u8>, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<Vec<u8>, Self::Error> {
        let len: usize = self.read(&mut *reader)?;
        let mut buff = vec![0u8; len];
        if len != 0 {
            reader.read_exact(&mut buff[..])?;
        }
        Ok(buff)
    }
}
