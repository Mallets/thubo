pub(crate) mod core;
pub(crate) mod fragment;
pub(crate) mod frame;

pub use core::*;

pub(crate) use fragment::*;
pub(crate) use frame::*;

/// Sequence number for transport-level frames.
pub(crate) type FrameSn = u32;

/// Size of a message batch.
pub(crate) type BatchSize = u16;

pub(crate) mod id {
    /// Number of bits used for message type identification.
    pub(crate) const BITS: u32 = 5;

    /// Bit mask to extract message type from the header byte (0b11000000).
    pub(crate) const MASK: u8 = !(u8::MAX << BITS);

    /// Frame message identifier (0b01 in bits 7-6).
    pub(crate) const FRAME: u8 = 0b0;

    /// Fragment message identifier (0b10 in bits 7-6).
    pub(crate) const FRAGMENT: u8 = 0b1;
}

/// ```text
///  7 6 5 4 3 2 1 0
/// +-+-+-+-+-+-+-+-+
/// |X|X|X|S|E|D|pty|
/// +-+---+-+-+-+---+
/// %    seq num    %  Sequence number
/// +---------------+
/// ```
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MessageBody {
    #[allow(dead_code)]
    /// A complete message frame.
    Frame(FrameHeader),
    /// A message fragment (part of a larger message).
    Fragment(FragmentHeader),
}

impl MessageBody {
    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        if rand::random_bool(0.5) {
            Self::Frame(FrameHeader::rand())
        } else {
            Self::Fragment(FragmentHeader::rand())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MessageHeader {
    pub(crate) header: u8,
    pub(crate) seq_num: SeqNum,
}

impl MessageHeader {
    pub(crate) const fn new(qos: QoS, seq_num: SeqNum) -> Self {
        Self {
            header: qos.as_u8(),
            seq_num,
        }
    }

    pub(crate) const unsafe fn from_parts(header: u8, seq_num: SeqNum) -> Self {
        Self { header, seq_num }
    }

    // pub(crate) fn set_sync(&mut self) {
    //     self.header |= flag::S;
    // }

    /// Extracts the QoS from the header byte.
    pub(crate) fn qos(&self) -> QoS {
        unsafe { QoS::from_u8(self.header) }
    }

    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        Self::new(QoS::rand(), SeqNum::rand())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) header: MessageHeader,
    pub(crate) body: MessageBody,
}

impl Message {
    pub(crate) const fn new(header: MessageHeader, body: MessageBody) -> Self {
        Self { header, body }
    }

    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        Self {
            header: MessageHeader::rand(),
            body: MessageBody::rand(),
        }
    }
}
