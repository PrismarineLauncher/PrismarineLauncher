use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use std::ffi::{CStr, c_char};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[repr(C)]
pub struct PrismarineTimestampResult {
    pub unix_ms_utc: i64,
    pub offset_seconds: i32,
    pub is_valid: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceSummary {
    pub name: String,
    pub path: PathBuf,
    pub modified_unix_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogSummary {
    pub file_name: String,
    pub path: PathBuf,
}

pub fn parse_s3_time(input: &str) -> Option<(i64, i32)> {
    let parsed = DateTime::parse_from_rfc3339(input).ok()?;
    Some((parsed.timestamp_millis(), parsed.offset().local_minus_utc()))
}

pub fn format_s3_time(unix_ms_utc: i64, offset_seconds: i32) -> Option<String> {
    let offset = FixedOffset::east_opt(offset_seconds)?;
    let utc = Utc.timestamp_millis_opt(unix_ms_utc).single()?;
    let local = utc.with_timezone(&offset);
    Some(local.format("%Y-%m-%dT%H:%M:%S%:z").to_string())
}

fn write_c_string(src: &str, out_buffer: *mut c_char, out_buffer_len: usize) -> Result<(), i32> {
    if out_buffer.is_null() || out_buffer_len == 0 {
        return Err(1);
    }

    let bytes = src.as_bytes();
    if bytes.len() + 1 > out_buffer_len {
        return Err(2);
    }

    // SAFETY: pointers are validated above and we copy exactly bytes.len() bytes, then one nul terminator.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_buffer.cast::<u8>(), bytes.len());
        *out_buffer.add(bytes.len()) = 0;
    }
    Ok(())
}

pub fn scan_instances(root: &Path) -> std::io::Result<Vec<InstanceSummary>> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|x| x.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }

        let meta = entry.metadata()?;
        let modified_unix_ms = meta
            .modified()
            .ok()
            .and_then(|x| x.duration_since(UNIX_EPOCH).ok())
            .map(|x| x.as_millis() as i64)
            .unwrap_or(0);

        out.push(InstanceSummary {
            name: name.to_string(),
            path,
            modified_unix_ms,
        });
    }

    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

pub fn create_instance(root: &Path, name: &str) -> std::io::Result<InstanceSummary> {
    let cleaned = name.trim();
    if cleaned.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "instance name is empty",
        ));
    }
    let path = root.join(cleaned);
    fs::create_dir_all(path.join("mods"))?;
    fs::create_dir_all(path.join("logs"))?;
    fs::create_dir_all(path.join("saves"))?;
    fs::write(
        path.join("instance.cfg"),
        b"# PrismarineLauncher instance\n",
    )?;
    let modified_unix_ms = fs::metadata(&path)?
        .modified()
        .ok()
        .and_then(|x| x.duration_since(UNIX_EPOCH).ok())
        .map(|x| x.as_millis() as i64)
        .unwrap_or(0);
    Ok(InstanceSummary {
        name: cleaned.to_string(),
        path,
        modified_unix_ms,
    })
}

pub fn delete_instance(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

pub fn copy_instance(src: &Path, root: &Path, new_name: &str) -> std::io::Result<InstanceSummary> {
    let cleaned = new_name.trim();
    if cleaned.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "new instance name is empty",
        ));
    }
    let dst = root.join(cleaned);
    copy_dir_recursive(src, &dst)?;
    let modified_unix_ms = fs::metadata(&dst)?
        .modified()
        .ok()
        .and_then(|x| x.duration_since(UNIX_EPOCH).ok())
        .map(|x| x.as_millis() as i64)
        .unwrap_or(0);
    Ok(InstanceSummary {
        name: cleaned.to_string(),
        path: dst,
        modified_unix_ms,
    })
}

pub fn rename_instance(path: &Path, new_name: &str) -> std::io::Result<PathBuf> {
    let cleaned = new_name.trim();
    if cleaned.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "new instance name is empty",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "instance has no parent")
    })?;
    let new_path = parent.join(cleaned);
    fs::rename(path, &new_path)?;
    Ok(new_path)
}

pub fn list_mod_files(instance_path: &Path) -> std::io::Result<Vec<String>> {
    let mods_dir = instance_path.join("mods");
    if !mods_dir.exists() {
        return Ok(Vec::new());
    }
    let mut mods = Vec::new();
    for entry in fs::read_dir(mods_dir)? {
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            mods.push(name.to_string());
        }
    }
    mods.sort();
    Ok(mods)
}

