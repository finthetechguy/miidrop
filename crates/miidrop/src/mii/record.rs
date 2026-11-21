//! The 96-byte Mii record carried in a transfer.

use std::fmt;

use crate::Error;

pub const RECORD_SIZE: usize = 0x60;
/// Size of a record as kept in a Mii database, without padding and CRC.
pub const STORED_SIZE: usize = 0x5c;

const FAVOURITE_BIT: u16 = 1 << 14;

/// A packed bit field inside the record, with the range a receiver accepts.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    pub offset: usize,
    pub shift: u32,
    pub width: u32,
    pub min: u32,
    pub max: u32,
}

const fn f(name: &'static str, offset: usize, shift: u32, width: u32, min: u32, max: u32) -> Field {
    Field {
        name,
        offset,
        shift,
        width,
        min,
        max,
    }
}

/// Appearance and header fields a receiver range-checks.
pub const FIELDS: &[Field] = &[
    f("face_shape", 0x30, 1, 4, 0, 11),
    f("skin_colour", 0x30, 5, 3, 0, 5),
    f("wrinkles", 0x30, 8, 4, 0, 11),
    f("makeup", 0x30, 12, 4, 0, 11),
    f("hair_style", 0x32, 0, 8, 0, 131),
    f("hair_colour", 0x32, 8, 3, 0, 7),
    f("hair_flip", 0x32, 11, 1, 0, 1),
    f("eye_type", 0x34, 0, 6, 0, 59),
    f("eye_colour", 0x34, 6, 3, 0, 5),
    f("eye_scale", 0x34, 9, 4, 0, 7),
    f("eye_aspect", 0x34, 13, 3, 0, 6),
    f("eye_rotation", 0x36, 0, 5, 0, 7),
    f("eye_spacing", 0x36, 5, 4, 0, 12),
    f("eye_y", 0x36, 9, 5, 0, 18),
    f("eyebrow_type", 0x38, 0, 5, 0, 23),
    f("eyebrow_colour", 0x38, 5, 3, 0, 7),
    f("eyebrow_scale", 0x38, 8, 4, 0, 8),
    f("eyebrow_aspect", 0x38, 12, 3, 0, 6),
    f("eyebrow_rotation", 0x3a, 0, 5, 0, 11),
    f("eyebrow_spacing", 0x3a, 5, 4, 0, 12),
    f("eyebrow_y", 0x3a, 9, 5, 3, 18),
    f("nose_type", 0x3c, 0, 5, 0, 17),
    f("nose_scale", 0x3c, 5, 4, 0, 8),
    f("nose_y", 0x3c, 9, 5, 0, 18),
    f("mouth_type", 0x3e, 0, 6, 0, 35),
    f("mouth_colour", 0x3e, 6, 3, 0, 4),
    f("mouth_scale", 0x3e, 9, 4, 0, 8),
    f("mouth_aspect", 0x3e, 13, 3, 0, 6),
    f("mouth_y", 0x40, 0, 5, 0, 18),
    f("moustache_type", 0x40, 5, 3, 0, 5),
    f("beard_type", 0x42, 0, 3, 0, 5),
    f("facial_hair_colour", 0x42, 3, 3, 0, 7),
    f("moustache_scale", 0x42, 6, 4, 0, 8),
    f("moustache_y", 0x42, 10, 5, 0, 16),
    f("glasses_type", 0x44, 0, 4, 0, 8),
    f("glasses_colour", 0x44, 4, 3, 0, 5),
    f("glasses_scale", 0x44, 7, 4, 0, 7),
    f("glasses_y", 0x44, 11, 5, 0, 20),
    f("mole", 0x46, 0, 1, 0, 1),
    f("mole_scale", 0x46, 1, 4, 0, 8),
    f("mole_x", 0x46, 5, 5, 0, 16),
    f("mole_y", 0x46, 10, 5, 0, 30),
    f("height", 0x2e, 0, 8, 0, 128),
    f("weight", 0x2f, 0, 8, 0, 128),
    f("gender", 0x18, 0, 1, 0, 1),
    f("favourite_colour", 0x18, 10, 4, 0, 11),
    f("region_lock", 0x00, 10, 2, 0, 3),
    f("char_set", 0x00, 12, 2, 0, 3),
    f("page", 0x00, 16, 4, 0, 9),
    f("slot", 0x00, 20, 4, 0, 9),
    f("origin", 0x00, 28, 3, 1, 3),
];

const MONTH_DAYS: [u32; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// A reason a receiver would refuse the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    Crc,
    Range { field: &'static str, value: u32 },
    Birthday { month: u32, day: u32 },
    EmptyName,
    NameCharacter,
    Identity,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::Crc => write!(f, "CRC does not match"),
            Issue::Range { field, value } => write!(f, "{field} out of range ({value})"),
            Issue::Birthday { month, day } => write!(f, "invalid birthday {month}/{day}"),
            Issue::EmptyName => write!(f, "name is empty"),
            Issue::NameCharacter => write!(f, "name or creator contains % or \\"),
            Issue::Identity => write!(f, "Mii ID flags are not accepted"),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Record(pub [u8; RECORD_SIZE]);

impl fmt::Debug for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Record({:08x} {:?})", self.mii_id(), self.name())
    }
}

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn utf16(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect()
}

