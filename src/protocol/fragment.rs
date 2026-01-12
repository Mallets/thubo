use super::{core::SeqNum, id};

/// Header for a fragmented message.
///
/// Large messages are split into multiple fragments for transmission. Each
/// fragment carries this header to identify its sequence number, fragment ID,
/// and whether it's the last fragment in the sequence.
///
/// # Wire Format
///
/// The fragment message structure is defined as follows:
///
/// ```text
///  7 6 5 4 3 2 1 0
/// +-+-+-+-+-+-+-+-+
/// |X|KND| FRAGMENT|
/// +-+---+---------+
/// %    frag id    %  Fragment identifier
/// +---------------+
/// ```
///
/// Where:
/// - KND: Kind of fragment
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FragmentHeader {
    /// The header byte containing message type, last flag, and QoS.
    pub(crate) header: u8,
    /// Fragment identifier within the message.
    pub(crate) frag_id: SeqNum,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum FragmentKind {
    First = 0b00 << Self::SHIFT,
    More = 0b01 << Self::SHIFT,
    Last = 0b10 << Self::SHIFT,
}

impl FragmentKind {
    pub(super) const SHIFT: u8 = 5;
    pub(super) const MASK: u8 = 0b11 << Self::SHIFT;

    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        match rand::random_range(0..3) {
            0 => Self::First,
            1 => Self::More,
            2 => Self::Last,
            _ => unreachable!(),
        }
    }
}

impl FragmentHeader {
    /// Creates a new fragment header with the specified QoS, sequence number,
    /// and fragment ID.
    pub(crate) const fn new(kind: FragmentKind, frag_id: SeqNum) -> Self {
        Self {
            header: id::FRAGMENT | kind as u8,
            frag_id,
        }
    }

    /// Constructs a fragment header from raw parts without validation.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `header` contains valid fragment message
    /// type and QoS bits.
    pub(crate) const unsafe fn from_parts(header: u8, frag_id: SeqNum) -> Self {
        Self { header, frag_id }
    }

    pub(crate) fn kind(&self) -> FragmentKind {
        unsafe { core::mem::transmute(self.header & FragmentKind::MASK) }
    }

    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        FragmentHeader::new(FragmentKind::rand(), SeqNum::rand())
    }
}
