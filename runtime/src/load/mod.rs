pub mod resolve;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoadSpec {
    pub libpath: String,
    pub env_tag: Option<String>,
    #[serde(default)]
    pub flash: Vec<ResolvedFlashRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedFlashRegion {
    pub base_addr: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashFormat {
    IntelHex,
    Srec,
    Binary,
}

impl FlashFormat {
    pub fn parse(raw: &str) -> Result<Self, FlashParseError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "hex" | "ihex" | "intel-hex" | "intel_hex" => Ok(Self::IntelHex),
            "srec" | "s-record" | "s_record" | "s19" | "s28" | "s37" | "mot" => Ok(Self::Srec),
            "bin" | "binary" => Ok(Self::Binary),
            other => Err(FlashParseError::UnsupportedFormat(other.to_string())),
        }
    }

    pub fn infer(path: &Path, explicit: Option<&str>) -> Result<Self, FlashParseError> {
        if let Some(raw) = explicit {
            return Self::parse(raw);
        }

        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .ok_or_else(|| FlashParseError::UnsupportedFormat(path.display().to_string()))?;
        Self::parse(ext)
    }
}

#[derive(Debug, Error)]
pub enum FlashParseError {
    #[error("unsupported flash format '{0}'")]
    UnsupportedFormat(String),
    #[error("invalid flash address '{0}'")]
    InvalidAddress(String),
    #[error("raw binary flash input requires an explicit base address")]
    MissingBinaryBase,
    #[error("flash input exceeds 32-bit address space at 0x{base_addr:08X} (+{len} bytes)")]
    AddressOverflow { base_addr: u32, len: usize },
    #[error("invalid Intel HEX line {line}: {message}")]
    InvalidIntelHex { line: usize, message: String },
    #[error("invalid S-record line {line}: {message}")]
    InvalidSrec { line: usize, message: String },
    #[error("failed to read flash file '{path}': {message}")]
    FileRead { path: String, message: String },
    #[error("load spec '{path}': {message}")]
    LoadSpec { path: String, message: String },
}

pub fn parse_address(raw: &str) -> Result<u32, FlashParseError> {
    let trimmed = raw.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).map_err(|_| FlashParseError::InvalidAddress(raw.to_string()))
    } else {
        trimmed
            .parse::<u32>()
            .map_err(|_| FlashParseError::InvalidAddress(raw.to_string()))
    }
}

pub fn parse_raw_binary(
    bytes: &[u8],
    base_addr: u32,
) -> Result<ResolvedFlashRegion, FlashParseError> {
    ensure_address_range(base_addr, bytes.len())?;
    Ok(ResolvedFlashRegion {
        base_addr,
        data: bytes.to_vec(),
    })
}

pub fn parse_intel_hex(content: &str) -> Result<Vec<ResolvedFlashRegion>, FlashParseError> {
    let mut upper_addr = 0_u32;
    let mut memory = FlashMemory::default();

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let payload = line
            .strip_prefix(':')
            .ok_or_else(|| FlashParseError::InvalidIntelHex {
                line: line_no,
                message: "record must start with ':'".to_string(),
            })?;
        if payload.len() < 10 || payload.len() % 2 != 0 {
            return Err(FlashParseError::InvalidIntelHex {
                line: line_no,
                message: "record has invalid hex length".to_string(),
            });
        }

        let bytes =
            decode_hex_bytes(payload).map_err(|message| FlashParseError::InvalidIntelHex {
                line: line_no,
                message,
            })?;
        let byte_count = usize::from(bytes[0]);
        if bytes.len() != byte_count + 5 {
            return Err(FlashParseError::InvalidIntelHex {
                line: line_no,
                message: format!(
                    "record length mismatch: byte_count={} actual_data_bytes={}",
                    byte_count,
                    bytes.len().saturating_sub(5)
                ),
            });
        }

        let checksum = bytes
            .iter()
            .fold(0_u8, |acc, value| acc.wrapping_add(*value));
        if checksum != 0 {
            return Err(FlashParseError::InvalidIntelHex {
                line: line_no,
                message: "checksum mismatch".to_string(),
            });
        }

        let address = u16::from(bytes[1]) << 8 | u16::from(bytes[2]);
        let record_type = bytes[3];
        let data = &bytes[4..4 + byte_count];
        match record_type {
            0x00 => {
                let base_addr = upper_addr.checked_add(u32::from(address)).ok_or(
                    FlashParseError::AddressOverflow {
                        base_addr: upper_addr,
                        len: usize::from(address),
                    },
                )?;
                memory.write(base_addr, data)?;
            }
            0x01 => break,
            0x02 => {
                if data.len() != 2 {
                    return Err(FlashParseError::InvalidIntelHex {
                        line: line_no,
                        message: "extended segment address record must contain 2 data bytes"
                            .to_string(),
                    });
                }
                upper_addr = ((u32::from(data[0]) << 8) | u32::from(data[1])) << 4;
            }
            0x04 => {
                if data.len() != 2 {
                    return Err(FlashParseError::InvalidIntelHex {
                        line: line_no,
                        message: "extended linear address record must contain 2 data bytes"
                            .to_string(),
                    });
                }
                upper_addr = ((u32::from(data[0]) << 8) | u32::from(data[1])) << 16;
            }
            0x03 | 0x05 => {}
            other => {
                return Err(FlashParseError::InvalidIntelHex {
                    line: line_no,
                    message: format!("unsupported record type 0x{other:02X}"),
                });
            }
        }
    }

    Ok(memory.into_regions())
}

