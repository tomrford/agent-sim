use crate::config::recipe::{DeviceDef, EnvInstance, FileConfig, FlashBlockDef, FlashFileBlockDef};
use crate::load::{
    FlashParseError, LoadSpec, ResolvedFlashRegion, encode_inline_bool, encode_inline_f32,
    encode_inline_i32, encode_inline_u32, merge_regions, parse_address, resolve_flash_file,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoadResolveError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Flash(#[from] FlashParseError),
}

pub fn resolve_standalone_load_spec(
    file_config: &FileConfig,
    config_base_dir: Option<&Path>,
    cli_libpath: Option<&str>,
    cli_flash: &[String],
    env_tag: Option<String>,
) -> Result<LoadSpec, LoadResolveError> {
    let defaults = file_config
        .defaults
        .as_ref()
        .and_then(|defaults| defaults.load.as_ref());
    let raw_libpath = cli_libpath
        .or_else(|| defaults.and_then(|defaults| defaults.lib.as_deref()))
        .ok_or_else(|| {
            LoadResolveError::Message(
                "load requires a library path or [defaults.load].lib in config".to_string(),
            )
        })?;
    let libpath = canonicalize_runtime_path(raw_libpath, config_base_dir, "shared library")
        .map_err(LoadResolveError::Message)?;

    let flash = if cli_flash.is_empty() {
        resolve_flash_blocks(
            defaults
                .map(|defaults| defaults.flash.as_slice())
                .unwrap_or_default(),
            config_base_dir,
        )?
    } else {
        resolve_cli_flash_entries(cli_flash, config_base_dir)?
    };

    Ok(LoadSpec {
        libpath,
        env_tag,
        flash,
    })
}

pub fn resolve_env_load_specs(
    env_name: &str,
    env_members: &[EnvInstance],
    devices: &BTreeMap<String, DeviceDef>,
    config_base_dir: Option<&Path>,
) -> Result<Vec<(String, LoadSpec)>, LoadResolveError> {
    let mut specs = Vec::with_capacity(env_members.len());
    for member in env_members {
        specs.push((
            member.name.clone(),
            resolve_env_member_load_spec(
                member,
                devices,
                config_base_dir,
                Some(env_name.to_string()),
            )?,
        ));
    }
    Ok(specs)
}

pub fn resolve_env_member_load_spec(
    member: &EnvInstance,
    devices: &BTreeMap<String, DeviceDef>,
    config_base_dir: Option<&Path>,
    env_tag: Option<String>,
) -> Result<LoadSpec, LoadResolveError> {
    let spec = match (member.lib.as_deref(), member.device.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(LoadResolveError::Message(format!(
                "env member '{}' cannot define both 'lib' and 'device'",
                member.name
            )));
        }
        (None, None) => {
            return Err(LoadResolveError::Message(format!(
                "env member '{}' must define exactly one of 'lib' or 'device'",
                member.name
            )));
        }
        (Some(lib), None) => LoadSpec {
            libpath: canonicalize_runtime_path(lib, config_base_dir, "shared library")
                .map_err(LoadResolveError::Message)?,
            env_tag,
            flash: Vec::new(),
        },
        (None, Some(device_name)) => {
            let device = devices.get(device_name).ok_or_else(|| {
                LoadResolveError::Message(format!(
                    "env member '{}' references missing device '{}'",
                    member.name, device_name
                ))
            })?;
            LoadSpec {
                libpath: canonicalize_runtime_path(&device.lib, config_base_dir, "shared library")
                    .map_err(LoadResolveError::Message)?,
                env_tag,
                flash: resolve_flash_blocks(&device.flash, config_base_dir)?,
            }
        }
    };
    Ok(spec)
}

pub fn resolve_flash_blocks(
    blocks: &[FlashBlockDef],
    config_base_dir: Option<&Path>,
) -> Result<Vec<ResolvedFlashRegion>, LoadResolveError> {
    let mut regions = Vec::new();
    for block in blocks {
        match block {
            FlashBlockDef::File(file) => {
                regions.extend(resolve_flash_file_block(file, config_base_dir)?);
            }
            FlashBlockDef::InlineU32 { u32, addr } => regions.push(ResolvedFlashRegion {
                base_addr: parse_address(addr)?,
                data: encode_inline_u32(*u32),
            }),
            FlashBlockDef::InlineI32 { i32, addr } => regions.push(ResolvedFlashRegion {
                base_addr: parse_address(addr)?,
                data: encode_inline_i32(*i32),
            }),
            FlashBlockDef::InlineF32 { f32, addr } => regions.push(ResolvedFlashRegion {
                base_addr: parse_address(addr)?,
                data: encode_inline_f32(*f32),
            }),
            FlashBlockDef::InlineBool { bool, addr } => regions.push(ResolvedFlashRegion {
                base_addr: parse_address(addr)?,
                data: encode_inline_bool(*bool),
            }),
        }
    }
    Ok(merge_regions(&regions)?)
}