pub fn list_logs(instance_path: &Path) -> std::io::Result<Vec<LogSummary>> {
    let logs_dir = instance_path.join("logs");
    if !logs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut logs = Vec::new();
    for entry in fs::read_dir(logs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        logs.push(LogSummary {
            file_name: name.to_string(),
            path,
        });
    }
    logs.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(logs)
}

pub fn read_log_preview(path: &Path, max_chars: usize) -> std::io::Result<String> {
    let text = fs::read_to_string(path)?;
    if text.chars().count() <= max_chars {
        return Ok(text);
    }
    Ok(text.chars().take(max_chars).collect())
}

#[unsafe(no_mangle)]
pub extern "C" fn prismarine_parse_s3_time(
    input: *const c_char,
    out_result: *mut PrismarineTimestampResult,
) -> i32 {
    if input.is_null() || out_result.is_null() {
        return 1;
    }

    // SAFETY: input pointer is checked for null and expected to be valid C string from caller.
    let input = unsafe { CStr::from_ptr(input) };
    let Ok(input) = input.to_str() else {
        return 2;
    };

    let Some((unix_ms_utc, offset_seconds)) = parse_s3_time(input) else {
        // SAFETY: out_result is non-null and points to writable memory provided by caller.
        unsafe {
            *out_result = PrismarineTimestampResult {
                unix_ms_utc: 0,
                offset_seconds: 0,
                is_valid: 0,
            };
        }
        return 0;
    };

    // SAFETY: out_result is non-null and points to writable memory provided by caller.
    unsafe {
        *out_result = PrismarineTimestampResult {
            unix_ms_utc,
            offset_seconds,
            is_valid: 1,
        };
    }

    0
}

#[unsafe(no_mangle)]
pub extern "C" fn prismarine_format_s3_time(
    unix_ms_utc: i64,
    offset_seconds: i32,
    out_buffer: *mut c_char,
    out_buffer_len: usize,
) -> i32 {
    let Some(formatted) = format_s3_time(unix_ms_utc, offset_seconds) else {
        return 1;
    };

    write_c_string(&formatted, out_buffer, out_buffer_len).map_or_else(|err| err, |_| 0)
}

#[cfg(test)]
mod tests {
    use super::{
        copy_instance, create_instance, delete_instance, format_s3_time, list_logs, list_mod_files,
        parse_s3_time, read_log_preview, rename_instance, scan_instances,
    };
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn s3_parse_and_format_round_trip() {
        let cases = [
            "2016-02-29T13:49:54+01:00",
            "2016-02-26T15:21:11+00:01",
            "2016-02-24T15:52:36+01:13",
            "2016-02-18T17:41:00+00:00",
            "2016-02-17T15:23:19+00:00",
            "2016-02-16T15:22:39+09:22",
            "2016-02-10T15:06:41+00:00",
            "2016-02-04T15:28:02-05:33",
        ];

        for case in cases {
            let (ms, offset) = parse_s3_time(case).expect("parse");
            let serialized = format_s3_time(ms, offset).expect("format");
            assert_eq!(serialized, case);
        }
    }

    #[test]
    fn scan_instances_lists_directories() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let base = PathBuf::from(format!(
            "/tmp/prismarine_launcher_test_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("Alpha")).expect("create alpha");
        fs::create_dir_all(base.join("Beta")).expect("create beta");
        fs::write(base.join("README.txt"), b"x").expect("create file");

        let items = scan_instances(&base).expect("scan");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "Alpha");
        assert_eq!(items[1].name, "Beta");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn instance_filesystem_operations_work() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/tmp/prismarine_launcher_ops_test_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create root");

        let created = create_instance(&root, "Alpha").expect("create instance");
        assert_eq!(created.name, "Alpha");
        fs::write(created.path.join("mods").join("example.jar"), b"jar").expect("write mod");
        fs::write(created.path.join("logs").join("latest.log"), b"hello log").expect("write log");

        let mods = list_mod_files(&created.path).expect("list mods");
        assert_eq!(mods, vec!["example.jar".to_string()]);

        let logs = list_logs(&created.path).expect("list logs");
        assert_eq!(logs.len(), 1);
        let preview = read_log_preview(&logs[0].path, 100).expect("preview");
        assert!(preview.contains("hello log"));

        let copied = copy_instance(&created.path, &root, "Alpha Copy").expect("copy");
        assert_eq!(copied.name, "Alpha Copy");

        let renamed = rename_instance(&copied.path, "Alpha Renamed").expect("rename");
        assert!(renamed.ends_with("Alpha Renamed"));

        delete_instance(&created.path).expect("delete created");
        delete_instance(&renamed).expect("delete renamed");
        let remaining = scan_instances(&root).expect("scan remaining");
        assert!(remaining.is_empty());

        let _ = fs::remove_dir_all(&root);
    }
}