pub fn parse_srec(content: &str) -> Result<Vec<ResolvedFlashRegion>, FlashParseError> {
    let mut memory = FlashMemory::default();

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if !line.starts_with('S') || line.len() < 4 {
            return Err(FlashParseError::InvalidSrec {
                line: line_no,
                message: "record must start with 'S' and include a type/length".to_string(),
            });
        }

        let record_type = line.as_bytes()[1] as char;
        let rest = &line[2..];
        if rest.len() % 2 != 0 {
            return Err(FlashParseError::InvalidSrec {
                line: line_no,
                message: "record hex payload must contain an even number of digits".to_string(),
            });
        }
        let bytes = decode_hex_bytes(rest).map_err(|message| FlashParseError::InvalidSrec {
            line: line_no,
            message,
        })?;
        if bytes.is_empty() {
            return Err(FlashParseError::InvalidSrec {
                line: line_no,
                message: "record is missing the byte-count field".to_string(),
            });
        }

        let declared_count = usize::from(bytes[0]);
        if declared_count != bytes.len().saturating_sub(1) {
            return Err(FlashParseError::InvalidSrec {
                line: line_no,
                message: format!(
                    "record length mismatch: byte_count={} actual={}",
                    declared_count,
                    bytes.len().saturating_sub(1)
                ),
            });
        }

        let checksum = bytes
            .iter()
            .fold(0_u8, |acc, value| acc.wrapping_add(*value));
        if checksum != 0xFF {
            return Err(FlashParseError::InvalidSrec {
                line: line_no,
                message: "checksum mismatch".to_string(),
            });
        }

        let addr_len = match record_type {
            '0' | '1' | '5' | '9' => 2,
            '2' | '6' | '8' => 3,
            '3' | '7' => 4,
            other => {
                return Err(FlashParseError::InvalidSrec {
                    line: line_no,
                    message: format!("unsupported record type 'S{other}'"),
                });
            }
        };
        if bytes.len() < addr_len + 2 {
            return Err(FlashParseError::InvalidSrec {
                line: line_no,
                message: "record is too short for its address size".to_string(),
            });
        }

        if matches!(record_type, '1' | '2' | '3') {
            let addr = bytes[1..1 + addr_len]
                .iter()
                .fold(0_u32, |acc, value| (acc << 8) | u32::from(*value));
            let data = &bytes[1 + addr_len..bytes.len() - 1];
            memory.write(addr, data)?;
        }
    }

    Ok(memory.into_regions())
}

pub fn resolve_flash_file(
    path: &Path,
    format: Option<&str>,
    base_addr: Option<u32>,
) -> Result<Vec<ResolvedFlashRegion>, FlashParseError> {
    let flash_format = FlashFormat::infer(path, format)?;
    match flash_format {
        FlashFormat::IntelHex => {
            let content =
                std::fs::read_to_string(path).map_err(|err| FlashParseError::FileRead {
                    path: path.display().to_string(),
                    message: err.to_string(),
                })?;
            parse_intel_hex(&content)
        }
        FlashFormat::Srec => {
            let content =
                std::fs::read_to_string(path).map_err(|err| FlashParseError::FileRead {
                    path: path.display().to_string(),
                    message: err.to_string(),
                })?;
            parse_srec(&content)
        }
        FlashFormat::Binary => {
            let bytes = std::fs::read(path).map_err(|err| FlashParseError::FileRead {
                path: path.display().to_string(),
                message: err.to_string(),
            })?;
            let region =
                parse_raw_binary(&bytes, base_addr.ok_or(FlashParseError::MissingBinaryBase)?)?;
            Ok(vec![region])
        }
    }
}

