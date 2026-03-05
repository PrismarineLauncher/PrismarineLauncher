use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashSet;
use std::ffi::{CStr, c_char};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use std::time::UNIX_EPOCH;
use zip::ZipArchive;

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LaunchProfile {
    pub java_path: String,
    pub jvm_args: Vec<String>,
    pub main_class: String,
    pub classpath: Vec<String>,
    pub game_args: Vec<String>,
    pub working_dir: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrismInstanceConfig {
    pub icon_key: Option<String>,
    pub group: Option<String>,
    pub intended_version: Option<String>,
    pub override_java_location: bool,
    pub java_path: Option<String>,
    pub override_java_args: bool,
    pub java_args: Option<String>,
    pub override_memory: bool,
    pub min_mem_alloc: Option<u32>,
    pub max_mem_alloc: Option<u32>,
    pub perm_gen: Option<u32>,
    pub override_commands: bool,
    pub pre_launch_command: Option<String>,
    pub post_exit_command: Option<String>,
    pub wrapper_command: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountValidation {
    pub username: String,
    pub uuid: String,
    pub has_minecraft_license: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MicrosoftDeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LicensedMicrosoftAccount {
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub has_minecraft_license: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModrinthSearchHit {
    pub title: String,
    pub project_id: String,
    pub slug: String,
    pub description: String,
    pub author: String,
    pub icon_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModrinthDownloadFile {
    pub url: String,
    pub filename: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModrinthProjectDetails {
    pub title: String,
    pub website_url: String,
    pub summary: String,
    pub body_markdown: String,
    pub issues_url: String,
    pub source_url: String,
    pub wiki_url: String,
    pub discord_url: String,
    pub donate_links: Vec<(String, String)>,
    pub icon_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CurseForgeSearchHit {
    pub mod_id: i64,
    pub title: String,
    pub slug: String,
    pub summary: String,
    pub website_url: String,
    pub author: String,
    pub icon_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CurseForgeProjectDetails {
    pub title: String,
    pub summary: String,
    pub website_url: String,
    pub issues_url: String,
    pub source_url: String,
    pub wiki_url: String,
    pub icon_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeDownloadProgress {
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub message: String,
}

pub fn default_launch_profile(instance_path: &Path) -> LaunchProfile {
    let game_dir = if instance_path.join("minecraft").is_dir() {
        instance_path.join("minecraft")
    } else {
        instance_path.to_path_buf()
    };
    LaunchProfile {
        java_path: "java".to_string(),
        jvm_args: vec!["-Xms1G".to_string(), "-Xmx2G".to_string()],
        main_class: "net.minecraft.client.main.Main".to_string(),
        classpath: Vec::new(),
        game_args: vec![
            "--gameDir".to_string(),
            game_dir.display().to_string(),
            "--version".to_string(),
            "Prismarine".to_string(),
        ],
        working_dir: instance_path.display().to_string(),
    }
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
    fs::create_dir_all(path.join("minecraft").join("mods"))?;
    fs::create_dir_all(path.join("minecraft").join("resourcepacks"))?;
    fs::create_dir_all(path.join("minecraft").join("shaderpacks"))?;
    fs::create_dir_all(path.join("minecraft").join("saves"))?;
    fs::create_dir_all(path.join("minecraft").join("config"))?;
    fs::create_dir_all(path.join("mrpack"))?;
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
    let mut mods = Vec::new();
    for mods_dir in [
        instance_path.join("mods"),
        instance_path.join(".minecraft/mods"),
        instance_path.join("minecraft/mods"),
    ] {
        if !mods_dir.exists() {
            continue;
        }
        for entry in fs::read_dir(mods_dir)? {
            let entry = entry?;
            if !entry.path().is_file() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                mods.push(name.to_string());
            }
        }
    }
    mods.sort();
    mods.dedup();
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

pub fn load_prism_instance_config(instance_path: &Path) -> std::io::Result<PrismInstanceConfig> {
    let cfg_path = instance_path.join("instance.cfg");
    if !cfg_path.exists() {
        return Ok(PrismInstanceConfig::default());
    }

    let mut cfg = PrismInstanceConfig::default();
    let content = fs::read_to_string(cfg_path)?;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let value = v.trim();
        if value.is_empty() {
            continue;
        }

        match key {
            "iconKey" => cfg.icon_key = Some(value.to_string()),
            "Group" | "group" => cfg.group = Some(value.to_string()),
            "IntendedVersion" | "MinecraftVersion" | "lastLaunchVersionId" => {
                cfg.intended_version = Some(value.to_string())
            }
            "OverrideJavaLocation" => cfg.override_java_location = parse_bool_cfg(value),
            "JavaPath" => cfg.java_path = Some(value.to_string()),
            "OverrideJavaArgs" => cfg.override_java_args = parse_bool_cfg(value),
            "JvmArgs" | "JavaArgs" => cfg.java_args = Some(value.to_string()),
            "OverrideMemory" => cfg.override_memory = parse_bool_cfg(value),
            "MinMemAlloc" => cfg.min_mem_alloc = value.parse::<u32>().ok(),
            "MaxMemAlloc" => cfg.max_mem_alloc = value.parse::<u32>().ok(),
            "PermGen" => cfg.perm_gen = value.parse::<u32>().ok(),
            "OverrideCommands" => cfg.override_commands = parse_bool_cfg(value),
            "PreLaunchCommand" => cfg.pre_launch_command = Some(value.to_string()),
            "PostExitCommand" => cfg.post_exit_command = Some(value.to_string()),
            "WrapperCommand" => cfg.wrapper_command = Some(value.to_string()),
            _ => {}
        }
    }

    Ok(cfg)
}

fn parse_bool_cfg(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

pub fn launch_profile_path(instance_path: &Path) -> PathBuf {
    instance_path.join("launch_profile.json")
}

pub fn load_launch_profile(instance_path: &Path) -> std::io::Result<LaunchProfile> {
    let profile_path = launch_profile_path(instance_path);
    if !profile_path.exists() {
        return Ok(default_launch_profile(instance_path));
    }
    let text = fs::read_to_string(profile_path)?;
    let parsed = serde_json::from_str::<LaunchProfile>(&text).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to parse launch profile: {err}"),
        )
    })?;
    Ok(parsed)
}

pub fn save_launch_profile(instance_path: &Path, profile: &LaunchProfile) -> std::io::Result<()> {
    fs::create_dir_all(instance_path)?;
    let text = serde_json::to_string_pretty(profile).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to serialize launch profile: {err}"),
        )
    })?;
    fs::write(launch_profile_path(instance_path), text)?;
    Ok(())
}

pub fn build_java_command(profile: &LaunchProfile) -> (String, Vec<String>) {
    let mut args = Vec::new();
    args.extend(profile.jvm_args.clone());
    if !profile.classpath.is_empty() {
        let classpath_sep = if cfg!(windows) { ";" } else { ":" };
        args.push("-cp".to_string());
        args.push(profile.classpath.join(classpath_sep));
    }
    args.push(profile.main_class.clone());
    args.extend(profile.game_args.clone());
    (profile.java_path.clone(), args)
}

fn mojang_os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

fn mojang_arch() -> &'static str {
    if cfg!(target_pointer_width = "64") {
        "64"
    } else {
        "32"
    }
}

fn rules_allow_library(lib: &serde_json::Value) -> bool {
    let Some(rules) = lib.get("rules").and_then(|v| v.as_array()) else {
        return true;
    };
    if rules.is_empty() {
        return true;
    }

    let mut allowed = false;
    for rule in rules {
        let action = rule
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("disallow");
        let os_match = rule
            .get("os")
            .and_then(|v| v.as_object())
            .map(|os| {
                os.get("name")
                    .and_then(|v| v.as_str())
                    .map(|name| name == mojang_os_name())
                    .unwrap_or(true)
            })
            .unwrap_or(true);
        if !os_match {
            continue;
        }
        allowed = action == "allow";
    }
    allowed
}

fn ensure_url_to_path(
    client: &Client,
    url: &str,
    path: &Path,
    expected_sha1: Option<&str>,
) -> Result<(), String> {
    if path.is_file() {
        if let Some(sha1) = expected_sha1 {
            if let Ok(existing) = file_sha1_hex(path)
                && existing.eq_ignore_ascii_case(sha1)
            {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("failed to create parent dir: {e}"))?;
    }
    let mut response = client
        .get(url)
        .send()
        .map_err(|e| format!("download request failed for {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("download returned {} for {url}", response.status()));
    }
    let mut file = fs::File::create(path).map_err(|e| format!("failed to create file: {e}"))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut buf)
            .map_err(|e| format!("failed to read response body: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])
            .map_err(|e| format!("failed to write file {}: {e}", path.display()))?;
    }
    file.flush()
        .map_err(|e| format!("failed to flush {}: {e}", path.display()))?;
    if let Some(sha1) = expected_sha1 {
        let existing = file_sha1_hex(path)?;
        if !existing.eq_ignore_ascii_case(sha1) {
            return Err(format!("sha1 mismatch for {}", path.display()));
        }
    }
    Ok(())
}

fn maven_artifact_path(name: &str) -> Option<String> {
    let (coords, ext) = if let Some((left, right)) = name.split_once('@') {
        (left, right)
    } else {
        (name, "jar")
    };
    let mut parts = coords.split(':');
    let group = parts.next()?;
    let artifact = parts.next()?;
    let version = parts.next()?;
    let classifier = parts.next();
    if group.is_empty() || artifact.is_empty() || version.is_empty() {
        return None;
    }
    let group_path = group.replace('.', "/");
    let mut file = format!("{artifact}-{version}");
    if let Some(classifier) = classifier
        && !classifier.is_empty()
    {
        file.push('-');
        file.push_str(classifier);
    }
    file.push('.');
    file.push_str(ext);
    Some(format!(
        "{group_path}/{artifact}/{version}/{file}"
    ))
}

fn resolve_fabric_loader_version(client: &Client, game_version: &str) -> Result<String, String> {
    let url = format!("https://meta.fabricmc.net/v2/versions/loader/{game_version}");
    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("fabric loader versions request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "fabric loader versions request returned {}",
            response.status()
        ));
    }
    let json = response
        .json::<serde_json::Value>()
        .map_err(|e| format!("failed to parse fabric loader versions response: {e}"))?;
    let version = json
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|x| x.get("loader"))
        .and_then(|x| x.get("version"))
        .and_then(|x| x.as_str())
        .ok_or_else(|| "fabric loader versions response is empty".to_string())?;
    Ok(version.to_string())
}

fn upsert_arg_pair(args: &mut Vec<String>, key: &str, value: &str) {
    if let Some(pos) = args.iter().position(|x| x == key) {
        if pos + 1 < args.len() {
            args[pos + 1] = value.to_string();
        } else {
            args.push(value.to_string());
        }
    } else {
        args.push(key.to_string());
        args.push(value.to_string());
    }
}

fn upsert_jvm_property(jvm_args: &mut Vec<String>, key: &str, value: &str) {
    let prefix = format!("-D{key}=");
    if let Some(pos) = jvm_args.iter().position(|x| x.starts_with(&prefix)) {
        jvm_args[pos] = format!("{prefix}{value}");
    } else {
        jvm_args.push(format!("{prefix}{value}"));
    }
}

fn version_manifest_url() -> &'static str {
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
}

