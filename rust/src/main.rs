use anyhow::Result;
use eframe::{App, Frame, NativeOptions, egui};
use rust_core::{
    LaunchProfile, ModrinthSearchHit, build_java_command, copy_instance, create_instance,
    default_launch_profile, delete_instance, download_file_to_path, format_s3_time, list_logs,
    list_mod_files, load_launch_profile, load_prism_instance_config, modrinth_resolve_primary_file,
    modrinth_search_projects, parse_s3_time, read_log_preview, rename_instance, scan_instances,
    validate_minecraft_account,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum CenterTab {
    Overview,
    Mods,
    Logs,
    Modrinth,
    Settings,
}

impl Default for CenterTab {
    fn default() -> Self {
        Self::Overview
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum SettingsSubTab {
    General,
    Java,
    Launch,
    UserCommands,
    Environment,
}

impl Default for SettingsSubTab {
    fn default() -> Self {
        Self::General
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum LaunchSettingsMode {
    Basic,
    Advanced,
}

impl Default for LaunchSettingsMode {
    fn default() -> Self {
        Self::Basic
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum AccountType {
    Offline,
    Licensed,
}

impl Default for AccountType {
    fn default() -> Self {
        Self::Offline
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Instance {
    name: String,
    version: String,
    running: bool,
    path: String,
    icon_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Account {
    name: String,
    active: bool,
    account_type: AccountType,
    access_token: Option<String>,
    licensed: bool,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            name: "Offline".to_string(),
            active: false,
            account_type: AccountType::Offline,
            access_token: None,
            licensed: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct GlobalLaunchSettings {
    mode: LaunchSettingsMode,
    java_path: String,
    skip_java_compat_check: bool,
    min_memory_mb: u32,
    max_memory_mb: u32,
    permgen_mb: u32,
    advanced_jvm_args: String,
    user_commands: String,
    environment_vars: String,
}

impl Default for GlobalLaunchSettings {
    fn default() -> Self {
        Self {
            mode: LaunchSettingsMode::Basic,
            java_path: "java".to_string(),
            skip_java_compat_check: false,
            min_memory_mb: 1024,
            max_memory_mb: 4096,
            permgen_mb: 128,
            advanced_jvm_args: "-Xms1G\n-Xmx4G".to_string(),
            user_commands: String::new(),
            environment_vars: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedState {
    #[serde(default)]
    selected: Option<usize>,
    #[serde(default = "default_true")]
    show_news: bool,
    #[serde(default)]
    filter: String,
    #[serde(default)]
    active_tab: CenterTab,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            selected: None,
            show_news: true,
            filter: String::new(),
            active_tab: CenterTab::Overview,
        }
    }
}

fn default_true() -> bool {
    true
}

fn local_data_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("PrismarineLauncher");
    }
    PathBuf::from(".").join(".local").join("PrismarineLauncher")
}

fn state_file_path() -> PathBuf {
    local_data_root().join("ui_state.json")
}

fn accounts_file_path() -> PathBuf {
    local_data_root().join("accounts.json")
}

fn global_settings_file_path() -> PathBuf {
    local_data_root().join("global_launch_settings.json")
}

fn instances_root_path() -> PathBuf {
    local_data_root().join("instances")
}

struct PrismarineApp {
    instances: Vec<Instance>,
    accounts: Vec<Account>,
    selected: Option<usize>,
    last_selected: Option<usize>,
    show_news: bool,
    status: String,
    filter: String,
    data_root: PathBuf,
    active_tab: CenterTab,
    show_create_dialog: bool,
    create_name: String,
    show_rename_dialog: bool,
    rename_name: String,
    show_copy_dialog: bool,
    copy_name: String,
    show_delete_dialog: bool,
    mods_cache: Vec<String>,
    logs_cache: Vec<(String, String)>,
    selected_log: Option<usize>,
    log_preview: String,
    launch_profile: LaunchProfile,
    processes: HashMap<String, Child>,
    show_add_account_dialog: bool,
    new_account_name: String,
    new_account_token: String,
    modrinth_query: String,
    modrinth_loader: String,
    modrinth_game_version: String,
    modrinth_hits: Vec<ModrinthSearchHit>,
    selected_modrinth_hit: Option<usize>,
    global_settings: GlobalLaunchSettings,
    settings_subtab: SettingsSubTab,
    icon_cache: HashMap<String, egui::TextureHandle>,
    post_exit_commands: HashMap<String, String>,
}

impl Default for PrismarineApp {
    fn default() -> Self {
        let persisted = load_state().unwrap_or_default();
        let data_root = local_data_root();
        let _ = fs::create_dir_all(instances_root_path());
        let mut accounts = load_accounts();
        if accounts.is_empty() {
            accounts = vec![
                Account {
                    name: "Default Account".to_string(),
                    active: true,
                    account_type: AccountType::Offline,
                    access_token: None,
                    licensed: false,
                },
                Account {
                    name: "Offline".to_string(),
                    active: false,
                    account_type: AccountType::Offline,
                    access_token: None,
                    licensed: false,
                },
            ];
        }
        let mut app = Self {
            instances: Vec::new(),
            accounts,
            selected: persisted.selected,
            last_selected: None,
            show_news: persisted.show_news,
            status: "Ready".to_string(),
            filter: persisted.filter,
            data_root,
            active_tab: persisted.active_tab,
            show_create_dialog: false,
            create_name: String::new(),
            show_rename_dialog: false,
            rename_name: String::new(),
            show_copy_dialog: false,
            copy_name: String::new(),
            show_delete_dialog: false,
            mods_cache: Vec::new(),
            logs_cache: Vec::new(),
            selected_log: None,
            log_preview: String::new(),
            launch_profile: default_launch_profile(Path::new(".")),
            processes: HashMap::new(),
            show_add_account_dialog: false,
            new_account_name: String::new(),
            new_account_token: String::new(),
            modrinth_query: String::new(),
            modrinth_loader: "fabric".to_string(),
            modrinth_game_version: String::new(),
            modrinth_hits: Vec::new(),
            selected_modrinth_hit: None,
            global_settings: load_global_settings(),
            settings_subtab: SettingsSubTab::General,
            icon_cache: HashMap::new(),
            post_exit_commands: HashMap::new(),
        };
        app.reload_instances();
        app
    }
}

impl PrismarineApp {
    fn selected_instance(&self) -> Option<&Instance> {
        self.selected.and_then(|i| self.instances.get(i))
    }

    fn active_account(&self) -> Option<&Account> {
        self.accounts.iter().find(|x| x.active)
    }

    fn apply_account_launch_args(&self, game_args: &mut Vec<String>) {
        remove_arg_pair(game_args, "--username");
        remove_arg_pair(game_args, "--uuid");
        remove_arg_pair(game_args, "--accessToken");
        remove_arg_pair(game_args, "--userType");
        remove_arg_pair(game_args, "--versionType");

        let account = self.active_account().cloned().unwrap_or(Account {
            name: "Player".to_string(),
            active: true,
            account_type: AccountType::Offline,
            access_token: None,
            licensed: false,
        });
        let username = account.name;
        let uuid = pseudo_uuid_from_name(&username);
        let (access_token, user_type) = if account.account_type == AccountType::Licensed {
            (
                account.access_token.unwrap_or_else(|| "0".to_string()),
                "msa".to_string(),
            )
        } else {
            ("0".to_string(), "offline".to_string())
        };

        game_args.push("--username".to_string());
        game_args.push(username);
        game_args.push("--uuid".to_string());
        game_args.push(uuid);
        game_args.push("--accessToken".to_string());
        game_args.push(access_token);
        game_args.push("--userType".to_string());
        game_args.push(user_type);
        game_args.push("--versionType".to_string());
        game_args.push("release".to_string());
    }

    fn selected_instance_path(&self) -> Option<PathBuf> {
        self.selected_instance().map(|x| PathBuf::from(&x.path))
    }

    fn instance_root(&self) -> PathBuf {
        self.data_root.join("instances")
    }

    fn set_active_account(&mut self, idx: usize) {
        for (i, account) in self.accounts.iter_mut().enumerate() {
            account.active = i == idx;
        }
        save_accounts(&self.accounts);
    }

    fn do_add_licensed_account(&mut self) {
        let name = self.new_account_name.trim();
        let token = self.new_account_token.trim();
        if name.is_empty() || token.is_empty() {
            self.status = "Account name and token are required".to_string();
            return;
        }

        match validate_minecraft_account(token) {
            Ok(validation) => {
                let account_name = if name.is_empty() {
                    validation.username
                } else {
                    name.to_string()
                };
                self.accounts.push(Account {
                    name: account_name.clone(),
                    active: false,
                    account_type: AccountType::Licensed,
                    access_token: Some(token.to_string()),
                    licensed: validation.has_minecraft_license,
                });
                self.status = if validation.has_minecraft_license {
                    format!("Licensed account added: {account_name}")
                } else {
                    format!("Account added but no Minecraft entitlement: {account_name}")
                };
                save_accounts(&self.accounts);
                self.new_account_name.clear();
                self.new_account_token.clear();
                self.show_add_account_dialog = false;
            }
            Err(err) => {
                self.status = format!("Account validation failed: {err}");
            }
        }
    }

    fn do_modrinth_search(&mut self) {
        match modrinth_search_projects(&self.modrinth_query, 20) {
            Ok(hits) => {
                self.modrinth_hits = hits;
                self.selected_modrinth_hit = if self.modrinth_hits.is_empty() {
                    None
                } else {
                    Some(0)
                };
                self.status = format!("Modrinth results: {}", self.modrinth_hits.len());
            }
            Err(err) => {
                self.status = format!("Modrinth search failed: {err}");
            }
        }
    }

    fn do_modrinth_download_selected(&mut self) {
        let Some(hit_idx) = self.selected_modrinth_hit else {
            self.status = "No Modrinth project selected".to_string();
            return;
        };
        if hit_idx >= self.modrinth_hits.len() {
            self.status = "Invalid Modrinth selection".to_string();
            return;
        }
        let Some(instance_path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        let hit = self.modrinth_hits[hit_idx].clone();

        match modrinth_resolve_primary_file(
            &hit.project_id,
            self.modrinth_game_version.trim(),
            self.modrinth_loader.trim(),
        ) {
            Ok(file) => {
                let target = instance_path.join("mods").join(&file.filename);
                match download_file_to_path(&file.url, &target) {
                    Ok(_) => {
                        self.status =
                            format!("Downloaded {} -> {}", file.filename, target.display());
                        self.refresh_selected_content();
                    }
                    Err(err) => {
                        self.status = format!("Failed to download file: {err}");
                    }
                }
            }
            Err(err) => {
                self.status = format!("Failed to resolve Modrinth file: {err}");
            }
        }
    }

    fn persist(&self) {
        let state = PersistedState {
            selected: self.selected,
            show_news: self.show_news,
            filter: self.filter.clone(),
            active_tab: self.active_tab.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            let path = state_file_path();
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, text);
        }
    }

    fn reload_instances(&mut self) {
        let root = self.instance_root();
        match scan_instances(&root) {
            Ok(items) => {
                self.instances = items
                    .into_iter()
                    .map(|item| {
                        let icon_a = item.path.join("icon.png");
                        let icon_b = item.path.join(".minecraft").join("icon.png");
                        let icon_path = if icon_a.is_file() {
                            Some(icon_a.display().to_string())
                        } else if icon_b.is_file() {
                            Some(icon_b.display().to_string())
                        } else {
                            None
                        };
                        Instance {
                            name: item.name,
                            version: "unknown".to_string(),
                            running: false,
                            path: item.path.display().to_string(),
                            icon_path,
                        }
                    })
                    .collect();
                if self.instances.is_empty() {
                    self.status =
                        format!("No instances found in {}", self.instance_root().display());
                    self.selected = None;
                } else {
                    self.status = format!("Loaded {} instances", self.instances.len());
                    if self.selected.is_none() || self.selected.unwrap_or(0) >= self.instances.len()
                    {
                        self.selected = Some(0);
                    }
                }
            }
            Err(err) => {
                self.instances.clear();
                self.selected = None;
                self.status = format!("Failed to scan {}: {}", self.instance_root().display(), err);
            }
        }
        self.refresh_selected_content();
    }

    fn refresh_selected_content(&mut self) {
        self.mods_cache.clear();
        self.logs_cache.clear();
        self.selected_log = None;
        self.log_preview.clear();

        let Some(path) = self.selected_instance_path() else {
            return;
        };

        match list_mod_files(&path) {
            Ok(mods) => {
                self.mods_cache = mods;
            }
            Err(err) => {
                self.status = format!("Failed to load mods: {err}");
            }
        }

        match list_logs(&path) {
            Ok(logs) => {
                self.logs_cache = logs
                    .into_iter()
                    .map(|x| (x.file_name, x.path.display().to_string()))
                    .collect();
            }
            Err(err) => {
                self.status = format!("Failed to load logs: {err}");
            }
        }

        match load_launch_profile(&path) {
            Ok(profile) => {
                self.launch_profile = profile;
            }
            Err(err) => {
                self.launch_profile = default_launch_profile(&path);
                self.status = format!("Failed to load launch profile: {err}");
            }
        }
    }

    fn sync_process_states(&mut self) {
        let keys: Vec<String> = self.processes.keys().cloned().collect();
        let mut finished = Vec::new();
        for key in keys {
            if let Some(child) = self.processes.get_mut(&key) {
                match child.try_wait() {
                    Ok(Some(_)) => finished.push(key),
                    Ok(None) => {}
                    Err(_) => finished.push(key),
                }
            }
        }
        for key in finished {
            self.processes.remove(&key);
            if let Some(post) = self.post_exit_commands.remove(&key) {
                let _ = Command::new("sh")
                    .arg("-lc")
                    .arg(post)
                    .current_dir(&key)
                    .status();
            }
        }
        for instance in &mut self.instances {
            instance.running = self.processes.contains_key(&instance.path);
        }
    }

    fn do_create_instance(&mut self) {
        let name = self.create_name.trim().to_string();
        if name.is_empty() {
            self.status = "Instance name must not be empty".to_string();
            return;
        }
        match create_instance(&self.instance_root(), &name) {
            Ok(_) => {
                self.status = format!("Created instance {name}");
                self.show_create_dialog = false;
                self.create_name.clear();
                self.reload_instances();
            }
            Err(err) => {
                self.status = format!("Failed to create instance: {err}");
            }
        }
    }

    fn do_rename_instance(&mut self) {
        let Some(path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        let new_name = self.rename_name.trim().to_string();
        if new_name.is_empty() {
            self.status = "New instance name must not be empty".to_string();
            return;
        }

        match rename_instance(&path, &new_name) {
            Ok(_) => {
                self.status = format!("Renamed instance to {new_name}");
                self.show_rename_dialog = false;
                self.reload_instances();
                if let Some(pos) = self.instances.iter().position(|x| x.name == new_name) {
                    self.selected = Some(pos);
                }
            }
            Err(err) => {
                self.status = format!("Failed to rename instance: {err}");
            }
        }
    }

    fn do_copy_instance(&mut self) {
        let Some(instance) = self.selected_instance().cloned() else {
            self.status = "No instance selected".to_string();
            return;
        };
        let new_name = self.copy_name.trim().to_string();
        if new_name.is_empty() {
            self.status = "Copy name must not be empty".to_string();
            return;
        }

        match copy_instance(Path::new(&instance.path), &self.instance_root(), &new_name) {
            Ok(_) => {
                self.status = format!("Copied instance to {new_name}");
                self.show_copy_dialog = false;
                self.reload_instances();
                if let Some(pos) = self.instances.iter().position(|x| x.name == new_name) {
                    self.selected = Some(pos);
                }
            }
            Err(err) => {
                self.status = format!("Failed to copy instance: {err}");
            }
        }
    }

    fn do_delete_instance(&mut self) {
        let Some(instance) = self.selected_instance().cloned() else {
            self.status = "No instance selected".to_string();
            return;
        };

        if self.processes.contains_key(&instance.path) {
            self.do_kill_instance();
        }

        match delete_instance(Path::new(&instance.path)) {
            Ok(_) => {
                self.status = format!("Deleted {}", instance.name);
                self.show_delete_dialog = false;
                self.reload_instances();
            }
            Err(err) => {
                self.status = format!("Failed to delete {}: {err}", instance.name);
            }
        }
    }

    fn do_launch_instance(&mut self) {
        let Some(instance) = self.selected_instance().cloned() else {
            self.status = "No instance selected".to_string();
            return;
        };
        if self.processes.contains_key(&instance.path) {
            self.status = format!("{} is already running", instance.name);
            return;
        }

        let instance_path = PathBuf::from(&instance.path);
        let prism_cfg = load_prism_instance_config(&instance_path).unwrap_or_default();
        let mut profile = self.launch_profile.clone();
        profile.java_path = self.global_settings.java_path.clone();
        match self.global_settings.mode {
            LaunchSettingsMode::Basic => {
                let min_mb = self.global_settings.min_memory_mb.max(256);
                let max_mb = self.global_settings.max_memory_mb.max(min_mb);
                profile.jvm_args = vec![format!("-Xms{}M", min_mb), format!("-Xmx{}M", max_mb)];
            }
            LaunchSettingsMode::Advanced => {
                profile.jvm_args = self
                    .global_settings
                    .advanced_jvm_args
                    .lines()
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .map(ToString::to_string)
                    .collect();
            }
        }
        if prism_cfg.override_memory {
            if let Some(min_mb) = prism_cfg.min_mem_alloc {
                profile.jvm_args.retain(|x| !x.starts_with("-Xms"));
                profile.jvm_args.push(format!("-Xms{}M", min_mb.max(256)));
            }
            if let Some(max_mb) = prism_cfg.max_mem_alloc {
                profile.jvm_args.retain(|x| !x.starts_with("-Xmx"));
                profile.jvm_args.push(format!("-Xmx{}M", max_mb.max(256)));
            }
        }
        if self.global_settings.permgen_mb > 0 {
            profile
                .jvm_args
                .push(format!("-XX:PermSize={}M", self.global_settings.permgen_mb));
        }
        if let Some(perm) = prism_cfg.perm_gen {
            profile.jvm_args.retain(|x| !x.starts_with("-XX:PermSize="));
            profile.jvm_args.push(format!("-XX:PermSize={}M", perm));
        }
        if prism_cfg.override_java_args
            && let Some(args) = prism_cfg.java_args.clone()
        {
            profile.jvm_args = args.split_whitespace().map(ToString::to_string).collect();
        }
        if prism_cfg.override_java_location
            && let Some(java) = prism_cfg.java_path.clone()
        {
            profile.java_path = java;
        }
        if profile.working_dir.trim().is_empty() {
            profile.working_dir = instance.path.clone();
        }
        self.apply_account_launch_args(&mut profile.game_args);
        let (exe, args) = build_java_command(&profile);

        let logs_dir = instance_path.join("logs");
        let _ = fs::create_dir_all(&logs_dir);
        let log_path = logs_dir.join("latest.log");
        let mut log_file = match fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            Ok(file) => file,
            Err(err) => {
                self.status = format!("Failed to open log file: {err}");
                return;
            }
        };
        let _ = writeln!(log_file, "=== launch: {} {} ===", exe, args.join(" "));
        let stdout_log = match log_file.try_clone() {
            Ok(f) => f,
            Err(err) => {
                self.status = format!("Failed to clone log handle: {err}");
                return;
            }
        };
        let stderr_log = match log_file.try_clone() {
            Ok(f) => f,
            Err(err) => {
                self.status = format!("Failed to clone log handle: {err}");
                return;
            }
        };

        let working_dir = PathBuf::from(&profile.working_dir);
        let pre_commands: Vec<String> = if prism_cfg.override_commands {
            prism_cfg.pre_launch_command.into_iter().collect()
        } else {
            self.global_settings
                .user_commands
                .lines()
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(ToString::to_string)
                .collect()
        };

        for cmd_line in pre_commands {
            let _ = Command::new("sh")
                .arg("-lc")
                .arg(cmd_line)
                .current_dir(&working_dir)
                .status();
        }
        let launch_line = format!(
            "{} {}",
            shell_escape(&exe),
            args.iter()
                .map(|a| shell_escape(a))
                .collect::<Vec<_>>()
                .join(" ")
        );

        let mut cmd = if prism_cfg.override_commands {
            if let Some(wrapper) = prism_cfg.wrapper_command {
                let mut c = Command::new("sh");
                c.arg("-lc").arg(format!("{wrapper} {launch_line}"));
                c
            } else {
                let mut c = Command::new(exe);
                c.args(args);
                c
            }
        } else {
            let mut c = Command::new(exe);
            c.args(args);
            c
        };

        if prism_cfg.override_commands
            && let Some(post) = prism_cfg.post_exit_command.clone()
            && !post.trim().is_empty()
        {
            self.post_exit_commands.insert(instance.path.clone(), post);
        } else {
            self.post_exit_commands.remove(&instance.path);
        }

        cmd.current_dir(working_dir)
            .stdout(Stdio::from(stdout_log))
            .stderr(Stdio::from(stderr_log));
        for line in self
            .global_settings
            .environment_vars
            .lines()
            .map(str::trim)
            .filter(|x| !x.is_empty())
        {
            if let Some((k, v)) = line.split_once('=') {
                cmd.env(k.trim(), v.trim());
            }
        }

        match cmd.spawn() {
            Ok(child) => {
                self.processes.insert(instance.path.clone(), child);
                self.status = format!("Launched {}", instance.name);
                self.sync_process_states();
            }
            Err(err) => {
                self.status = format!("Failed to launch {}: {}", instance.name, err);
            }
        }
    }

    fn do_kill_instance(&mut self) {
        let Some(instance) = self.selected_instance().cloned() else {
            self.status = "No instance selected".to_string();
            return;
        };

        if let Some(mut child) = self.processes.remove(&instance.path) {
            match child.kill() {
                Ok(_) => {
                    let _ = child.wait();
                    if let Some(post) = self.post_exit_commands.remove(&instance.path) {
                        let _ = Command::new("sh")
                            .arg("-lc")
                            .arg(post)
                            .current_dir(&instance.path)
                            .status();
                    }
                    self.status = format!("Stopped {}", instance.name);
                }
                Err(err) => {
                    self.status = format!("Failed to stop {}: {}", instance.name, err);
                }
            }
        } else {
            self.status = format!("{} is not running", instance.name);
            self.post_exit_commands.remove(&instance.path);
        }
        self.sync_process_states();
    }

    fn open_create_dialog(&mut self) {
        self.create_name.clear();
        self.show_create_dialog = true;
    }

    fn open_rename_dialog(&mut self) {
        if let Some(instance) = self.selected_instance() {
            self.rename_name = instance.name.clone();
            self.show_rename_dialog = true;
        } else {
            self.status = "No instance selected".to_string();
        }
    }

    fn open_copy_dialog(&mut self) {
        if let Some(instance) = self.selected_instance() {
            self.copy_name = format!("{} Copy", instance.name);
            self.show_copy_dialog = true;
        } else {
            self.status = "No instance selected".to_string();
        }
    }

    fn open_delete_dialog(&mut self) {
        if self.selected_instance().is_some() {
            self.show_delete_dialog = true;
        } else {
            self.status = "No instance selected".to_string();
        }
    }

    fn open_selected_log_preview(&mut self) {
        let Some(idx) = self.selected_log else {
            return;
        };
        if idx >= self.logs_cache.len() {
            return;
        }
        let path = self.logs_cache[idx].1.clone();
        match read_log_preview(Path::new(&path), 20_000) {
            Ok(text) => {
                self.log_preview = text;
            }
            Err(err) => {
                self.log_preview.clear();
                self.status = format!("Failed to read log: {err}");
            }
        }
    }

    fn top_menu(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Add Instance").clicked() {
                    self.open_create_dialog();
                    ui.close();
                }
                if ui.button("Launch").clicked() {
                    self.do_launch_instance();
                    ui.close();
                }
                if ui.button("Kill").clicked() {
                    self.do_kill_instance();
                    ui.close();
                }
            });

            ui.menu_button("Edit", |ui| {
                if ui.button("Rename Instance").clicked() {
                    self.open_rename_dialog();
                    ui.close();
                }
                if ui.button("Copy Instance").clicked() {
                    self.open_copy_dialog();
                    ui.close();
                }
                if ui.button("Delete Instance").clicked() {
                    self.open_delete_dialog();
                    ui.close();
                }
            });

            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.show_news, "Show News Bar");
            });

            ui.menu_button("Folders", |ui| {
                ui.label(format!("Data root: {}", self.data_root.display()));
                ui.label(format!("Instances: {}", self.instance_root().display()));
                if ui.button("Reload Instances").clicked() {
                    self.reload_instances();
                    ui.close();
                }
            });

            ui.menu_button("Accounts", |ui| {
                for idx in 0..self.accounts.len() {
                    let active = self.accounts[idx].active;
                    let mut name = self.accounts[idx].name.clone();
                    if self.accounts[idx].account_type == AccountType::Licensed {
                        if self.accounts[idx].licensed {
                            name.push_str(" [licensed]");
                        } else {
                            name.push_str(" [unverified]");
                        }
                    }
                    if ui.selectable_label(active, name).clicked() {
                        self.set_active_account(idx);
                    }
                }
                ui.separator();
                if ui.button("Add Licensed Account").clicked() {
                    self.show_add_account_dialog = true;
                    ui.close();
                }
            });

            ui.menu_button("Help", |ui| {
                ui.label("PrismarineLauncher Rust UI migration build");
                ui.label("Goal: parity with existing Qt interface and behavior");
            });
        });
    }

    fn ensure_icon_texture(
        &mut self,
        ctx: &egui::Context,
        icon_path: &str,
    ) -> Option<egui::TextureHandle> {
        if let Some(tex) = self.icon_cache.get(icon_path) {
            return Some(tex.clone());
        }

        let bytes = fs::read(icon_path).ok()?;
        let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let size = [decoded.width() as usize, decoded.height() as usize];
        if size[0] == 0 || size[1] == 0 {
            return None;
        }
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, decoded.as_raw());
        let texture = ctx.load_texture(
            format!("instance_icon::{icon_path}"),
            color_image,
            egui::TextureOptions::LINEAR,
        );
        self.icon_cache
            .insert(icon_path.to_string(), texture.clone());
        Some(texture)
    }

    fn draw_instance_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.filter);
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for idx in 0..self.instances.len() {
                let instance = self.instances[idx].clone();
                if !self.filter.is_empty()
                    && !instance
                        .name
                        .to_lowercase()
                        .contains(&self.filter.to_lowercase())
                {
                    continue;
                }

                let selected = self.selected == Some(idx);
                let status = if instance.running {
                    "running"
                } else {
                    "stopped"
                };
                let line = format!("{} [{} | {}]", instance.name, instance.version, status);
                let row_height = ui.text_style_height(&egui::TextStyle::Body).max(18.0);
                ui.horizontal(|ui| {
                    if let Some(icon_path) = &instance.icon_path {
                        if let Some(tex) = self.ensure_icon_texture(ctx, icon_path) {
                            ui.image((tex.id(), egui::vec2(row_height, row_height)));
                        } else {
                            ui.add_space(row_height);
                        }
                    } else {
                        ui.add_space(row_height);
                    }
                    if ui.selectable_label(selected, line).clicked() {
                        self.selected = Some(idx);
                    }
                });
            }
        });
    }

    fn tab_selector(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.active_tab, CenterTab::Overview, "Overview");
            ui.selectable_value(&mut self.active_tab, CenterTab::Mods, "Mods");
            ui.selectable_value(&mut self.active_tab, CenterTab::Logs, "Logs");
            ui.selectable_value(&mut self.active_tab, CenterTab::Modrinth, "Modrinth");
            ui.selectable_value(&mut self.active_tab, CenterTab::Settings, "Settings");
        });
        ui.separator();
    }

    fn draw_overview_tab(&mut self, ui: &mut egui::Ui) {
        if let Some(instance) = self.selected_instance() {
            ui.heading(&instance.name);
            ui.label(format!("Path: {}", instance.path));
            ui.label(format!("Version: {}", instance.version));
            ui.label(format!(
                "State: {}",
                if instance.running {
                    "running"
                } else {
                    "stopped"
                }
            ));
        } else {
            ui.label("No instance selected");
        }

        ui.separator();

        if let Some((ms, offset)) = parse_s3_time("2016-02-29T13:49:54+01:00")
            && let Some(serialized) = format_s3_time(ms, offset)
        {
            ui.label(format!("Core timestamp parity: {}", serialized));
        }
    }

    fn draw_mods_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh Mods").clicked() {
                self.refresh_selected_content();
            }
            ui.label(format!("Total: {}", self.mods_cache.len()));
        });
        ui.separator();

        if self.mods_cache.is_empty() {
            ui.label("No mods found");
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for item in &self.mods_cache {
                ui.monospace(item);
            }
        });
    }

    fn draw_logs_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Refresh Logs").clicked() {
                self.refresh_selected_content();
            }
            if ui.button("Open Selected Preview").clicked() {
                self.open_selected_log_preview();
            }
        });
        ui.separator();

        ui.columns(2, |cols| {
            cols[0].label("Log files");
            cols[0].separator();
            egui::ScrollArea::vertical().show(&mut cols[0], |ui| {
                let mut clicked_index = None;
                for (idx, (name, _)) in self.logs_cache.iter().enumerate() {
                    let selected = self.selected_log == Some(idx);
                    if ui.selectable_label(selected, name).clicked() {
                        clicked_index = Some(idx);
                    }
                }
                if let Some(idx) = clicked_index {
                    self.selected_log = Some(idx);
                    self.open_selected_log_preview();
                }
            });

            cols[1].label("Preview");
            cols[1].separator();
            egui::ScrollArea::vertical().show(&mut cols[1], |ui| {
                if self.log_preview.is_empty() {
                    ui.label("No log selected");
                } else {
                    ui.monospace(&self.log_preview);
                }
            });
        });
    }

    fn draw_modrinth_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Query:");
            ui.text_edit_singleline(&mut self.modrinth_query);
            if ui.button("Search").clicked() {
                self.do_modrinth_search();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Loader:");
            ui.text_edit_singleline(&mut self.modrinth_loader);
            ui.label("Game version:");
            ui.text_edit_singleline(&mut self.modrinth_game_version);
        });
        if ui.button("Download Selected To mods/").clicked() {
            self.do_modrinth_download_selected();
        }
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (idx, hit) in self.modrinth_hits.iter().enumerate() {
                let selected = self.selected_modrinth_hit == Some(idx);
                let label = format!("{} ({})", hit.title, hit.slug);
                if ui.selectable_label(selected, label).clicked() {
                    self.selected_modrinth_hit = Some(idx);
                }
            }
        });
    }

    fn draw_settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Параметры");
        ui.horizontal(|ui| {
            if ui.button("Открыть глобальные параметры").clicked() {
                self.status = "Глобальные параметры уже открыты справа.".to_string();
            }
            ui.label("Эти параметры переопределяют настройки экземпляра.");
        });
        ui.separator();

        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.settings_subtab, SettingsSubTab::General, "Общие");
            ui.selectable_value(&mut self.settings_subtab, SettingsSubTab::Java, "Java");
            ui.selectable_value(
                &mut self.settings_subtab,
                SettingsSubTab::Launch,
                "Настройки",
            );
            ui.selectable_value(
                &mut self.settings_subtab,
                SettingsSubTab::UserCommands,
                "Пользовательские команды",
            );
            ui.selectable_value(
                &mut self.settings_subtab,
                SettingsSubTab::Environment,
                "Переменные окружения",
            );
        });
        ui.separator();

        let mut changed = false;

        match self.settings_subtab {
            SettingsSubTab::General => {
                ui.group(|ui| {
                    ui.label(format!("Хранилище: {}", self.data_root.display()));
                    ui.label(format!("Экземпляры: {}", self.instance_root().display()));
                    ui.label(
                        "Все аккаунты и экземпляры хранятся в ~/.local/share/PrismarineLauncher.",
                    );
                });
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label("Режим настроек запуска");
                    changed |= ui
                        .selectable_value(
                            &mut self.global_settings.mode,
                            LaunchSettingsMode::Basic,
                            "Обычные",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut self.global_settings.mode,
                            LaunchSettingsMode::Advanced,
                            "Advanced",
                        )
                        .changed();
                    ui.label("В обычном режиме доступны минимальная/максимальная память.");
                });
            }
            SettingsSubTab::Java => {
                ui.group(|ui| {
                    ui.label("Установка Java");
                    ui.label("Исполняемый файл Java");
                    changed |= ui
                        .text_edit_singleline(&mut self.global_settings.java_path)
                        .changed();
                    ui.horizontal(|ui| {
                        if ui.button("Найти").clicked() {
                            self.status =
                                "Автопоиск Java будет добавлен следующим шагом.".to_string();
                        }
                        if ui.button("Обзор").clicked() {
                            self.status =
                                "Выбор файла Java через диалог будет добавлен следующим шагом."
                                    .to_string();
                        }
                    });
                    changed |= ui
                        .checkbox(
                            &mut self.global_settings.skip_java_compat_check,
                            "Пропустить проверку совместимости Java",
                        )
                        .changed();
                    if ui.button("Проверить настройки").clicked() {
                        self.status =
                            format!("Проверка Java: {}", self.global_settings.java_path.trim());
                    }
                });
            }
            SettingsSubTab::Launch => {
                ui.group(|ui| {
                    ui.label("Память");
                    ui.horizontal(|ui| {
                        ui.label("Минимальное использование памяти:");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.global_settings.min_memory_mb)
                                    .speed(64)
                                    .range(256..=131072),
                            )
                            .changed();
                        ui.label("MiB (-Xms)");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Максимальное использование памяти:");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.global_settings.max_memory_mb)
                                    .speed(64)
                                    .range(256..=131072),
                            )
                            .changed();
                        ui.label("MiB (-Xmx)");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Размер PermGen:");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.global_settings.permgen_mb)
                                    .speed(16)
                                    .range(0..=4096),
                            )
                            .changed();
                        ui.label("MiB (-XX:PermSize)");
                    });
                    if self.global_settings.max_memory_mb < self.global_settings.min_memory_mb {
                        self.global_settings.max_memory_mb = self.global_settings.min_memory_mb;
                        changed = true;
                    }
                });
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label("Аргументы Java");
                    changed |= ui
                        .add(
                            egui::TextEdit::multiline(&mut self.global_settings.advanced_jvm_args)
                                .desired_rows(8),
                        )
                        .changed();
                    ui.label("Используются в режиме Advanced.");
                });
            }
            SettingsSubTab::UserCommands => {
                ui.group(|ui| {
                    ui.label("Пользовательские команды (по одной на строку)");
                    changed |= ui
                        .add(
                            egui::TextEdit::multiline(&mut self.global_settings.user_commands)
                                .desired_rows(10),
                        )
                        .changed();
                });
            }
            SettingsSubTab::Environment => {
                ui.group(|ui| {
                    ui.label("Переменные окружения (формат KEY=VALUE, по одной на строку)");
                    changed |= ui
                        .add(
                            egui::TextEdit::multiline(&mut self.global_settings.environment_vars)
                                .desired_rows(10),
                        )
                        .changed();
                });
            }
        }

        if changed {
            save_global_settings(&self.global_settings);
        }
    }

    fn draw_dialogs(&mut self, ctx: &egui::Context) {
        if self.show_create_dialog {
            egui::Window::new("Create Instance")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Instance name:");
                    ui.text_edit_singleline(&mut self.create_name);
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            self.do_create_instance();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_create_dialog = false;
                        }
                    });
                });
        }

        if self.show_rename_dialog {
            egui::Window::new("Rename Instance")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("New name:");
                    ui.text_edit_singleline(&mut self.rename_name);
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            self.do_rename_instance();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_rename_dialog = false;
                        }
                    });
                });
        }

        if self.show_copy_dialog {
            egui::Window::new("Copy Instance")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("New copy name:");
                    ui.text_edit_singleline(&mut self.copy_name);
                    ui.horizontal(|ui| {
                        if ui.button("Copy").clicked() {
                            self.do_copy_instance();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_copy_dialog = false;
                        }
                    });
                });
        }

        if self.show_delete_dialog {
            let name = self
                .selected_instance()
                .map(|x| x.name.clone())
                .unwrap_or_else(|| "selected instance".to_string());
            egui::Window::new("Delete Instance")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label(format!("Delete {name}?"));
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            self.do_delete_instance();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_delete_dialog = false;
                        }
                    });
                });
        }

        if self.show_add_account_dialog {
            egui::Window::new("Add Licensed Account")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Display name:");
                    ui.text_edit_singleline(&mut self.new_account_name);
                    ui.label("Access token (Minecraft Services):");
                    ui.add(egui::TextEdit::singleline(&mut self.new_account_token).password(true));
                    ui.horizontal(|ui| {
                        if ui.button("Validate + Add").clicked() {
                            self.do_add_licensed_account();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_add_account_dialog = false;
                        }
                    });
                });
        }
    }
}