impl Record {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let arr: [u8; RECORD_SIZE] = bytes.try_into().map_err(|_| {
            Error::Format(format!(
                "record must be {RECORD_SIZE} bytes, got {}",
                bytes.len()
            ))
        })?;
        Ok(Record(arr))
    }

    /// Builds a record from the stored form, adding padding and a CRC.
    pub fn from_stored(stored: &[u8]) -> Result<Self, Error> {
        if stored.len() != STORED_SIZE {
            return Err(Error::Format(format!(
                "stored record must be {STORED_SIZE} bytes"
            )));
        }
        let mut r = [0u8; RECORD_SIZE];
        r[..STORED_SIZE].copy_from_slice(stored);
        let mut rec = Record(r);
        rec.update_crc();
        Ok(rec)
    }

    pub fn as_bytes(&self) -> &[u8; RECORD_SIZE] {
        &self.0
    }

    pub fn get(&self, field: &Field) -> u32 {
        let n = (field.shift + field.width).div_ceil(8) as usize;
        let mut word = 0u32;
        for i in 0..n {
            word |= (self.0[field.offset + i] as u32) << (8 * i);
        }
        (word >> field.shift) & ((1u32 << field.width) - 1)
    }

    pub fn set(&mut self, field: &Field, value: u32) -> Result<(), Error> {
        if value >= 1u32 << field.width {
            return Err(Error::Format(format!(
                "{} does not fit {} bits",
                field.name, field.width
            )));
        }
        let n = (field.shift + field.width).div_ceil(8) as usize;
        let mut word = 0u32;
        for i in 0..n {
            word |= (self.0[field.offset + i] as u32) << (8 * i);
        }
        let mask = ((1u32 << field.width) - 1) << field.shift;
        word = (word & !mask) | (value << field.shift);
        for i in 0..n {
            self.0[field.offset + i] = (word >> (8 * i)) as u8;
        }
        Ok(())
    }

    pub fn field(&self, name: &str) -> Option<u32> {
        FIELDS.iter().find(|f| f.name == name).map(|f| self.get(f))
    }

    pub fn version(&self) -> u8 {
        self.0[0]
    }

    pub fn copying_allowed(&self) -> bool {
        self.0[1] & 1 != 0
    }

    pub fn sharing_disabled(&self) -> bool {
        self.0[0x30] & 1 != 0
    }

    pub fn system_id(&self) -> [u8; 8] {
        self.0[4..0xc].try_into().unwrap()
    }

    pub fn mii_id(&self) -> u32 {
        u32::from_be_bytes(self.0[0xc..0x10].try_into().unwrap())
    }

    pub fn creator_mac(&self) -> [u8; 6] {
        self.0[0x10..0x16].try_into().unwrap()
    }

    fn flags(&self) -> u16 {
        u16::from_le_bytes([self.0[0x18], self.0[0x19]])
    }

    pub fn birthday(&self) -> (u32, u32) {
        let f = self.flags() as u32;
        ((f >> 1) & 0xf, (f >> 5) & 0x1f)
    }

    pub fn favourite(&self) -> bool {
        self.flags() & FAVOURITE_BIT != 0
    }

    pub fn name(&self) -> String {
        String::from_utf16_lossy(&utf16(&self.0[0x1a..0x2e]))
    }

    pub fn creator_name(&self) -> String {
        String::from_utf16_lossy(&utf16(&self.0[0x48..0x5c]))
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), Error> {
        let units: Vec<u16> = name.encode_utf16().collect();
        if units.len() > 10 {
            return Err(Error::Format("name longer than 10 UTF-16 units".into()));
        }
        self.0[0x1a..0x2e].fill(0);
        for (i, u) in units.iter().enumerate() {
            self.0[0x1a + 2 * i..0x1c + 2 * i].copy_from_slice(&u.to_le_bytes());
        }
        Ok(())
    }

    pub fn stored_crc(&self) -> u16 {
        u16::from_be_bytes([self.0[0x5e], self.0[0x5f]])
    }

    pub fn crc_valid(&self) -> bool {
        crc16(&self.0[..0x5e]) == self.stored_crc()
    }

    pub fn update_crc(&mut self) {
        let crc = crc16(&self.0[..0x5e]);
        self.0[0x5e..].copy_from_slice(&crc.to_be_bytes());
    }

    /// Returns the record as a sender puts it on the wire: not a favourite, fresh CRC.
    pub fn for_sending(&self) -> Record {
        let mut r = self.clone();
        let flags = r.flags() & !FAVOURITE_BIT;
        r.0[0x18..0x1a].copy_from_slice(&flags.to_le_bytes());
        r.0[0x5c..0x5e].fill(0);
        r.update_crc();
        r
    }

    /// Lists every reason a receiver would refuse this record.
    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        if !self.crc_valid() {
            issues.push(Issue::Crc);
        }
        let id = &self.0[0xc..0x16];
        if id.iter().all(|&b| b == 0)
            || self.0[0xc] & 0x20 != 0
            || (self.0[0xc] & 0x80 == 0 && !self.sharing_disabled())
        {
            issues.push(Issue::Identity);
        }
        for field in FIELDS {
            let value = self.get(field);
            if value < field.min || value > field.max {
                issues.push(Issue::Range {
                    field: field.name,
                    value,
                });
            }
        }
        let (month, day) = self.birthday();
        let birthday_ok = match (month, day) {
            (0, 0) => true,
            (0, _) | (_, 0) => false,
            (m, d) => m <= 12 && d <= MONTH_DAYS[m as usize - 1],
        };
        if !birthday_ok {
            issues.push(Issue::Birthday { month, day });
        }
        let name = utf16(&self.0[0x1a..0x2e]);
        let author = utf16(&self.0[0x48..0x5c]);
        if name.is_empty() {
            issues.push(Issue::EmptyName);
        }
        if name.iter().chain(&author).any(|&u| u == 0x25 || u == 0x5c) {
            issues.push(Issue::NameCharacter);
        }
        issues
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A synthetic record that passes every check.
    pub fn sample() -> Record {
        let mut r = [0u8; RECORD_SIZE];
        r[0] = 3;
        r[1] = 1;
        r[3] = 0x30;
        r[4..0xc].copy_from_slice(&[0x11; 8]);
        r[0xc..0x10].copy_from_slice(&0x9000_1234u32.to_be_bytes());
        r[0x10..0x16].copy_from_slice(&[0x02, 0, 0, 0, 0, 1]);
        let mut rec = Record(r);
        rec.set_name("Test").unwrap();
        rec.set(&FIELDS[20], 10).unwrap();
        rec.0[0x2e] = 64;
        rec.0[0x2f] = 64;
        rec.update_crc();
        rec
    }

    fn with(name: &str, value: u32) -> Record {
        let mut r = sample();
        let field = FIELDS.iter().find(|f| f.name == name).unwrap();
        r.set(field, value).unwrap();
        r.update_crc();
        r
    }

    #[test]
    fn crc_matches_xmodem_check_value() {
        assert_eq!(crc16(b"123456789"), 0x31c3);
    }

    #[test]
    fn sample_is_valid() {
        let r = sample();
        assert_eq!(r.validate(), vec![]);
        assert_eq!(r.name(), "Test");
        assert_eq!(r.mii_id(), 0x9000_1234);
    }

    #[test]
    fn range_boundaries() {
        for (name, ok, bad) in [
            ("hair_style", 131, 132),
            ("eye_type", 59, 60),
            ("height", 128, 129),
            ("eyebrow_y", 3, 2),
        ] {
            assert!(with(name, ok).validate().is_empty(), "{name}={ok}");
            assert_eq!(
                with(name, bad).validate(),
                vec![Issue::Range {
                    field: name,
                    value: bad
                }]
            );
        }
    }

    #[test]
    fn birthday_rules() {
        let set = |m: u32, d: u32| {
            let mut r = sample();
            let flags = (r.flags() as u32 & !(0x1ff << 1)) | (m << 1) | (d << 5);
            r.0[0x18..0x1a].copy_from_slice(&(flags as u16).to_le_bytes());
            r.update_crc();
            r.validate()
        };
        assert!(set(2, 29).is_empty());
        assert!(set(0, 0).is_empty());
        assert_eq!(set(2, 30), vec![Issue::Birthday { month: 2, day: 30 }]);
        assert_eq!(set(0, 5).len(), 1);
        assert_eq!(set(13, 1).len(), 1);
    }

    #[test]
    fn identity_and_names() {
        let mut r = sample();
        r.0[0xc] &= !0x80;
        r.update_crc();
        assert_eq!(r.validate(), vec![Issue::Identity]);
        r.0[0x30] |= 1;
        r.update_crc();
        assert!(r.validate().is_empty());

        let mut r = sample();
        r.set_name("a%b").unwrap();
        r.update_crc();
        assert_eq!(r.validate(), vec![Issue::NameCharacter]);
        r.set_name("").unwrap();
        r.update_crc();
        assert_eq!(r.validate(), vec![Issue::EmptyName]);
    }

    #[test]
    fn sending_clears_favourite_and_fixes_crc() {
        let mut r = sample();
        r.0[0x19] |= 0x40;
        r.0[0x5e] ^= 1;
        assert!(r.favourite() && !r.crc_valid());
        let s = r.for_sending();
        assert!(!s.favourite() && s.crc_valid());
    }

    #[test]
    fn stored_round_trip() {
        let r = sample();
        assert_eq!(Record::from_stored(&r.0[..STORED_SIZE]).unwrap(), r);
    }
}