fn load_mojang_version_json(
    client: &Client,
    data_root: &Path,
    version_id: &str,
) -> Result<serde_json::Value, String> {
    let manifest = client
        .get(version_manifest_url())
        .send()
        .map_err(|e| format!("failed to fetch version manifest: {e}"))?;
    if !manifest.status().is_success() {
        return Err(format!("version manifest returned {}", manifest.status()));
    }
    let manifest_json = manifest
        .json::<serde_json::Value>()
        .map_err(|e| format!("failed to parse version manifest: {e}"))?;
    let version_url = manifest_json
        .get("versions")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|item| {
                if item.get("id").and_then(|x| x.as_str()) == Some(version_id) {
                    item.get("url")
                        .and_then(|x| x.as_str())
                        .map(ToString::to_string)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| format!("version {version_id} not found in Mojang manifest"))?;

    let version_dir = data_root.join("versions").join(version_id);
    let version_json_path = version_dir.join(format!("{version_id}.json"));
    ensure_url_to_path(client, &version_url, &version_json_path, None)?;
    let text = fs::read_to_string(&version_json_path)
        .map_err(|e| format!("failed to read version json {}: {e}", version_json_path.display()))?;
    serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|e| format!("failed to parse version json {}: {e}", version_json_path.display()))
}

fn merge_parent_child_version(
    parent: &serde_json::Value,
    child: &serde_json::Value,
) -> serde_json::Value {
    let mut merged = parent.clone();
    if let Some(obj) = merged.as_object_mut() {
        for key in [
            "id",
            "mainClass",
            "arguments",
            "minecraftArguments",
            "assetIndex",
            "downloads",
            "type",
            "assets",
        ] {
            if let Some(v) = child.get(key) {
                obj.insert(key.to_string(), v.clone());
            }
        }
        let parent_libs = parent
            .get("libraries")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let child_libs = child
            .get("libraries")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut libs = parent_libs;
        libs.extend(child_libs);
        obj.insert("libraries".to_string(), serde_json::Value::Array(libs));
    }
    merged
}

fn extract_natives_from_jar(jar_path: &Path, natives_dir: &Path) -> Result<(), String> {
    let file = fs::File::open(jar_path)
        .map_err(|e| format!("failed to open natives jar {}: {e}", jar_path.display()))?;
    let mut zip = ZipArchive::new(file)
        .map_err(|e| format!("failed to read natives jar {}: {e}", jar_path.display()))?;
    fs::create_dir_all(natives_dir)
        .map_err(|e| format!("failed to create natives dir {}: {e}", natives_dir.display()))?;

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("failed to access zip entry #{i}: {e}"))?;
        let name = entry.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        if name.starts_with("META-INF/") {
            continue;
        }
        let target = natives_dir.join(&name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create native parent dir: {e}"))?;
        }
        let mut out = fs::File::create(&target)
            .map_err(|e| format!("failed to create native file {}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("failed to extract native file {}: {e}", target.display()))?;
    }

    Ok(())
}

