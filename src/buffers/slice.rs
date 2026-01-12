use core::{mem, num::NonZeroUsize};

use super::{
    Chunk,
    reader::{DidntRead, DidntSiphon, HasReader, Reader, SiphonableReader},
    writer::{DidntWrite, HasWriter, Writer},
};

// Writer implementations
impl HasWriter for &mut [u8] {
    type Writer = Self;

    fn writer(self) -> Self::Writer {
        self
    }
}

impl Writer for &mut [u8] {
    fn write(&mut self, bytes: &[u8]) -> Result<NonZeroUsize, DidntWrite> {
        let Some(len) = NonZeroUsize::new(bytes.len().min(self.len())) else {
            return Err(DidntWrite);
        };
        let (to_write, remain) = mem::take(self).split_at_mut(len.get());
        to_write.copy_from_slice(&bytes[..len.get()]);
        *self = remain;
        Ok(len)
    }

    fn write_exact(&mut self, bytes: &[u8]) -> Result<(), DidntWrite> {
        let len = bytes.len();
        if self.len() < len {
            return Err(DidntWrite);
        }
        let _ = self.write(bytes);
        Ok(())
    }

    fn remaining(&self) -> usize {
        self.len()
    }

    unsafe fn with_slot<F>(&mut self, len: usize, write: F) -> Result<NonZeroUsize, DidntWrite>
    where
        F: FnOnce(&mut [u8]) -> usize,
    {
        if len > self.len() {
            return Err(DidntWrite);
        }
        let written = write(&mut self[..len]);
        // SAFETY: `written` < `len` is guaranteed by function contract
        *self = unsafe { mem::take(self).get_unchecked_mut(written..) };
        NonZeroUsize::new(written).ok_or(DidntWrite)
    }
}

// Reader implementations
impl HasReader for &[u8] {
    type Reader = Self;

    fn reader(self) -> Self::Reader {
        self
    }
}

impl Reader for &[u8] {
    fn read(&mut self, into: &mut [u8]) -> Result<NonZeroUsize, DidntRead> {
        let Some(len) = NonZeroUsize::new(self.len().min(into.len())) else {
            return Err(DidntRead);
        };
        let (to_write, remain) = self.split_at(len.get());
        into[..len.get()].copy_from_slice(to_write);
        *self = remain;
        Ok(len)
    }

    fn read_exact(&mut self, into: &mut [u8]) -> Result<(), DidntRead> {
        let len = into.len();
        if self.len() < len {
            return Err(DidntRead);
        }
        let (to_write, remain) = self.split_at(len);
        into[..len].copy_from_slice(to_write);
        *self = remain;
        Ok(())
    }

    fn read_u8(&mut self) -> Result<u8, DidntRead> {
        let mut buf = [0; 1];
        self.read(&mut buf)?;
        Ok(buf[0])
    }

    fn read_chunks<F: FnMut(Chunk)>(&mut self, len: usize, mut f: F) -> Result<(), DidntRead> {
        let chunk = self.read_chunk(len)?;
        f(chunk);
        Ok(())
    }

    fn read_chunk(&mut self, len: usize) -> Result<Chunk, DidntRead> {
        let mut buffer = vec![0u8; len];
        self.read_exact(&mut buffer)?;
        Ok(buffer.into())
    }

    fn remaining(&self) -> usize {
        self.len()
    }

    fn can_read(&self) -> bool {
        !self.is_empty()
    }
}

impl SiphonableReader for &[u8] {
    fn siphon<W>(&mut self, writer: &mut W) -> Result<NonZeroUsize, DidntSiphon>
    where
        W: Writer,
    {
        let res = writer.write(self).map_err(|_| DidntSiphon);
        if let Ok(len) = res {
            // SAFETY: len is returned from the writer, therefore it means
            //         len amount of bytes have been written to the slice.
            *self = super::unsafe_slice!(self, len.get()..);
        }
        res
    }
}
