use super::{LCodec, RCodec, ThuboCodec, WCodec};
use crate::buffers::{
    reader::{DidntRead, Reader},
    writer::{DidntWrite, Writer},
};

impl LCodec<&str> for ThuboCodec {
    fn w_len(self, x: &str) -> usize {
        self.w_len(x.as_bytes())
    }
}

impl LCodec<&String> for ThuboCodec {
    fn w_len(self, x: &String) -> usize {
        self.w_len(x.as_bytes())
    }
}

impl<W> WCodec<&str, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: &str) -> Self::Output {
        self.write(&mut *writer, x.as_bytes())
    }
}

impl<W> WCodec<&String, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: &String) -> Self::Output {
        self.write(&mut *writer, x.as_bytes())
    }
}

impl<R> RCodec<String, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<String, Self::Error> {
        let vec: Vec<u8> = self.read(&mut *reader)?;
        String::from_utf8(vec).map_err(|_| DidntRead)
    }
}