pub fn ensure_minecraft_runtime_with_progress<F>(
    data_root: &Path,
    instance_path: &Path,
    version_id: &str,
    profile: &mut LaunchProfile,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(RuntimeDownloadProgress),
{
    let version_id = version_id.trim();
    if version_id.is_empty() {
        return Err("instance version is empty".to_string());
    }

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
    on_progress(RuntimeDownloadProgress {
        stage: "meta".to_string(),
        done: 0,
        total: 1,
        message: format!("Loading metadata for {version_id}"),
    });

    let mut version_json = load_mojang_version_json(&client, data_root, version_id)?;
    if let Some(parent_id) = version_json
        .get("inheritsFrom")
        .and_then(|x| x.as_str())
        .map(ToString::to_string)
    {
        let parent_json = load_mojang_version_json(&client, data_root, &parent_id)?;
        version_json = merge_parent_child_version(&parent_json, &version_json);
    }

    let versions_dir = data_root.join("versions").join(version_id);
    let libraries_dir = data_root.join("libraries");
    let assets_dir = data_root.join("assets");
    let game_dir = if instance_path.join("minecraft").is_dir() {
        instance_path.join("minecraft")
    } else {
        instance_path.to_path_buf()
    };
    let natives_dir = instance_path.join("natives");
    fs::create_dir_all(&versions_dir)
        .map_err(|e| format!("failed to create versions dir {}: {e}", versions_dir.display()))?;
    fs::create_dir_all(&libraries_dir)
        .map_err(|e| format!("failed to create libraries dir {}: {e}", libraries_dir.display()))?;
    fs::create_dir_all(&assets_dir)
        .map_err(|e| format!("failed to create assets dir {}: {e}", assets_dir.display()))?;
    fs::create_dir_all(&game_dir)
        .map_err(|e| format!("failed to create game dir {}: {e}", game_dir.display()))?;

    let client_download = version_json
        .get("downloads")
        .and_then(|d| d.get("client"))
        .ok_or_else(|| format!("version {version_id} has no client download"))?;
    let client_url = client_download
        .get("url")
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("version {version_id} has no client url"))?;
    let client_sha1 = client_download.get("sha1").and_then(|x| x.as_str());
    let client_jar = versions_dir.join(format!("{version_id}.jar"));
    ensure_url_to_path(&client, client_url, &client_jar, client_sha1)?;
    on_progress(RuntimeDownloadProgress {
        stage: "client".to_string(),
        done: 1,
        total: 1,
        message: "Client jar ready".to_string(),
    });

    let asset_index = version_json
        .get("assetIndex")
        .ok_or_else(|| format!("version {version_id} has no assetIndex"))?;
    let asset_id = asset_index
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or(version_id);
    let asset_url = asset_index
        .get("url")
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("version {version_id} assetIndex has no url"))?;
    let asset_sha1 = asset_index.get("sha1").and_then(|x| x.as_str());
    let asset_index_path = assets_dir.join("indexes").join(format!("{asset_id}.json"));
    ensure_url_to_path(&client, asset_url, &asset_index_path, asset_sha1)?;

    let asset_index_text = fs::read_to_string(&asset_index_path)
        .map_err(|e| format!("failed to read asset index {}: {e}", asset_index_path.display()))?;
    let asset_index_json = serde_json::from_str::<serde_json::Value>(&asset_index_text)
        .map_err(|e| format!("failed to parse asset index {}: {e}", asset_index_path.display()))?;
    if let Some(objects) = asset_index_json.get("objects").and_then(|x| x.as_object()) {
        let total_assets = objects.len().max(1);
        let mut asset_done = 0usize;
        for object in objects.values() {
            let Some(hash) = object.get("hash").and_then(|x| x.as_str()) else {
                continue;
            };
            if hash.len() < 2 {
                continue;
            }
            let target = assets_dir.join("objects").join(&hash[0..2]).join(hash);
            if target.is_file() {
                asset_done += 1;
                continue;
            }
            let obj_url = format!(
                "https://resources.download.minecraft.net/{}/{}",
                &hash[0..2],
                hash
            );
            ensure_url_to_path(&client, &obj_url, &target, Some(hash))?;
            asset_done += 1;
            if asset_done.is_multiple_of(25) || asset_done == total_assets {
                on_progress(RuntimeDownloadProgress {
                    stage: "assets".to_string(),
                    done: asset_done,
                    total: total_assets,
                    message: format!("Assets {asset_done}/{total_assets}"),
                });
            }
        }
    }

    let mut classpath = Vec::new();
    if let Some(libraries) = version_json.get("libraries").and_then(|x| x.as_array()) {
        let total_libs = libraries.len().max(1);
        let mut libs_done = 0usize;
        for lib in libraries {
            if !rules_allow_library(lib) {
                libs_done += 1;
                continue;
            }
            if let Some(artifact) = lib.get("downloads").and_then(|d| d.get("artifact")) {
                let path = artifact.get("path").and_then(|x| x.as_str()).unwrap_or("");
                let url = artifact.get("url").and_then(|x| x.as_str()).unwrap_or("");
                let sha1 = artifact.get("sha1").and_then(|x| x.as_str());
                if !path.is_empty() && !url.is_empty() {
                    let lib_path = libraries_dir.join(path);
                    ensure_url_to_path(&client, url, &lib_path, sha1)?;
                    classpath.push(lib_path.display().to_string());
                }
            }

            let classifier = lib
                .get("natives")
                .and_then(|n| n.get(mojang_os_name()))
                .and_then(|x| x.as_str())
                .map(|x| x.replace("${arch}", mojang_arch()));
            if let Some(classifier) = classifier
                && let Some(native) = lib
                    .get("downloads")
                    .and_then(|d| d.get("classifiers"))
                    .and_then(|c| c.get(&classifier))
            {
                let path = native.get("path").and_then(|x| x.as_str()).unwrap_or("");
                let url = native.get("url").and_then(|x| x.as_str()).unwrap_or("");
                let sha1 = native.get("sha1").and_then(|x| x.as_str());
                if !path.is_empty() && !url.is_empty() {
                    let native_jar = libraries_dir.join(path);
                    ensure_url_to_path(&client, url, &native_jar, sha1)?;
                    extract_natives_from_jar(&native_jar, &natives_dir)?;
                }
            }
            libs_done += 1;
            if libs_done.is_multiple_of(15) || libs_done == total_libs {
                on_progress(RuntimeDownloadProgress {
                    stage: "libraries".to_string(),
                    done: libs_done,
                    total: total_libs,
                    message: format!("Libraries {libs_done}/{total_libs}"),
                });
            }
        }
    }
    classpath.push(client_jar.display().to_string());

    profile.classpath = classpath;
    if let Some(main_class) = version_json.get("mainClass").and_then(|x| x.as_str()) {
        profile.main_class = main_class.to_string();
    }

    upsert_arg_pair(&mut profile.game_args, "--version", version_id);
    upsert_arg_pair(&mut profile.game_args, "--gameDir", &game_dir.display().to_string());
    upsert_arg_pair(
        &mut profile.game_args,
        "--assetsDir",
        &assets_dir.display().to_string(),
    );
    upsert_arg_pair(&mut profile.game_args, "--assetIndex", asset_id);
    upsert_arg_pair(
        &mut profile.game_args,
        "--versionType",
        version_json
            .get("type")
            .and_then(|x| x.as_str())
            .unwrap_or("release"),
    );
    upsert_jvm_property(
        &mut profile.jvm_args,
        "java.library.path",
        &natives_dir.display().to_string(),
    );
    on_progress(RuntimeDownloadProgress {
        stage: "done".to_string(),
        done: 1,
        total: 1,
        message: "Runtime ready".to_string(),
    });

    Ok(())
}

pub fn ensure_minecraft_runtime(
    data_root: &Path,
    instance_path: &Path,
    version_id: &str,
    profile: &mut LaunchProfile,
) -> Result<(), String> {
    ensure_minecraft_runtime_with_progress(data_root, instance_path, version_id, profile, |_| {})
}

