use super::{ThuboCodec, WCodec};
use crate::{
    buffers::{
        reader::{DidntRead, Reader},
        writer::{DidntWrite, Writer},
    },
    codec::{LCodec, RCodec, ThuboCodecHeader},
    protocol::{FragmentHeader, core::SeqNum, id},
};

impl LCodec<&FragmentHeader> for ThuboCodec {
    fn w_len(self, x: &FragmentHeader) -> usize {
        1 + self.w_len(&x.frag_id)
    }
}

impl<W> WCodec<&FragmentHeader, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, x: &FragmentHeader) -> Self::Output {
        let FragmentHeader { header, frag_id } = x;

        self.write(&mut *writer, header)?;
        self.write(&mut *writer, frag_id)?;

        Ok(())
    }
}

impl<R> RCodec<FragmentHeader, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<FragmentHeader, Self::Error> {
        let header: u8 = self.read(&mut *reader)?;
        let f_id = header & id::MASK;
        if f_id != id::FRAGMENT {
            return Err(DidntRead);
        }

        let codec = ThuboCodecHeader { header };
        codec.read(&mut *reader)
    }
}

impl<R> RCodec<FragmentHeader, &mut R> for ThuboCodecHeader
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<FragmentHeader, Self::Error> {
        let codec = ThuboCodec::new();
        let frag_id: SeqNum = codec.read(&mut *reader)?;

        let x = unsafe { FragmentHeader::from_parts(self.header, frag_id) };
        Ok(x)
    }
}
