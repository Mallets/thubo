use crate::{
    buffers::{
        reader::{DidntRead, Reader},
        writer::{DidntWrite, Writer},
    },
    protocol::{FragmentHeader, FrameHeader, Message, MessageBody, MessageHeader, SeqNum, id},
};

pub(crate) mod core;
pub(crate) mod fragment;
pub(crate) mod frame;

pub(crate) trait WCodec<Message, Buffer> {
    type Output;
    fn write(self, buffer: Buffer, message: Message) -> Self::Output;
}

pub(crate) trait RCodec<Message, Buffer> {
    type Error;
    fn read(self, buffer: Buffer) -> Result<Message, Self::Error>;
}

// Calculate the length of the value once serialized
pub(crate) trait LCodec<Message> {
    fn w_len(self, message: Message) -> usize;
}

#[derive(Clone, Copy)]
pub(crate) struct ThuboCodec;

impl Default for ThuboCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl ThuboCodec {
    pub(crate) const fn new() -> Self {
        Self
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ThuboCodecHeader {
    pub(crate) header: u8,
}

impl LCodec<&MessageHeader> for ThuboCodec {
    fn w_len(self, x: &MessageHeader) -> usize {
        1 + self.w_len(&x.seq_num)
    }
}

impl<W> WCodec<&MessageHeader, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, message: &MessageHeader) -> Self::Output {
        let MessageHeader { header, seq_num } = message;

        self.write(&mut *writer, header)?;
        self.write(&mut *writer, seq_num)?;

        Ok(())
    }
}

impl<R> RCodec<MessageHeader, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<MessageHeader, Self::Error> {
        let header: u8 = self.read(&mut *reader)?;
        let seq_num: SeqNum = self.read(&mut *reader)?;
        let message = unsafe { MessageHeader::from_parts(header, seq_num) };

        Ok(message)
    }
}

impl LCodec<&MessageBody> for ThuboCodec {
    fn w_len(self, x: &MessageBody) -> usize {
        match x {
            MessageBody::Frame(f) => self.w_len(f),
            MessageBody::Fragment(f) => self.w_len(f),
        }
    }
}

impl<W> WCodec<&MessageBody, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, message: &MessageBody) -> Self::Output {
        match message {
            MessageBody::Frame(f) => self.write(&mut *writer, f),
            MessageBody::Fragment(f) => self.write(&mut *writer, f),
        }
    }
}

impl<R> RCodec<MessageBody, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<MessageBody, Self::Error> {
        let header: u8 = self.read(&mut *reader)?;

        let codec = ThuboCodecHeader { header };
        let body = match header & id::MASK {
            id::FRAME => {
                let frame: FrameHeader = codec.read(&mut *reader)?;
                MessageBody::Frame(frame)
            }
            id::FRAGMENT => {
                let fragment: FragmentHeader = codec.read(&mut *reader)?;
                MessageBody::Fragment(fragment)
            }
            _ => return Err(DidntRead),
        };

        Ok(body)
    }
}

impl LCodec<&Message> for ThuboCodec {
    fn w_len(self, x: &Message) -> usize {
        self.w_len(&x.header) + self.w_len(&x.body)
    }
}

impl<W> WCodec<&Message, &mut W> for ThuboCodec
where
    W: Writer,
{
    type Output = Result<(), DidntWrite>;

    fn write(self, writer: &mut W, message: &Message) -> Self::Output {
        self.write(&mut *writer, &message.header)?;
        self.write(&mut *writer, &message.body)?;
        Ok(())
    }
}

