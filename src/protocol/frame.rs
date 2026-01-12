use super::id;

/// Header for a complete (non-fragmented) message frame.
///
/// A frame represents a complete message that fits within a single transmission
/// unit. Unlike fragments, frames are self-contained and don't require
/// reassembly.
///
/// # Wire Format
///
/// The frame message structure is defined as follows:
///
/// ```text
///  7 6 5 4 3 2 1 0
/// +-+-+-+-+-+-+-+-+
/// |X|X|X|    FRAME|  Header byte: message type and QoS
/// +-+-+-+---------+
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameHeader {
    /// The header byte containing message type and QoS.
    pub(crate) header: u8,
}

impl FrameHeader {
    /// Creates a new frame header with the specified QoS and sequence number.
    pub(crate) const fn new() -> Self {
        Self { header: id::FRAME }
    }

    /// Constructs a frame header from raw parts without validation.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `header` contains valid frame message type and QoS bits.
    pub(crate) const unsafe fn from_u8(header: u8) -> Self {
        Self { header }
    }

    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        FrameHeader::new()
    }
}