pub fn merge_regions(
    regions: &[ResolvedFlashRegion],
) -> Result<Vec<ResolvedFlashRegion>, FlashParseError> {
    let mut memory = FlashMemory::default();
    for region in regions {
        memory.write(region.base_addr, &region.data)?;
    }
    Ok(memory.into_regions())
}

pub fn read_load_spec(path: &Path) -> Result<LoadSpec, FlashParseError> {
    let content = std::fs::read_to_string(path).map_err(|err| FlashParseError::LoadSpec {
        path: path.display().to_string(),
        message: err.to_string(),
    })?;
    serde_json::from_str(&content).map_err(|err| FlashParseError::LoadSpec {
        path: path.display().to_string(),
        message: format!("invalid load spec json: {err}"),
    })
}

pub fn write_load_spec(path: &Path, spec: &LoadSpec) -> Result<(), FlashParseError> {
    let content = serde_json::to_string(spec).map_err(|err| FlashParseError::LoadSpec {
        path: path.display().to_string(),
        message: format!("failed to serialize load spec: {err}"),
    })?;
    std::fs::write(path, content).map_err(|err| FlashParseError::LoadSpec {
        path: path.display().to_string(),
        message: err.to_string(),
    })
}

pub fn encode_inline_u32(value: u32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn encode_inline_i32(value: i32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn encode_inline_f32(value: f32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

pub fn encode_inline_bool(value: bool) -> Vec<u8> {
    vec![u8::from(value)]
}

#[derive(Debug, Default)]
struct FlashMemory {
    bytes: BTreeMap<u32, u8>,
}

impl FlashMemory {
    fn write(&mut self, base_addr: u32, data: &[u8]) -> Result<(), FlashParseError> {
        ensure_address_range(base_addr, data.len())?;
        for (offset, value) in data.iter().enumerate() {
            self.bytes.insert(base_addr + offset as u32, *value);
        }
        Ok(())
    }

    fn into_regions(self) -> Vec<ResolvedFlashRegion> {
        let mut regions = Vec::new();
        let mut current_base = None;
        let mut previous_addr = 0_u32;
        let mut current = Vec::new();

        for (addr, value) in self.bytes {
            match current_base {
                None => {
                    current_base = Some(addr);
                    previous_addr = addr;
                    current.push(value);
                }
                Some(_) if addr == previous_addr.saturating_add(1) => {
                    previous_addr = addr;
                    current.push(value);
                }
                Some(base) => {
                    regions.push(ResolvedFlashRegion {
                        base_addr: base,
                        data: std::mem::take(&mut current),
                    });
                    current_base = Some(addr);
                    previous_addr = addr;
                    current.push(value);
                }
            }
        }

        if let Some(base_addr) = current_base {
            regions.push(ResolvedFlashRegion {
                base_addr,
                data: current,
            });
        }
        regions
    }
}

fn ensure_address_range(base_addr: u32, len: usize) -> Result<(), FlashParseError> {
    if len == 0 {
        return Ok(());
    }
    let last_addr = u64::from(base_addr) + len as u64 - 1;
    if last_addr > u64::from(u32::MAX) {
        return Err(FlashParseError::AddressOverflow { base_addr, len });
    }
    Ok(())
}

fn decode_hex_bytes(raw: &str) -> Result<Vec<u8>, String> {
    if !raw.len().is_multiple_of(2) {
        return Err("hex payload must contain an even number of digits".to_string());
    }
    let mut out = Vec::with_capacity(raw.len() / 2);
    let mut idx = 0;
    while idx < raw.len() {
        let pair = &raw[idx..idx + 2];
        let value =
            u8::from_str_radix(pair, 16).map_err(|_| format!("invalid hex byte '{pair}'"))?;
        out.push(value);
        idx += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_address_accepts_hex_and_decimal() {
        assert_eq!(
            parse_address("0x08000000").expect("hex address should parse"),
            0x0800_0000
        );
        assert_eq!(
            parse_address("4096").expect("decimal address should parse"),
            4096
        );
    }

    #[test]
    fn parse_intel_hex_merges_records_and_validates_checksum() {
        let content = concat!(
            ":020000040800F2\n",
            ":0400000001020304F2\n",
            ":00000001FF\n"
        );
        let regions = parse_intel_hex(content).expect("valid ihex should parse");
        assert_eq!(
            regions,
            vec![ResolvedFlashRegion {
                base_addr: 0x0800_0000,
                data: vec![1, 2, 3, 4],
            }]
        );

        let err = parse_intel_hex(":0400000001020304F3\n").expect_err("bad checksum must fail");
        assert!(matches!(err, FlashParseError::InvalidIntelHex { .. }));
    }

    #[test]
    fn parse_srec_reads_data_records_and_validates_checksum() {
        let content = concat!("S00600004844521B\n", "S107123401020304A8\n", "S9030000FC\n");
        let regions = parse_srec(content).expect("valid srec should parse");
        assert_eq!(
            regions,
            vec![ResolvedFlashRegion {
                base_addr: 0x1234,
                data: vec![1, 2, 3, 4],
            }]
        );

        let err = parse_srec("S107123401020304A9\n").expect_err("bad checksum must fail");
        assert!(matches!(err, FlashParseError::InvalidSrec { .. }));
    }

    #[test]
    fn flash_memory_last_write_wins_and_regions_compact() {
        let mut memory = FlashMemory::default();
        memory
            .write(0x1000, &[1, 2, 3])
            .expect("first write should succeed");
        memory
            .write(0x1001, &[9])
            .expect("overlapping write should succeed");
        memory
            .write(0x2000, &[7])
            .expect("disjoint write should succeed");
        assert_eq!(
            memory.into_regions(),
            vec![
                ResolvedFlashRegion {
                    base_addr: 0x1000,
                    data: vec![1, 9, 3],
                },
                ResolvedFlashRegion {
                    base_addr: 0x2000,
                    data: vec![7],
                },
            ]
        );
    }

    #[test]
    fn parse_raw_binary_requires_32bit_address_space() {
        let region = parse_raw_binary(&[0xAA, 0xBB], 0x0800_0000).expect("binary should parse");
        assert_eq!(region.base_addr, 0x0800_0000);
        assert_eq!(region.data, vec![0xAA, 0xBB]);

        let err = parse_raw_binary(&[0; 2], u32::MAX).expect_err("overflow must fail");
        assert!(matches!(err, FlashParseError::AddressOverflow { .. }));
    }

    #[test]
    fn inline_values_encode_little_endian() {
        assert_eq!(encode_inline_u32(0x1234_5678), vec![0x78, 0x56, 0x34, 0x12]);
        assert_eq!(encode_inline_i32(-2), (-2_i32).to_le_bytes().to_vec());
        assert_eq!(encode_inline_f32(3.5), 3.5_f32.to_le_bytes().to_vec());
        assert_eq!(encode_inline_bool(true), vec![1]);
        assert_eq!(encode_inline_bool(false), vec![0]);
    }

    #[test]
    fn read_load_spec_reports_load_spec_context() {
        let temp = tempfile::NamedTempFile::new().expect("temp file should be creatable");
        std::fs::write(temp.path(), "{ not-json }").expect("temp file should be writable");

        let err = read_load_spec(temp.path()).expect_err("invalid json must fail");

        assert!(matches!(err, FlashParseError::LoadSpec { .. }));
        let message = err.to_string();
        assert!(message.contains("load spec"), "unexpected error: {message}");
        assert!(
            !message.contains("flash file"),
            "error should not refer to flash files: {message}"
        );
    }

    #[test]
    fn write_load_spec_reports_load_spec_context() {
        let temp = tempfile::tempdir().expect("temp dir should be creatable");
        let missing_parent = temp.path().join("missing").join("spec.json");
        let spec = LoadSpec {
            libpath: "libsim.so".to_string(),
            env_tag: Some("demo".to_string()),
            flash: Vec::new(),
        };

        let err = write_load_spec(&missing_parent, &spec).expect_err("missing parent must fail");

        assert!(matches!(err, FlashParseError::LoadSpec { .. }));
        let message = err.to_string();
        assert!(message.contains("load spec"), "unexpected error: {message}");
        assert!(
            !message.contains("flash file"),
            "error should not refer to flash files: {message}"
        );
    }
}