pub fn ensure_fabric_runtime_with_progress<F>(
    data_root: &Path,
    instance_path: &Path,
    game_version: &str,
    loader_version_hint: Option<&str>,
    profile: &mut LaunchProfile,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(RuntimeDownloadProgress),
{
    ensure_minecraft_runtime_with_progress(
        data_root,
        instance_path,
        game_version,
        profile,
        |p| on_progress(p),
    )?;

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let loader_version = match loader_version_hint.map(str::trim) {
        Some(v) if !v.is_empty() && v != "0.0.0" => v.to_string(),
        _ => resolve_fabric_loader_version(&client, game_version)?,
    };

    on_progress(RuntimeDownloadProgress {
        stage: "fabric".to_string(),
        done: 0,
        total: 1,
        message: format!("Preparing Fabric loader {loader_version}"),
    });

    let profile_url = format!(
        "https://meta.fabricmc.net/v2/versions/loader/{}/{}/profile/json",
        game_version, loader_version
    );
    let response = client
        .get(&profile_url)
        .send()
        .map_err(|e| format!("fabric profile request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("fabric profile request returned {}", response.status()));
    }
    let json = response
        .json::<serde_json::Value>()
        .map_err(|e| format!("failed to parse fabric profile json: {e}"))?;

    let mut added = 0usize;
    let mut total = 0usize;
    if let Some(libs) = json.get("libraries").and_then(|x| x.as_array()) {
        total = libs.len().max(1);
        for (idx, lib) in libs.iter().enumerate() {
            let name = lib.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let base_url = lib
                .get("url")
                .and_then(|x| x.as_str())
                .unwrap_or("https://maven.fabricmc.net/");
            let sha1 = lib.get("sha1").and_then(|x| x.as_str());
            let Some(rel_path) = maven_artifact_path(name) else {
                continue;
            };
            let target = data_root.join("libraries").join(&rel_path);
            let mut full_url = base_url.trim_end_matches('/').to_string();
            full_url.push('/');
            full_url.push_str(&rel_path);
            ensure_url_to_path(&client, &full_url, &target, sha1)?;
            let target_str = target.display().to_string();
            if !profile.classpath.iter().any(|x| x == &target_str) {
                profile.classpath.push(target_str);
                added += 1;
            }
            on_progress(RuntimeDownloadProgress {
                stage: "fabric".to_string(),
                done: idx + 1,
                total,
                message: format!("Fabric libraries {}/{}", idx + 1, total),
            });
        }
    }

    if let Some(main_class) = json.get("mainClass").and_then(|x| x.as_str())
        && !main_class.trim().is_empty()
    {
        profile.main_class = main_class.to_string();
    }
    if let Some(jvm_args) = json
        .get("arguments")
        .and_then(|x| x.get("jvm"))
        .and_then(|x| x.as_array())
    {
        for arg in jvm_args {
            if let Some(s) = arg.as_str()
                && !profile.jvm_args.iter().any(|x| x == s)
            {
                profile.jvm_args.push(s.to_string());
            }
        }
    }

    on_progress(RuntimeDownloadProgress {
        stage: "fabric".to_string(),
        done: total.max(1),
        total: total.max(1),
        message: format!("Fabric runtime ready (+{added} libs)"),
    });
    Ok(())
}

#[derive(Deserialize)]
struct MinecraftProfileResponse {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct EntitlementsResponse {
    items: Vec<serde_json::Value>,
}

pub fn validate_minecraft_account(access_token: &str) -> Result<AccountValidation, String> {
    let token = access_token.trim();
    if token.is_empty() {
        return Err("access token is empty".to_string());
    }

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let profile = client
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(token)
        .send()
        .map_err(|e| format!("profile request failed: {e}"))?;
    if !profile.status().is_success() {
        return Err(format!("profile request returned {}", profile.status()));
    }
    let profile = profile
        .json::<MinecraftProfileResponse>()
        .map_err(|e| format!("failed to parse profile response: {e}"))?;

    let entitlements = client
        .get("https://api.minecraftservices.com/entitlements/mcstore")
        .bearer_auth(token)
        .send()
        .map_err(|e| format!("entitlements request failed: {e}"))?;
    if !entitlements.status().is_success() {
        return Err(format!(
            "entitlements request returned {}",
            entitlements.status()
        ));
    }
    let entitlements = entitlements
        .json::<EntitlementsResponse>()
        .map_err(|e| format!("failed to parse entitlements response: {e}"))?;

    Ok(AccountValidation {
        username: profile.name,
        uuid: profile.id,
        has_minecraft_license: !entitlements.items.is_empty(),
    })
}

#[derive(Deserialize)]
struct DeviceCodeResponse {
    device_code: Option<String>,
    user_code: Option<String>,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct DeviceTokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Deserialize)]
struct XboxDisplayClaims {
    xui: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct XboxTokenResponse {
    #[serde(rename = "Token")]
    token: String,
    #[serde(rename = "DisplayClaims")]
    display_claims: XboxDisplayClaims,
}

#[derive(Deserialize)]
struct MinecraftLoginResponse {
    access_token: String,
}

pub fn start_microsoft_device_code(client_id: &str) -> Result<MicrosoftDeviceCode, String> {
    let id = client_id.trim();
    if id.is_empty() {
        return Err("MSA client id is empty".to_string());
    }

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let response = client
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode")
        .form(&[
            ("client_id", id),
            ("scope", "XboxLive.SignIn XboxLive.offline_access"),
        ])
        .send()
        .map_err(|e| format!("device code request failed: {e}"))?;
    let status = response.status();
    let parsed = response
        .json::<DeviceCodeResponse>()
        .map_err(|e| format!("failed to parse device code response: {e}"))?;
    if !status.is_success() {
        let err = parsed.error.unwrap_or_else(|| format!("http {status}"));
        let msg = parsed.error_description.unwrap_or_default();
        return Err(if msg.is_empty() {
            format!("device code request failed: {err}")
        } else {
            format!("device code request failed: {err}: {msg}")
        });
    }

    let device_code = parsed
        .device_code
        .ok_or_else(|| "device code response missing device_code".to_string())?;
    let user_code = parsed
        .user_code
        .ok_or_else(|| "device code response missing user_code".to_string())?;
    let verification_uri = parsed
        .verification_uri
        .ok_or_else(|| "device code response missing verification_uri".to_string())?;

    Ok(MicrosoftDeviceCode {
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete: parsed.verification_uri_complete,
        expires_in: parsed.expires_in.unwrap_or(900),
        interval: parsed.interval.unwrap_or(5),
    })
}

fn extract_uhs(claims: &XboxDisplayClaims) -> Option<String> {
    for item in &claims.xui {
        let obj = item.as_object()?;
        if let Some(uhs) = obj.get("uhs").and_then(|x| x.as_str()) {
            return Some(uhs.to_string());
        }
    }
    None
}

fn build_http_client(http1_only: bool) -> Result<Client, String> {
    let mut builder = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .connect_timeout(Duration::from_secs(12))
        .timeout(Duration::from_secs(45))
        .tcp_keepalive(Duration::from_secs(30));
    if http1_only {
        builder = builder.http1_only();
    }
    builder
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))
}

fn post_json_with_http_fallback(
    client: &Client,
    url: &str,
    body: &serde_json::Value,
    with_xbl_header: bool,
) -> Result<reqwest::blocking::Response, String> {
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");
    if with_xbl_header {
        req = req.header("x-xbl-contract-version", "1");
    }
    match req.json(body).send() {
        Ok(response) => Ok(response),
        Err(first_err) => {
            let fallback = build_http_client(true)?;
            let mut req_fallback = fallback
                .post(url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json");
            if with_xbl_header {
                req_fallback = req_fallback.header("x-xbl-contract-version", "1");
            }
            req_fallback.json(body).send().map_err(|second_err| {
                format!(
                    "request failed (h2): {first_err}; retry failed (http1): {second_err}"
                )
            })
        }
    }
}

pub fn complete_microsoft_device_login(
    client_id: &str,
    device: &MicrosoftDeviceCode,
) -> Result<LicensedMicrosoftAccount, String> {
    let id = client_id.trim();
    if id.is_empty() {
        return Err("MSA client id is empty".to_string());
    }

    let client = build_http_client(false)?;

    let mut interval = device.interval.max(1);
    let max_polls = (device.expires_in / interval.max(1)).saturating_add(2);
    let mut msa_access_token = String::new();
    let mut msa_refresh_token = None::<String>;

    for _ in 0..max_polls {
        thread::sleep(Duration::from_secs(interval));
        let response = client
            .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
            .form(&[
                ("client_id", id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device.device_code.as_str()),
            ])
            .send()
            .map_err(|e| format!("device token request failed: {e}"))?;
        let status = response.status();
        let parsed = response
            .json::<DeviceTokenResponse>()
            .map_err(|e| format!("failed to parse device token response: {e}"))?;

        if status.is_success() {
            if let Some(token) = parsed.access_token {
                msa_access_token = token;
                msa_refresh_token = parsed.refresh_token;
                break;
            }
            return Err("device token response missing access_token".to_string());
        }

        let code = parsed.error.unwrap_or_else(|| "authorization_pending".to_string());
        match code.as_str() {
            "authorization_pending" => {}
            "slow_down" => {
                interval = interval.saturating_add(5);
            }
            "expired_token" => return Err("device login expired before approval".to_string()),
            _ => {
                let msg = parsed.error_description.unwrap_or_default();
                return Err(if msg.is_empty() {
                    format!("device token error: {code}")
                } else {
                    format!("device token error: {code}: {msg}")
                });
            }
        }
    }
    if msa_access_token.is_empty() {
        return Err("device login timed out".to_string());
    }

    let mut account = exchange_msa_access_token_for_minecraft_account(&client, &msa_access_token)?;
    account.refresh_token = msa_refresh_token;
    Ok(account)
}

pub fn refresh_microsoft_account(
    client_id: &str,
    refresh_token: &str,
) -> Result<LicensedMicrosoftAccount, String> {
    let id = client_id.trim();
    if id.is_empty() {
        return Err("MSA client id is empty".to_string());
    }
    let refresh = refresh_token.trim();
    if refresh.is_empty() {
        return Err("refresh token is empty".to_string());
    }

    let client = build_http_client(false)?;
    let response = client
        .post("https://login.microsoftonline.com/consumers/oauth2/v2.0/token")
        .form(&[
            ("client_id", id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("scope", "XboxLive.SignIn XboxLive.offline_access"),
        ])
        .send()
        .map_err(|e| format!("refresh token request failed: {e}"))?;
    let status = response.status();
    let parsed = response
        .json::<DeviceTokenResponse>()
        .map_err(|e| format!("failed to parse refresh token response: {e}"))?;
    if !status.is_success() {
        let code = parsed.error.unwrap_or_else(|| format!("http {status}"));
        let msg = parsed.error_description.unwrap_or_default();
        return Err(if msg.is_empty() {
            format!("refresh token error: {code}")
        } else {
            format!("refresh token error: {code}: {msg}")
        });
    }
    let msa_access = parsed
        .access_token
        .ok_or_else(|| "refresh token response missing access_token".to_string())?;
    let mut account = exchange_msa_access_token_for_minecraft_account(&client, &msa_access)?;
    account.refresh_token = parsed.refresh_token.or_else(|| Some(refresh.to_string()));
    Ok(account)
}

fn exchange_msa_access_token_for_minecraft_account(
    client: &Client,
    msa_access_token: &str,
) -> Result<LicensedMicrosoftAccount, String> {
    let xbox_user_body = serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={msa_access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        });
    let xbox_user = post_json_with_http_fallback(
        &client,
        "https://user.auth.xboxlive.com/user/authenticate",
        &xbox_user_body,
        true,
    )
    .map_err(|e| format!("xbox user auth request failed: {e}"))?;
    if !xbox_user.status().is_success() {
        return Err(format!("xbox user auth returned {}", xbox_user.status()));
    }
    let xbox_user = xbox_user
        .json::<XboxTokenResponse>()
        .map_err(|e| format!("failed to parse xbox user auth: {e}"))?;
    let uhs = extract_uhs(&xbox_user.display_claims)
        .ok_or_else(|| "xbox user auth missing user hash".to_string())?;

    let xsts_body = serde_json::json!({
            "Properties": {
                "SandboxId": "RETAIL",
                "UserTokens": [xbox_user.token],
            },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        });
    let xsts = post_json_with_http_fallback(
        &client,
        "https://xsts.auth.xboxlive.com/xsts/authorize",
        &xsts_body,
        true,
    )
    .map_err(|e| format!("xsts authorize request failed: {e}"))?;
    if !xsts.status().is_success() {
        return Err(format!("xsts authorize returned {}", xsts.status()));
    }
    let xsts = xsts
        .json::<XboxTokenResponse>()
        .map_err(|e| format!("failed to parse xsts authorize: {e}"))?;
    let xsts_uhs =
        extract_uhs(&xsts.display_claims).ok_or_else(|| "xsts token missing user hash".to_string())?;
    if xsts_uhs != uhs {
        return Err("xsts user hash does not match xbox user hash".to_string());
    }

    let mc_login_body = serde_json::json!({
            "xtoken": format!("XBL3.0 x={uhs};{}", xsts.token),
            "platform": "PC_LAUNCHER",
        });
    let mc_login = post_json_with_http_fallback(
        &client,
        "https://api.minecraftservices.com/launcher/login",
        &mc_login_body,
        false,
    )
    .map_err(|e| format!("minecraft launcher login request failed: {e}"))?;
    if !mc_login.status().is_success() {
        return Err(format!("minecraft launcher login returned {}", mc_login.status()));
    }
    let mc_login = mc_login
        .json::<MinecraftLoginResponse>()
        .map_err(|e| format!("failed to parse minecraft launcher login: {e}"))?;

    let validation = validate_minecraft_account(&mc_login.access_token)?;
    Ok(LicensedMicrosoftAccount {
        username: validation.username,
        uuid: validation.uuid,
        access_token: mc_login.access_token,
        refresh_token: None,
        has_minecraft_license: validation.has_minecraft_license,
    })
}

#[derive(Deserialize)]
struct ModrinthSearchResponse {
    hits: Vec<ModrinthSearchHitResponse>,
}

#[derive(Deserialize)]
struct ModrinthSearchHitResponse {
    title: String,
    project_id: String,
    slug: String,
    description: Option<String>,
    author: Option<String>,
    icon_url: Option<String>,
}

#[derive(Deserialize)]
struct ModrinthVersionResponse {
    game_versions: Vec<String>,
    loaders: Vec<String>,
    files: Vec<ModrinthVersionFileResponse>,
}

#[derive(Deserialize)]
struct ModrinthVersionFileResponse {
    url: String,
    filename: String,
    primary: Option<bool>,
    hashes: Option<ModrinthVersionFileHashesResponse>,
}

#[derive(Deserialize)]
struct ModrinthVersionFileHashesResponse {
    sha1: Option<String>,
}

#[derive(Deserialize)]
struct ModrinthProjectDetailsResponse {
    title: String,
    slug: String,
    description: Option<String>,
    body: Option<String>,
    issues_url: Option<String>,
    source_url: Option<String>,
    wiki_url: Option<String>,
    discord_url: Option<String>,
    donation_urls: Option<Vec<ModrinthDonationLinkResponse>>,
    icon_url: Option<String>,
}

#[derive(Deserialize)]
struct ModrinthDonationLinkResponse {
    platform: Option<String>,
    url: Option<String>,
}

pub fn modrinth_search_projects(
    query: &str,
    limit: usize,
) -> Result<Vec<ModrinthSearchHit>, String> {
    modrinth_search_projects_by_type_paged(query, limit, "mod", 0)
}

pub fn modrinth_search_projects_by_type(
    query: &str,
    limit: usize,
    project_type: &str,
) -> Result<Vec<ModrinthSearchHit>, String> {
    modrinth_search_projects_by_type_paged(query, limit, project_type, 0)
}

pub fn modrinth_search_projects_by_type_paged(
    query: &str,
    limit: usize,
    project_type: &str,
    offset: usize,
) -> Result<Vec<ModrinthSearchHit>, String> {
    let q = query.trim();
    let project_type = project_type.trim();
    if project_type.is_empty() {
        return Err("modrinth project_type is empty".to_string());
    }

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let facets = format!("[[\"project_type:{project_type}\"]]");
    let mut parsed: Option<ModrinthSearchResponse> = None;
    let sort_indices = if q.is_empty() {
        [Some("downloads"), Some("updated"), Some("newest"), None]
    } else {
        [None, Some("downloads"), Some("updated"), Some("newest")]
    };
    let mut last_error = "unknown modrinth error".to_string();
    for idx in sort_indices {
        let mut req = client
            .get("https://api.modrinth.com/v2/search")
            .query(&[("limit", limit)])
            .query(&[("offset", offset)])
            .query(&[("facets", facets.as_str())]);
        if !q.is_empty() {
            req = req.query(&[("query", q)]);
        }
        if let Some(index) = idx {
            req = req.query(&[("index", index)]);
        }
        let response = req
            .send()
            .map_err(|e| format!("modrinth search failed: {e}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            last_error = format!("modrinth search returned {status}: {body}");
            continue;
        }
        match response.json::<ModrinthSearchResponse>() {
            Ok(p) => {
                parsed = Some(p);
                break;
            }
            Err(e) => {
                last_error = format!("failed to parse modrinth search response: {e}");
            }
        }
    }
    let parsed = parsed.ok_or(last_error)?;
    Ok(parsed
        .hits
        .into_iter()
        .map(|x| ModrinthSearchHit {
            title: x.title,
            project_id: x.project_id,
            slug: x.slug,
            description: x.description.unwrap_or_default(),
            author: x.author.unwrap_or_default(),
            icon_url: x.icon_url,
        })
        .collect())
}

pub fn modrinth_resolve_primary_file(
    project_id: &str,
    game_version: &str,
    loader: &str,
) -> Result<ModrinthDownloadFile, String> {
    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let response = client
        .get(format!(
            "https://api.modrinth.com/v2/project/{project_id}/version"
        ))
        .send()
        .map_err(|e| format!("modrinth versions request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "modrinth versions request returned {}",
            response.status()
        ));
    }

    let versions = response
        .json::<Vec<ModrinthVersionResponse>>()
        .map_err(|e| format!("failed to parse modrinth versions response: {e}"))?;

    let match_version = versions.into_iter().find(|v| {
        let loader_ok = loader.trim().is_empty() || v.loaders.iter().any(|x| x == loader);
        let game_ok =
            game_version.trim().is_empty() || v.game_versions.iter().any(|x| x == game_version);
        loader_ok && game_ok
    });

    let version = match_version.ok_or_else(|| "no matching modrinth version found".to_string())?;
    let primary = version
        .files
        .iter()
        .find(|f| f.primary.unwrap_or(false))
        .or_else(|| version.files.first())
        .ok_or_else(|| "selected modrinth version has no files".to_string())?;

    Ok(ModrinthDownloadFile {
        url: primary.url.clone(),
        filename: primary.filename.clone(),
    })
}

pub fn modrinth_get_project_details(project_id: &str) -> Result<ModrinthProjectDetails, String> {
    let id = project_id.trim();
    if id.is_empty() {
        return Err("modrinth project id is empty".to_string());
    }

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let response = client
        .get(format!("https://api.modrinth.com/v2/project/{id}"))
        .send()
        .map_err(|e| format!("modrinth project request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("modrinth project request returned {}", response.status()));
    }
    let parsed = response
        .json::<ModrinthProjectDetailsResponse>()
        .map_err(|e| format!("failed to parse modrinth project response: {e}"))?;

    let donate_links = parsed
        .donation_urls
        .unwrap_or_default()
        .into_iter()
        .filter_map(|x| {
            let platform = x.platform?;
            let url = x.url?;
            if url.trim().is_empty() {
                return None;
            }
            Some((platform, url))
        })
        .collect::<Vec<_>>();

    Ok(ModrinthProjectDetails {
        title: parsed.title,
        website_url: format!("https://modrinth.com/project/{}", parsed.slug),
        summary: parsed.description.unwrap_or_default(),
        body_markdown: parsed.body.unwrap_or_default(),
        issues_url: parsed.issues_url.unwrap_or_default(),
        source_url: parsed.source_url.unwrap_or_default(),
        wiki_url: parsed.wiki_url.unwrap_or_default(),
        discord_url: parsed.discord_url.unwrap_or_default(),
        donate_links,
        icon_url: parsed.icon_url,
    })
}

#[derive(Deserialize)]
struct CurseForgeSearchEnvelope {
    data: Vec<CurseForgeModData>,
}

#[derive(Deserialize)]
struct CurseForgeModEnvelope {
    data: CurseForgeModData,
}

#[derive(Deserialize)]
struct CurseForgeFilesEnvelope {
    data: Vec<CurseForgeFileData>,
}

#[derive(Deserialize)]
struct CurseForgeModData {
    id: i64,
    name: String,
    slug: String,
    summary: Option<String>,
    links: Option<CurseForgeLinks>,
    authors: Option<Vec<CurseForgeAuthor>>,
    logo: Option<CurseForgeLogo>,
}

#[derive(Deserialize)]
struct CurseForgeAuthor {
    name: Option<String>,
}

#[derive(Deserialize)]
struct CurseForgeLogo {
    url: Option<String>,
}

#[derive(Deserialize)]
struct CurseForgeLinks {
    website_url: Option<String>,
    issues_url: Option<String>,
    source_url: Option<String>,
    wiki_url: Option<String>,
}

#[derive(Deserialize)]
struct CurseForgeFileData {
    #[serde(rename = "downloadUrl")]
    download_url: Option<String>,
    #[serde(rename = "fileName")]
    file_name: Option<String>,
    #[serde(rename = "gameVersions")]
    game_versions: Option<Vec<String>>,
}

pub fn curseforge_search_projects(
    api_key: &str,
    query: &str,
    game_version: &str,
    class_id: i32,
    limit: usize,
) -> Result<Vec<CurseForgeSearchHit>, String> {
    curseforge_search_projects_paged(api_key, query, game_version, class_id, limit, 0)
}

pub fn curseforge_search_projects_paged(
    api_key: &str,
    query: &str,
    game_version: &str,
    class_id: i32,
    limit: usize,
    offset: usize,
) -> Result<Vec<CurseForgeSearchHit>, String> {
    let q = query.trim();
    let key = api_key.trim();
    if key.is_empty() {
        return Err("CurseForge API key is empty".to_string());
    }

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
    let mut req = client
        .get("https://api.curseforge.com/v1/mods/search")
        .header("x-api-key", key)
        .query(&[
            ("gameId", "432"),
            ("classId", &class_id.to_string()),
            ("pageSize", &limit.to_string()),
            ("index", &offset.to_string()),
        ]);
    if !q.is_empty() {
        req = req.query(&[("searchFilter", q)]);
    }
    if !game_version.trim().is_empty() {
        req = req.query(&[("gameVersion", game_version.trim())]);
    }

    let response = req
        .send()
        .map_err(|e| format!("curseforge search failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("curseforge search returned {}", response.status()));
    }

    let parsed = response
        .json::<CurseForgeSearchEnvelope>()
        .map_err(|e| format!("failed to parse curseforge search response: {e}"))?;

    Ok(parsed
        .data
        .into_iter()
        .map(|x| CurseForgeSearchHit {
            mod_id: x.id,
            title: x.name,
            slug: x.slug,
            summary: x.summary.unwrap_or_default(),
            website_url: x.links.and_then(|l| l.website_url).unwrap_or_default(),
            author: x
                .authors
                .unwrap_or_default()
                .into_iter()
                .find_map(|a| a.name)
                .unwrap_or_default(),
            icon_url: x.logo.and_then(|l| l.url),
        })
        .collect())
}

pub fn curseforge_get_project_details(
    api_key: &str,
    mod_id: i64,
) -> Result<CurseForgeProjectDetails, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("CurseForge API key is empty".to_string());
    }

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
    let response = client
        .get(format!("https://api.curseforge.com/v1/mods/{mod_id}"))
        .header("x-api-key", key)
        .send()
        .map_err(|e| format!("curseforge project request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "curseforge project request returned {}",
            response.status()
        ));
    }
    let parsed = response
        .json::<CurseForgeModEnvelope>()
        .map_err(|e| format!("failed to parse curseforge project response: {e}"))?
        .data;

    Ok(CurseForgeProjectDetails {
        title: parsed.name,
        summary: parsed.summary.unwrap_or_default(),
        website_url: parsed
            .links
            .as_ref()
            .and_then(|x| x.website_url.clone())
            .unwrap_or_default(),
        issues_url: parsed
            .links
            .as_ref()
            .and_then(|x| x.issues_url.clone())
            .unwrap_or_default(),
        source_url: parsed
            .links
            .as_ref()
            .and_then(|x| x.source_url.clone())
            .unwrap_or_default(),
        wiki_url: parsed
            .links
            .as_ref()
            .and_then(|x| x.wiki_url.clone())
            .unwrap_or_default(),
        icon_url: parsed.logo.and_then(|x| x.url),
    })
}

pub fn curseforge_resolve_primary_file(
    api_key: &str,
    mod_id: i64,
    game_version: &str,
) -> Result<ModrinthDownloadFile, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("CurseForge API key is empty".to_string());
    }
    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;
    let response = client
        .get(format!("https://api.curseforge.com/v1/mods/{mod_id}/files"))
        .header("x-api-key", key)
        .query(&[("pageSize", "50")])
        .send()
        .map_err(|e| format!("curseforge files request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("curseforge files request returned {}", response.status()));
    }
    let parsed = response
        .json::<CurseForgeFilesEnvelope>()
        .map_err(|e| format!("failed to parse curseforge files response: {e}"))?;
    let selected = parsed.data.into_iter().find(|f| {
        if game_version.trim().is_empty() {
            return f.download_url.is_some() && f.file_name.is_some();
        }
        f.download_url.is_some()
            && f.file_name.is_some()
            && f.game_versions
                .as_ref()
                .map(|vs| vs.iter().any(|v| v == game_version))
                .unwrap_or(false)
    });
    let selected = selected.ok_or_else(|| "no suitable curseforge file found".to_string())?;
    Ok(ModrinthDownloadFile {
        url: selected
            .download_url
            .ok_or_else(|| "selected curseforge file has no download URL".to_string())?,
        filename: selected
            .file_name
            .ok_or_else(|| "selected curseforge file has no filename".to_string())?,
    })
}

