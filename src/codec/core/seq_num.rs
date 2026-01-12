use super::{ThuboCodec, WCodec};
use crate::{
    buffers::{
        reader::{DidntRead, Reader},
        writer::{DidntWrite, Writer},
    },
    codec::{LCodec, RCodec},
    protocol::core::SeqNum,
};

impl LCodec<&SeqNum> for ThuboCodec {
    fn w_len(self, x: &SeqNum) -> usize {
        self.w_len(x.get())
    }
}

impl<W> WCodec<&SeqNum, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: &SeqNum) -> Self::Output {
        self.write(&mut *writer, x.get())
    }
}

impl<R> RCodec<SeqNum, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<SeqNum, Self::Error> {
        let x: u32 = self.read(&mut *reader)?;
        Ok(SeqNum::new(x))
    }
}
