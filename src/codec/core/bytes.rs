use super::{LCodec, RCodec, ThuboCodec, WCodec};
use crate::buffers::{
    Bytes,
    reader::{DidntRead, Reader},
    writer::{DidntWrite, Writer},
};

impl<W> WCodec<&Bytes, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: &Bytes) -> Self::Output {
        self.write(&mut *writer, x.len())?;
        for s in x.chunks() {
            writer.write_chunk(s)?;
        }
        Ok(())
    }
}

impl<R> RCodec<Bytes, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<Bytes, Self::Error> {
        let len: usize = self.read(&mut *reader)?;
        let mut bytes = Bytes::new();
        reader.read_chunks(len, |s| bytes.push(s))?;
        Ok(bytes)
    }
}

impl LCodec<&Bytes> for ThuboCodec {
    fn w_len(self, message: &Bytes) -> usize {
        self.w_len(message.len()) + message.len()
    }
}