pub fn download_file_to_path(url: &str, path: &Path) -> Result<(), String> {
    download_file_to_path_with_progress(url, path, |_, _| {})
}

pub fn download_file_to_path_with_progress<F>(
    url: &str,
    path: &Path,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, Option<u64>),
{
    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let mut response = client
        .get(url)
        .send()
        .map_err(|e| format!("download request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("download returned {}", response.status()));
    }
    let total = response.content_length();

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create parent directory: {e}"))?;
    }
    let mut file = fs::File::create(path).map_err(|e| format!("failed to create file: {e}"))?;
    let mut downloaded = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = response
            .read(&mut buf)
            .map_err(|e| format!("failed to read download body: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])
            .map_err(|e| format!("failed to write file: {e}"))?;
        downloaded += read as u64;
        on_progress(downloaded, total);
    }
    file.flush()
        .map_err(|e| format!("failed to flush file: {e}"))?;
    Ok(())
}

#[derive(Deserialize)]
struct MrpackIndex {
    files: Vec<MrpackFile>,
}

#[derive(Deserialize)]
struct MrpackFile {
    path: String,
    hashes: Option<MrpackHashes>,
    downloads: Vec<String>,
}

#[derive(Deserialize)]
struct MrpackHashes {
    sha1: Option<String>,
}

fn file_sha1_hex(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open file for hashing failed: {e}"))?;
    let mut sha = Sha1::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("read file for hashing failed: {e}"))?;
        if read == 0 {
            break;
        }
        sha.update(&buf[..read]);
    }
    Ok(format!("{:x}", sha.finalize()))
}