pub fn resolve_cli_flash_entries(
    entries: &[String],
    config_base_dir: Option<&Path>,
) -> Result<Vec<ResolvedFlashRegion>, LoadResolveError> {
    let mut regions = Vec::new();
    for entry in entries {
        let (path_raw, explicit_base) = parse_cli_flash_entry(entry)?;
        let path = resolve_runtime_path(path_raw, config_base_dir);
        let path =
            canonicalize_existing_path(&path, "flash file").map_err(LoadResolveError::Message)?;
        let base_addr = explicit_base.map(parse_address).transpose()?;
        regions.extend(resolve_flash_file(&path, None, base_addr)?);
    }
    Ok(merge_regions(&regions)?)
}

pub fn canonicalize_runtime_path(
    raw_path: &str,
    config_base_dir: Option<&Path>,
    kind: &str,
) -> Result<String, String> {
    let candidate = resolve_runtime_path(raw_path, config_base_dir);
    let canonical = canonicalize_runtime_candidate(&candidate, kind)?;
    Ok(canonical.to_string_lossy().into_owned())
}

fn resolve_flash_file_block(
    block: &FlashFileBlockDef,
    config_base_dir: Option<&Path>,
) -> Result<Vec<ResolvedFlashRegion>, LoadResolveError> {
    let path = resolve_runtime_path(&block.file, config_base_dir);
    let path =
        canonicalize_existing_path(&path, "flash file").map_err(LoadResolveError::Message)?;
    let base_addr = block.base.as_deref().map(parse_address).transpose()?;
    Ok(resolve_flash_file(
        &path,
        block.format.as_deref(),
        base_addr,
    )?)
}

fn resolve_runtime_path(raw_path: &str, config_base_dir: Option<&Path>) -> PathBuf {
    let path = Path::new(raw_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(base_dir) = config_base_dir {
        base_dir.join(path)
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn canonicalize_existing_path(path: &Path, kind: &str) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|err| {
        format!(
            "failed to resolve {kind} path '{}' to an absolute path: {err}",
            path.display()
        )
    })
}

fn canonicalize_runtime_candidate(path: &Path, kind: &str) -> Result<PathBuf, String> {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return Ok(canonical);
    }
    if kind == "shared library" {
        for fallback in shared_library_fallbacks(path) {
            if let Ok(canonical) = std::fs::canonicalize(&fallback) {
                return Ok(canonical);
            }
        }
    }
    canonicalize_existing_path(path, kind)
}

fn shared_library_fallbacks(path: &Path) -> Vec<PathBuf> {
    let ext = native_shared_library_extension();
    let mut out = Vec::new();
    if path.extension().is_some() && path.extension().and_then(|value| value.to_str()) != Some(ext)
    {
        out.push(path.with_extension(ext));
    }
    if path.extension().is_none() {
        out.push(PathBuf::from(format!("{}.{}", path.display(), ext)));
    }
    out
}

fn native_shared_library_extension() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "dll"
    }
    #[cfg(target_os = "macos")]
    {
        "dylib"
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        "so"
    }
}

