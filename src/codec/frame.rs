use super::{ThuboCodec, WCodec};
use crate::{
    buffers::{
        reader::{DidntRead, Reader},
        writer::{DidntWrite, Writer},
    },
    codec::{LCodec, RCodec, ThuboCodecHeader},
    protocol::{FrameHeader, id},
};

impl LCodec<&FrameHeader> for ThuboCodec {
    fn w_len(self, _x: &FrameHeader) -> usize {
        1
    }
}

impl<W> WCodec<&FrameHeader, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: &FrameHeader) -> Self::Output {
        let FrameHeader { header } = x;

        self.write(&mut *writer, header)?;

        Ok(())
    }
}

impl<R> RCodec<FrameHeader, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<FrameHeader, Self::Error> {
        let header: u8 = self.read(&mut *reader)?;
        if header & id::MASK != id::FRAME {
            return Err(DidntRead);
        }

        let codec = ThuboCodecHeader { header };
        codec.read(&mut *reader)
    }
}

impl<R> RCodec<FrameHeader, &mut R> for ThuboCodecHeader
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, _reader: &mut R) -> Result<FrameHeader, Self::Error> {
        let x = unsafe { FrameHeader::from_u8(self.header) };
        Ok(x)
    }
}