pub fn sync_modrinth_managed_mods(instance_path: &Path) -> Result<usize, String> {
    let index_path = instance_path.join("mrpack").join("modrinth.index.json");
    if !index_path.exists() {
        return Ok(0);
    }
    let text =
        fs::read_to_string(&index_path).map_err(|e| format!("failed to read modrinth index: {e}"))?;
    let index = serde_json::from_str::<MrpackIndex>(&text)
        .map_err(|e| format!("failed to parse modrinth index: {e}"))?;

    let mut updated = 0usize;
    let game_root = instance_path.join("minecraft");
    let known_mod_dirs = modrinth_mod_dirs(instance_path);
    for item in index.files {
        let rel = item.path.replace('\\', "/");
        if !rel.starts_with("mods/") {
            continue;
        }
        let target = game_root.join(&item.path);
        let Some(file_name) = Path::new(&item.path).file_name().and_then(|x| x.to_str()) else {
            continue;
        };
        let disabled_name = format!("{file_name}.disabled");
        let disabled_exists = known_mod_dirs
            .iter()
            .any(|dir| dir.join(&disabled_name).is_file());
        if disabled_exists {
            // Respect explicit user disable toggle and do not re-download managed jar.
            continue;
        }
        let mut needs_download = !target.is_file();
        if !needs_download
            && let Some(expected) = item
                .hashes
                .as_ref()
                .and_then(|h| h.sha1.as_ref())
                .map(|x| x.to_ascii_lowercase())
        {
            let actual = file_sha1_hex(&target)?;
            if actual != expected {
                needs_download = true;
            }
        }
        if !needs_download {
            continue;
        }

        let url = item
            .downloads
            .first()
            .ok_or_else(|| format!("mod index entry has no download URL: {}", item.path))?;
        download_file_to_path(url, &target)?;
        updated += 1;
    }
    Ok(updated)
}

