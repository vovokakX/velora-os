//! Mass storage, bulk-only transport (BOT), with the SCSI commands a
//! flash drive needs: INQUIRY, TEST UNIT READY, READ CAPACITY (10),
//! READ (10), WRITE (10), SYNCHRONIZE CACHE (10) and REQUEST SENSE.

pub const SUBCLASS_SCSI: u8 = 6;
pub const PROTOCOL_BOT: u8 = 0x50;
/// Class request: mass storage reset; GET MAX LUN.
pub const REQ_RESET: u8 = 0xff;
pub const REQ_GET_MAX_LUN: u8 = 0xfe;

pub const CBW_LEN: usize = 31;
pub const CSW_LEN: usize = 13;

/// A command block wrapper for `cdb`.
pub fn cbw(tag: u32, data_len: u32, data_in: bool, lun: u8, cdb: &[u8]) -> [u8; CBW_LEN] {
    let mut b = [0u8; CBW_LEN];
    b[0..4].copy_from_slice(b"USBC");
    b[4..8].copy_from_slice(&tag.to_le_bytes());
    b[8..12].copy_from_slice(&data_len.to_le_bytes());
    b[12] = if data_in { 0x80 } else { 0 };
    b[13] = lun & 0xf;
    let n = cdb.len().min(16);
    b[14] = n as u8;
    b[15..15 + n].copy_from_slice(&cdb[..n]);
    b
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CswError {
    BadSignature,
    WrongTag,
    /// The command failed (status 1); REQUEST SENSE tells why.
    Failed,
    /// Phase error (status 2): the device needs a reset.
    PhaseError,
}

/// Checks a command status wrapper. Returns the data residue.
pub fn parse_csw(b: &[u8], tag: u32) -> Result<u32, CswError> {
    if b.len() < CSW_LEN || &b[0..4] != b"USBS" {
        return Err(CswError::BadSignature);
    }
    if u32::from_le_bytes(b[4..8].try_into().unwrap()) != tag {
        return Err(CswError::WrongTag);
    }
    let residue = u32::from_le_bytes(b[8..12].try_into().unwrap());
    match b[12] {
        0 => Ok(residue),
        1 => Err(CswError::Failed),
        _ => Err(CswError::PhaseError),
    }
}

pub fn inquiry() -> [u8; 6] {
    [0x12, 0, 0, 0, 36, 0]
}

pub fn test_unit_ready() -> [u8; 6] {
    [0; 6]
}

pub fn request_sense() -> [u8; 6] {
    [0x03, 0, 0, 0, 18, 0]
}

pub fn read_capacity() -> [u8; 10] {
    [0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}

fn rw10(op: u8, lba: u32, blocks: u16) -> [u8; 10] {
    let l = lba.to_be_bytes();
    let n = blocks.to_be_bytes();
    [op, 0, l[0], l[1], l[2], l[3], 0, n[0], n[1], 0]
}

pub fn read10(lba: u32, blocks: u16) -> [u8; 10] {
    rw10(0x28, lba, blocks)
}

pub fn write10(lba: u32, blocks: u16) -> [u8; 10] {
    rw10(0x2a, lba, blocks)
}

pub fn sync_cache() -> [u8; 10] {
    [0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}

/// READ CAPACITY (10) data: (number of blocks, block size).
pub fn parse_capacity(b: &[u8]) -> Option<(u64, u32)> {
    if b.len() < 8 {
        return None;
    }
    let last = u32::from_be_bytes(b[0..4].try_into().unwrap());
    let size = u32::from_be_bytes(b[4..8].try_into().unwrap());
    if size == 0 {
        return None;
    }
    Some((last as u64 + 1, size))
}

/// Vendor and product from INQUIRY data, trimmed.
pub fn parse_inquiry(b: &[u8]) -> Option<(&str, &str)> {
    if b.len() < 32 {
        return None;
    }
    let s = |r: core::ops::Range<usize>| core::str::from_utf8(&b[r]).ok().map(str::trim);
    Some((s(8..16)?, s(16..32)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_block_wrapper() {
        let c = cbw(7, 4096, true, 0, &read10(0x1234, 8));
        assert_eq!(&c[0..4], b"USBC");
        assert_eq!(u32::from_le_bytes(c[4..8].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(c[8..12].try_into().unwrap()), 4096);
        assert_eq!(c[12], 0x80);
        assert_eq!(c[14], 10);
        assert_eq!(&c[15..25], &[0x28, 0, 0, 0, 0x12, 0x34, 0, 0, 8, 0]);
        assert_eq!(cbw(1, 0, false, 0, &test_unit_ready())[12], 0);
    }

    #[test]
    fn command_status_wrapper() {
        let mut s = [0u8; 13];
        s[0..4].copy_from_slice(b"USBS");
        s[4..8].copy_from_slice(&9u32.to_le_bytes());
        assert_eq!(parse_csw(&s, 9), Ok(0));
        assert_eq!(parse_csw(&s, 8), Err(CswError::WrongTag));
        s[12] = 1;
        assert_eq!(parse_csw(&s, 9), Err(CswError::Failed));
        s[12] = 2;
        assert_eq!(parse_csw(&s, 9), Err(CswError::PhaseError));
        s[0] = b'X';
        assert_eq!(parse_csw(&s, 9), Err(CswError::BadSignature));
    }

    #[test]
    fn capacity_and_inquiry() {
        assert_eq!(
            parse_capacity(&[0, 0, 0xff, 0xff, 0, 0, 2, 0]),
            Some((0x10000, 512))
        );
        assert_eq!(parse_capacity(&[0; 8]), None);
        let mut q = [b' '; 36];
        q[8..12].copy_from_slice(b"QEMU");
        q[16..26].copy_from_slice(b"QEMU HARDD");
        assert_eq!(parse_inquiry(&q), Some(("QEMU", "QEMU HARDD")));
        assert_eq!(&write10(1, 2)[..2], &[0x2a, 0]);
    }
}