impl<R> RCodec<Message, &mut R> for ThuboCodec
where
    R: Reader,
{
    type Error = DidntRead;

    fn read(self, reader: &mut R) -> Result<Message, Self::Error> {
        let header: MessageHeader = self.read(&mut *reader)?;
        let body: MessageBody = self.read(&mut *reader)?;
        let message = Message { header, body };
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use rand::{
        distr::{Alphanumeric, SampleString},
        *,
    };

    use super::*;
    use crate::{
        buffers::{
            BoxBuf, Bytes, Chunk,
            reader::{HasReader, Reader},
            writer::HasWriter,
        },
        protocol::{FragmentHeader, FrameHeader},
    };

    #[test]
    fn bytes_test() {
        let mut buffer = vec![0u8; 64];

        let bytes = Bytes::new();
        let mut writer = buffer.writer();

        let codec = ThuboCodec::new();
        codec.write(&mut writer, &bytes).unwrap();
        println!("Buffer: {buffer:?}");

        let mut reader = buffer.reader();
        let ret: Bytes = codec.read(&mut reader).unwrap();
        assert_eq!(ret, bytes);
    }

    const NUM_ITER: usize = 100;
    const MAX_PAYLOAD_SIZE: usize = 256;

    macro_rules! run_single {
        ($type:ty, $rand:expr, $wcode:expr, $rcode:expr, $buff:expr) => {
            for _ in 0..NUM_ITER {
                let x: $type = $rand;

                $buff.clear();
                {
                    let mut writer = $buff.writer();
                    $wcode.write(&mut writer, &x).unwrap();
                }
                {
                    let mut reader = $buff.reader();
                    let y: $type = $rcode.read(&mut reader).unwrap();
                    assert_eq!(x, y);
                    assert!(!reader.can_read());
                }
            }
        };
    }

    macro_rules! run_fragmented {
        ($type:ty, $rand:expr, $wcode:expr, $rcode:expr) => {
            for _ in 0..NUM_ITER {
                let x: $type = $rand;

                let mut vbuf = vec![];
                {
                    let mut writer = vbuf.writer();
                    $wcode.write(&mut writer, &x).unwrap();
                }

                let mut bytes = Bytes::new();
                {
                    let mut reader = vbuf.reader();
                    while let Ok(b) = reader.read_u8() {
                        bytes.push(vec![b].into());
                    }

                    let mut reader = bytes.reader();
                    let y: $type = $rcode.read(&mut reader).unwrap();
                    assert_eq!(x, y);
                    assert!(!reader.can_read());
                }
            }
        };
    }

    macro_rules! run_buffers {
        ($type:ty, $rand:expr, $wcode:expr, $rcode:expr) => {
            println!("Vec<u8>: codec {}", std::any::type_name::<$type>());
            let mut buffer = vec![];
            run_single!($type, $rand, $wcode, $rcode, buffer);

            println!("BBuf: codec {}", std::any::type_name::<$type>());
            let mut buffer = BoxBuf::with_capacity(u16::MAX as usize);
            run_single!($type, $rand, $wcode, $rcode, buffer);

            println!("Bytes: codec {}", std::any::type_name::<$type>());
            let mut buffer = Bytes::new();
            run_single!($type, $rand, $wcode, $rcode, buffer);

            println!("Chunk: codec {}", std::any::type_name::<$type>());
            for _ in 0..NUM_ITER {
                let x: $type = $rand;

                let mut buffer = vec![];
                let mut writer = buffer.writer();
                $wcode.write(&mut writer, &x).unwrap();

                let mut chunk = Chunk::from(buffer);
                let mut reader = chunk.reader();
                let y: $type = $rcode.read(&mut reader).unwrap();
                assert_eq!(x, y);
                assert!(!reader.can_read());
            }

            println!("Fragmented: codec {}", std::any::type_name::<$type>());
            run_fragmented!($type, $rand, $wcode, $rcode)
        };
    }

    macro_rules! run {
        ($type:ty, $rand:expr) => {
            let codec = ThuboCodec::new();
            run_buffers!($type, $rand, codec, codec);
        };
        ($type:ty, $rand:expr, $wcode:block, $rcode:block) => {
            run_buffers!($type, $rand, $wcode, $rcode);
        };
    }

    // Core
    #[test]
    fn codec_vle() {
        let mut rng = rand::rng();

        run!(u8, { u8::MIN });
        run!(u8, { u8::MAX });
        run!(u8, { rng.random::<u8>() });

        run!(u16, { u16::MIN });
        run!(u16, { u16::MAX });
        run!(u16, { rng.random::<u16>() });

        run!(u32, { u32::MIN });
        run!(u32, { u32::MAX });
        run!(u32, { rng.random::<u32>() });

        run!(u64, { u64::MIN });
        run!(u64, { u64::MAX });
        let codec = ThuboCodec::new();
        for i in 1..=codec.w_len(u64::MAX) {
            run!(u64, { 1 << (7 * i) });
        }
        run!(u64, { rng.random::<u64>() });

        run!(usize, { usize::MIN });
        run!(usize, { usize::MAX });
        run!(usize, { rng.random_range(usize::MIN..=usize::MAX) });
    }

    #[test]
    fn codec_vle_len() {
        let codec = ThuboCodec::new();

        let mut buff = vec![];
        let mut writer = buff.writer();
        let n: u64 = 0;
        codec.write(&mut writer, n).unwrap();
        assert_eq!(codec.w_len(n), buff.len());

        for i in 1..=codec.w_len(u64::MAX) {
            let mut buff = vec![];
            let mut writer = buff.writer();
            let n: u64 = 1 << (7 * i);
            codec.write(&mut writer, n).unwrap();
            println!("vle len: {n} {buff:02x?}");
            assert_eq!(codec.w_len(n), buff.len());
        }

        let mut buff = vec![];
        let mut writer = buff.writer();
        let n = u64::MAX;
        codec.write(&mut writer, n).unwrap();
        assert_eq!(codec.w_len(n), buff.len());
    }

    #[test]
    fn codec_string() {
        let mut rng = rand::rng();
        run!(String, {
            let len = rng.random_range(0..16);
            Alphanumeric.sample_string(&mut rng, len)
        });
    }

    #[test]
    fn codec_dynbuf() {
        run!(Chunk, Chunk::rand(rand::rng().random_range(1..=MAX_PAYLOAD_SIZE)));
    }

    #[test]
    fn codec_bytes() {
        run!(Bytes, Bytes::rand(rand::rng().random_range(1..=MAX_PAYLOAD_SIZE)));
    }

    #[test]
    fn codec_frame() {
        run!(FrameHeader, FrameHeader::rand());
    }

    #[test]
    fn codec_fragment() {
        run!(FragmentHeader, FragmentHeader::rand());
    }

    #[test]
    fn codec_header() {
        run!(MessageHeader, MessageHeader::rand());
    }

    #[test]
    fn codec_body() {
        run!(MessageBody, MessageBody::rand());
    }

    #[test]
    fn codec_message() {
        run!(Message, Message::rand());
    }
}