impl App for PrismarineApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.sync_process_states();

        if self.last_selected != self.selected {
            self.last_selected = self.selected;
            self.refresh_selected_content();
        }

        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            self.top_menu(ui);
        });

        egui::TopBottomPanel::top("main_toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Add Instance").clicked() {
                    self.open_create_dialog();
                }
                if ui.button("Folders/Reload").clicked() {
                    self.reload_instances();
                }
                if ui.button("Settings").clicked() {
                    self.active_tab = CenterTab::Settings;
                }
                if ui.button("Help").clicked() {
                    self.status = "Open help action".to_string();
                }
                if ui.button("Check Update").clicked() {
                    self.status = "Checking updates".to_string();
                }
                ui.separator();
                ui.monospace(format!("Storage: {}", self.instance_root().display()));
            });
        });

        if self.show_news {
            egui::TopBottomPanel::bottom("news_toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Latest news is available");
                    if ui.link("More News...").clicked() {
                        self.status = "Open news page".to_string();
                    }
                });
            });
        }

        egui::SidePanel::right("instance_toolbar")
            .resizable(false)
            .default_width(180.0)
            .show(ctx, |ui| {
                let has_selected = self.selected_instance().is_some();
                if ui
                    .add_enabled(has_selected, egui::Button::new("Launch"))
                    .clicked()
                {
                    self.do_launch_instance();
                }
                if ui
                    .add_enabled(has_selected, egui::Button::new("Kill"))
                    .clicked()
                {
                    self.do_kill_instance();
                }
                ui.separator();
                if ui
                    .add_enabled(has_selected, egui::Button::new("Rename"))
                    .clicked()
                {
                    self.open_rename_dialog();
                }
                if ui
                    .add_enabled(has_selected, egui::Button::new("Copy"))
                    .clicked()
                {
                    self.open_copy_dialog();
                }
                if ui
                    .add_enabled(has_selected, egui::Button::new("Delete"))
                    .clicked()
                {
                    self.open_delete_dialog();
                }
                if ui.button("Reload").clicked() {
                    self.reload_instances();
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.columns(2, |cols| {
                cols[0].heading("Instances");
                cols[0].separator();
                self.draw_instance_list(&mut cols[0], ctx);

                cols[1].heading("Instance Details");
                cols[1].separator();
                self.tab_selector(&mut cols[1]);
                match self.active_tab {
                    CenterTab::Overview => self.draw_overview_tab(&mut cols[1]),
                    CenterTab::Mods => self.draw_mods_tab(&mut cols[1]),
                    CenterTab::Logs => self.draw_logs_tab(&mut cols[1]),
                    CenterTab::Modrinth => self.draw_modrinth_tab(&mut cols[1]),
                    CenterTab::Settings => self.draw_settings_tab(&mut cols[1]),
                }
            });
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Status:");
                ui.monospace(&self.status);
            });
        });

        self.draw_dialogs(ctx);
        self.persist();
    }
}

