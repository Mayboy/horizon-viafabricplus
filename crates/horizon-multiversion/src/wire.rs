//! Rewriting a packet body field by field: [`Rw`] reads the old body and
//! writes the new one, passing fields through unchanged unless a step reads
//! and writes them itself (ViaVersion's PacketWrapper).
use horizon_protocol::{
    Error, Result,
    buf::{Reader, Writer},
};

pub struct Rw<'a> {
    pub r: Reader<'a>,
    pub w: Writer,
}
impl<'a> Rw<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            r: Reader::new(data),
            w: Writer::new(),
        }
    }
    /// The new body, with everything not read yet passed through.
    pub fn finish(mut self) -> Vec<u8> {
        let rest = self.r.rest();
        self.w.bytes(rest);
        self.w.data
    }
    /// The new body, dropping what was not read.
    pub fn written(self) -> Vec<u8> {
        self.w.data
    }

    /// Pass through whatever `read` consumes, returning what it read.
    pub fn pass<T>(&mut self, read: impl FnOnce(&mut Reader<'a>) -> Result<T>) -> Result<T> {
        let start = self.r.position();
        let value = read(&mut self.r)?;
        let bytes = self.r.since(start);
        self.w.bytes(bytes);
        Ok(value)
    }
    pub fn varint(&mut self) -> Result<i32> {
        self.pass(|r| r.varint())
    }
    pub fn bool(&mut self) -> Result<bool> {
        self.pass(|r| r.bool())
    }
    pub fn u8(&mut self) -> Result<u8> {
        self.pass(|r| r.u8())
    }
    pub fn i8(&mut self) -> Result<i8> {
        self.pass(|r| r.i8())
    }
    pub fn i16(&mut self) -> Result<i16> {
        self.pass(|r| r.i16())
    }
    pub fn i32(&mut self) -> Result<i32> {
        self.pass(|r| r.i32())
    }
    pub fn i64(&mut self) -> Result<i64> {
        self.pass(|r| r.i64())
    }
    pub fn f32(&mut self) -> Result<f32> {
        self.pass(|r| r.f32())
    }
    pub fn f64(&mut self) -> Result<f64> {
        self.pass(|r| r.f64())
    }
    pub fn uuid(&mut self) -> Result<u128> {
        self.pass(|r| r.uuid())
    }
    pub fn string(&mut self) -> Result<String> {
        self.pass(|r| r.string(32767))
    }
    pub fn nbt(&mut self) -> Result<horizon_world::nbt::Tag> {
        self.pass(|r| r.nbt())
    }
    /// A BlockPos packed in a long.
    pub fn pos(&mut self) -> Result<i64> {
        self.i64()
    }
    /// A VarInt written back through `map`.
    pub fn map_varint(&mut self, map: impl FnOnce(i32) -> i32) -> Result<i32> {
        let v = self.r.varint()?;
        let mapped = map(v);
        self.w.varint(mapped);
        Ok(mapped)
    }
    /// A VarInt-counted list, each element passed through `element`.
    pub fn list(&mut self, mut element: impl FnMut(&mut Self) -> Result<()>) -> Result<usize> {
        let n = self.r.count(self.r.remaining())?;
        self.w.varint(n as i32);
        for _ in 0..n {
            element(self)?;
        }
        Ok(n)
    }
    /// A flag, then `value` when it is set.
    pub fn optional(&mut self, value: impl FnOnce(&mut Self) -> Result<()>) -> Result<bool> {
        let present = self.bool()?;
        if present {
            value(self)?;
        }
        Ok(present)
    }
}

/// A table lookup old id -> new id; ids past the table stay as they are.
pub fn up(table: &[u16], id: i32) -> i32 {
    usize::try_from(id)
        .ok()
        .and_then(|i| table.get(i))
        .map_or(id, |&v| i32::from(v))
}

/// The inverse of an old -> new table: new id -> old id, `fallback` for the
/// new ids the old version lacks.
pub fn invert(table: &[u16], new_len: usize, fallback: i32) -> Vec<i32> {
    let mut out = vec![fallback; new_len.max(table.len())];
    for (old, &new) in table.iter().enumerate().rev() {
        if let Some(slot) = out.get_mut(usize::from(new)) {
            *slot = old as i32;
        }
    }
    out
}
pub fn down(table: &[i32], id: i32, fallback: i32) -> i32 {
    usize::try_from(id)
        .ok()
        .and_then(|i| table.get(i))
        .copied()
        .unwrap_or(fallback)
}

pub fn error(message: impl Into<String>) -> Error {
    Error::new(message)
}