fn modrinth_mod_dirs(instance_path: &Path) -> Vec<PathBuf> {
    vec![
        instance_path.join("mods"),
        instance_path.join(".minecraft").join("mods"),
        instance_path.join("minecraft").join("mods"),
    ]
}

fn list_mod_jar_paths(instance_path: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for mods_dir in modrinth_mod_dirs(instance_path) {
        if !mods_dir.is_dir() {
            continue;
        }
        let entries = fs::read_dir(&mods_dir)
            .map_err(|e| format!("failed to read mods directory {}: {e}", mods_dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("failed to read mods entry: {e}"))?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let is_jar = path
                .extension()
                .and_then(|x| x.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("jar"))
                .unwrap_or(false);
            if !is_jar {
                continue;
            }
            let key = path
                .file_name()
                .and_then(|x| x.to_str())
                .map(|x| x.to_ascii_lowercase())
                .unwrap_or_else(|| path.display().to_string());
            if seen.insert(key) {
                out.push(path);
            }
        }
    }
    Ok(out)
}

fn modrinth_json_array_query(value: &str) -> Result<String, String> {
    serde_json::to_string(&vec![value])
        .map_err(|e| format!("failed to serialize modrinth query values: {e}"))
}

pub fn update_installed_modrinth_mods(
    instance_path: &Path,
    game_version: &str,
    loader: &str,
) -> Result<usize, String> {
    let game_version = game_version.trim();
    if game_version.is_empty() {
        return Ok(0);
    }

    let jar_paths = list_mod_jar_paths(instance_path)?;
    if jar_paths.is_empty() {
        return Ok(0);
    }

    let game_versions_json = modrinth_json_array_query(game_version)?;
    let loader_json = if loader.trim().is_empty() {
        None
    } else {
        Some(modrinth_json_array_query(loader.trim())?)
    };

    let client = Client::builder()
        .user_agent("PrismarineLauncher-Rust")
        .build()
        .map_err(|e| format!("failed to build http client: {e}"))?;

    let mut updated = 0usize;
    for path in jar_paths {
        let current_sha1 = match file_sha1_hex(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let url = format!("https://api.modrinth.com/v2/version_file/{current_sha1}/update");
        let mut request = client
            .get(url)
            .query(&[("algorithm", "sha1"), ("game_versions", &game_versions_json)]);
        if let Some(loader_json) = &loader_json {
            request = request.query(&[("loaders", loader_json)]);
        }
        let response = match request.send() {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !response.status().is_success() {
            continue;
        }
        let version = match response.json::<ModrinthVersionResponse>() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(file) = version
            .files
            .iter()
            .find(|f| f.primary.unwrap_or(false))
            .or_else(|| version.files.first())
        else {
            continue;
        };

        let target_sha1 = file
            .hashes
            .as_ref()
            .and_then(|h| h.sha1.as_ref())
            .map(|x| x.to_ascii_lowercase());
        if target_sha1
            .as_ref()
            .map(|sha1| sha1 == &current_sha1)
            .unwrap_or(false)
        {
            continue;
        }

        let temp_name = format!(
            ".{}.download",
            path.file_name().and_then(|x| x.to_str()).unwrap_or("mod")
        );
        let temp_path = path.with_file_name(temp_name);
        if let Err(err) = download_file_to_path(&file.url, &temp_path) {
            let _ = fs::remove_file(&temp_path);
            return Err(format!(
                "failed to download updated mod for {}: {err}",
                path.display()
            ));
        }
        if let Some(expected) = target_sha1.as_ref() {
            let downloaded = file_sha1_hex(&temp_path)?;
            if &downloaded != expected {
                let _ = fs::remove_file(&temp_path);
                return Err(format!(
                    "mod update hash mismatch for {} (expected {}, got {})",
                    path.display(),
                    expected,
                    downloaded
                ));
            }
        }
        fs::rename(&temp_path, &path).map_err(|e| {
            let _ = fs::remove_file(&temp_path);
            format!("failed to replace mod file {}: {e}", path.display())
        })?;
        updated += 1;
    }
    Ok(updated)
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
        build_java_command, copy_instance, create_instance, default_launch_profile,
        delete_instance, format_s3_time, list_logs, list_mod_files, load_launch_profile,
        load_prism_instance_config, parse_s3_time, read_log_preview, rename_instance,
        save_launch_profile, scan_instances, sync_modrinth_managed_mods,
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
        fs::create_dir_all(created.path.join(".minecraft/mods")).expect("create dot minecraft mods");
        fs::write(
            created.path.join(".minecraft/mods").join("another.jar"),
            b"jar",
        )
        .expect("write second mod");
        fs::write(
            created.path.join("minecraft/mods").join("third.jar"),
            b"jar",
        )
        .expect("write third mod");
        fs::write(created.path.join("logs").join("latest.log"), b"hello log").expect("write log");

        let mods = list_mod_files(&created.path).expect("list mods");
        assert_eq!(
            mods,
            vec![
                "another.jar".to_string(),
                "example.jar".to_string(),
                "third.jar".to_string()
            ]
        );

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

    #[test]
    fn launch_profile_roundtrip_and_command_builder_work() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/tmp/prismarine_launcher_launch_test_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create root");

        let mut profile = default_launch_profile(&root);
        profile.java_path = "java".to_string();
        profile.main_class = "com.example.Main".to_string();
        profile.jvm_args = vec!["-Xmx1G".to_string()];
        profile.classpath = vec!["a.jar".to_string(), "b.jar".to_string()];
        profile.game_args = vec!["--demo".to_string()];
        save_launch_profile(&root, &profile).expect("save profile");

        let loaded = load_launch_profile(&root).expect("load profile");
        assert_eq!(loaded.main_class, "com.example.Main");

        let (_exe, args) = build_java_command(&loaded);
        assert!(args.iter().any(|x| x == "-cp"));
        assert!(args.iter().any(|x| x == "com.example.Main"));
        assert!(args.iter().any(|x| x == "--demo"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prism_instance_cfg_is_parsed() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/tmp/prismarine_launcher_cfg_test_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create root");

        fs::write(
            root.join("instance.cfg"),
            "\
iconKey=skyblock
IntendedVersion=1.20.1
OverrideJavaLocation=true
JavaPath=/usr/bin/java
OverrideJavaArgs=true
JvmArgs=-Xms512m -Xmx4096m
OverrideMemory=true
MinMemAlloc=512
MaxMemAlloc=4096
PermGen=128
OverrideCommands=true
PreLaunchCommand=echo pre
PostExitCommand=echo post
WrapperCommand=echo wrap
",
        )
        .expect("write cfg");

        let cfg = load_prism_instance_config(&root).expect("parse cfg");
        assert_eq!(cfg.icon_key.as_deref(), Some("skyblock"));
        assert_eq!(cfg.intended_version.as_deref(), Some("1.20.1"));
        assert!(cfg.override_java_location);
        assert_eq!(cfg.java_path.as_deref(), Some("/usr/bin/java"));
        assert!(cfg.override_java_args);
        assert!(cfg.override_memory);
        assert_eq!(cfg.min_mem_alloc, Some(512));
        assert_eq!(cfg.max_mem_alloc, Some(4096));
        assert_eq!(cfg.perm_gen, Some(128));
        assert!(cfg.override_commands);
        assert_eq!(cfg.pre_launch_command.as_deref(), Some("echo pre"));
        assert_eq!(cfg.post_exit_command.as_deref(), Some("echo post"));
        assert_eq!(cfg.wrapper_command.as_deref(), Some("echo wrap"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sync_modrinth_managed_mods_without_index_is_noop() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = PathBuf::from(format!(
            "/tmp/prismarine_launcher_modsync_noop_test_{}_{}",
            std::process::id(),
            nanos
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create root");

        let changed = sync_modrinth_managed_mods(&root).expect("sync");
        assert_eq!(changed, 0);

        let _ = fs::remove_dir_all(&root);
    }
}