fn parse_cli_flash_entry(raw: &str) -> Result<(&str, Option<&str>), LoadResolveError> {
    let Some((path, maybe_base)) = raw.rsplit_once(':') else {
        return Ok((raw, None));
    };
    if maybe_base.starts_with("0x")
        || maybe_base.starts_with("0X")
        || (!maybe_base.is_empty() && maybe_base.chars().all(|ch| ch.is_ascii_digit()))
    {
        Ok((path, Some(maybe_base)))
    } else {
        Ok((raw, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::recipe::{DefaultsConfig, FlashFileBlockDef, LoadDefaults};

    #[test]
    fn resolve_env_member_rejects_missing_device_reference() {
        let member = EnvInstance {
            name: "ecu-a".to_string(),
            lib: None,
            device: Some("missing".to_string()),
        };
        let err = resolve_env_member_load_spec(&member, &BTreeMap::new(), None, None)
            .expect_err("missing device should fail");
        assert!(err.to_string().contains("missing device"));
    }

    #[test]
    fn resolve_env_member_rejects_lib_and_device_together() {
        let member = EnvInstance {
            name: "ecu-a".to_string(),
            lib: Some("./libecu.so".to_string()),
            device: Some("ecu".to_string()),
        };
        let err = resolve_env_member_load_spec(&member, &BTreeMap::new(), None, None)
            .expect_err("lib+device should fail");
        assert!(err.to_string().contains("both 'lib' and 'device'"));
    }

    #[test]
    fn resolve_flash_blocks_merges_overlaps_in_order() {
        let blocks = vec![
            FlashBlockDef::InlineU32 {
                u32: 0x1122_3344,
                addr: "0x1000".to_string(),
            },
            FlashBlockDef::InlineBool {
                bool: true,
                addr: "0x1001".to_string(),
            },
        ];
        let regions = resolve_flash_blocks(&blocks, None).expect("inline blocks should resolve");
        assert_eq!(
            regions,
            vec![ResolvedFlashRegion {
                base_addr: 0x1000,
                data: vec![0x44, 0x01, 0x22, 0x11],
            }]
        );
    }

    #[test]
    fn resolve_standalone_load_spec_uses_defaults_when_cli_path_missing() {
        let temp = tempfile::tempdir().expect("tempdir should be creatable");
        let lib = temp.path().join("libfoo.so");
        std::fs::write(&lib, b"fake").expect("lib placeholder should be writable");
        let file_config = FileConfig {
            defaults: Some(DefaultsConfig {
                json: None,
                speed: None,
                load: Some(LoadDefaults {
                    lib: Some(lib.to_string_lossy().into_owned()),
                    flash: Vec::new(),
                }),
            }),
            ..FileConfig::default()
        };

        let spec = resolve_standalone_load_spec(&file_config, None, None, &[], None)
            .expect("defaults should resolve");
        assert_eq!(
            spec.libpath,
            std::fs::canonicalize(&lib)
                .expect("lib should canonicalize")
                .to_string_lossy()
        );
    }

    #[test]
    fn parse_cli_flash_entry_handles_optional_binary_base() {
        assert_eq!(
            parse_cli_flash_entry("./cal.hex").expect("hex entry should parse"),
            ("./cal.hex", None)
        );
        assert_eq!(
            parse_cli_flash_entry("./blob.bin:0x08040000").expect("bin entry should parse"),
            ("./blob.bin", Some("0x08040000"))
        );
        assert_eq!(
            parse_cli_flash_entry("./blob.bin:").expect("trailing colon should be treated as path"),
            ("./blob.bin:", None)
        );
    }

    #[test]
    fn resolve_flash_file_block_requires_base_for_binary() {
        let temp = tempfile::tempdir().expect("tempdir should be creatable");
        let bin = temp.path().join("blob.bin");
        std::fs::write(&bin, [1_u8, 2, 3]).expect("binary blob should be writable");
        let err = resolve_flash_file_block(
            &FlashFileBlockDef {
                file: bin.to_string_lossy().into_owned(),
                format: Some("bin".to_string()),
                base: None,
            },
            None,
        )
        .expect_err("binary flash block without base must fail");
        assert!(matches!(
            err,
            LoadResolveError::Flash(FlashParseError::MissingBinaryBase)
        ));
    }

    #[test]
    fn canonicalize_runtime_path_resolves_extensionless_shared_library() {
        let temp = tempfile::tempdir().expect("tempdir should be creatable");
        let lib = temp
            .path()
            .join(format!("libdemo.{}", native_shared_library_extension()));
        std::fs::write(&lib, b"fake").expect("shared library placeholder should be writable");

        let resolved = canonicalize_runtime_path(
            &temp.path().join("libdemo").to_string_lossy(),
            None,
            "shared library",
        )
        .expect("extensionless shared library path should resolve");

        assert_eq!(
            resolved,
            std::fs::canonicalize(&lib)
                .expect("shared library should canonicalize")
                .to_string_lossy()
        );
    }

    #[test]
    fn canonicalize_runtime_path_falls_back_to_native_shared_library_suffix() {
        let temp = tempfile::tempdir().expect("tempdir should be creatable");
        let lib = temp
            .path()
            .join(format!("libdemo.{}", native_shared_library_extension()));
        std::fs::write(&lib, b"fake").expect("shared library placeholder should be writable");

        let requested = temp
            .path()
            .join(format!("libdemo.{}", non_native_shared_library_extension()));
        let resolved =
            canonicalize_runtime_path(&requested.to_string_lossy(), None, "shared library")
                .expect("mismatched shared library suffix should resolve to native artifact");

        assert_eq!(
            resolved,
            std::fs::canonicalize(&lib)
                .expect("shared library should canonicalize")
                .to_string_lossy()
        );
    }

    fn non_native_shared_library_extension() -> &'static str {
        ["so", "dylib", "dll"]
            .into_iter()
            .find(|ext| *ext != native_shared_library_extension())
            .expect("a non-native shared library extension should exist")
    }
}
