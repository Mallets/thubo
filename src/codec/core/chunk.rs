use super::{RCodec, ThuboCodec, WCodec};
use crate::{
    buffers::{
        Chunk,
        reader::{DidntRead, Reader},
        writer::{DidntWrite, Writer},
    },
    codec::LCodec,
};

impl LCodec<&Chunk> for ThuboCodec {
    fn w_len(self, message: &Chunk) -> usize {
        self.w_len(message.as_slice())
    }
}

impl<W> WCodec<&Chunk, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: &Chunk) -> Self::Output {
        self.write(&mut *writer, x.len())?;
        writer.write_chunk(x)?;
        Ok(())
    }
}

impl<R> RCodec<Chunk, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<Chunk, Self::Error> {
        let len: usize = self.read(&mut *reader)?;
        let chunk = reader.read_chunk(len)?;
        Ok(chunk)
    }
}