fn load_state() -> Option<PersistedState> {
    let text = fs::read_to_string(state_file_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn shell_escape(input: &str) -> String {
    if input
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=+".contains(c))
    {
        return input.to_string();
    }
    let escaped = input.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn remove_arg_pair(args: &mut Vec<String>, key: &str) {
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == key {
            args.remove(i);
            if i < args.len() {
                args.remove(i);
            }
        } else {
            i += 1;
        }
    }
}

fn pseudo_uuid_from_name(name: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h1);
    let a = h1.finish();
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    format!("{}::prismarine", name).hash(&mut h2);
    let b = h2.finish();
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16,
        (b >> 48) as u16,
        (b & 0x0000_FFFF_FFFF_FFFF) as u64
    )
}

fn load_accounts() -> Vec<Account> {
    let path = accounts_file_path();
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<Account>>(&text).unwrap_or_default()
}

fn save_accounts(accounts: &[Account]) {
    let path = accounts_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(accounts) {
        let _ = fs::write(path, text);
    }
}

fn load_global_settings() -> GlobalLaunchSettings {
    let path = global_settings_file_path();
    let Ok(text) = fs::read_to_string(path) else {
        return GlobalLaunchSettings::default();
    };
    serde_json::from_str::<GlobalLaunchSettings>(&text).unwrap_or_default()
}

fn save_global_settings(settings: &GlobalLaunchSettings) {
    let path = global_settings_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(path, text);
    }
}

fn main() -> Result<()> {
    let options = NativeOptions::default();
    eframe::run_native(
        "PrismarineLauncher (Rust UI)",
        options,
        Box::new(|_cc| Ok(Box::<PrismarineApp>::default())),
    )
    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(())
}
