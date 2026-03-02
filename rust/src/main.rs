use anyhow::Result;
use eframe::{App, Frame, NativeOptions, egui};
use rust_core::{
    LaunchProfile, ModrinthSearchHit, build_java_command, copy_instance, create_instance,
    default_launch_profile, delete_instance, download_file_to_path, format_s3_time, list_logs,
    list_mod_files, load_launch_profile, modrinth_resolve_primary_file, modrinth_search_projects,
    parse_s3_time, read_log_preview, rename_instance, save_launch_profile, scan_instances,
    validate_minecraft_account,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const STATE_FILE: &str = "rust/.state.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum CenterTab {
    Overview,
    Mods,
    Logs,
    Modrinth,
    Settings,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum AccountType {
    Offline,
    Licensed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Instance {
    name: String,
    version: String,
    running: bool,
    path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Account {
    name: String,
    active: bool,
    account_type: AccountType,
    access_token: Option<String>,
    licensed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedState {
    data_dir: String,
    selected: Option<usize>,
    show_news: bool,
    filter: String,
    active_tab: CenterTab,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            data_dir: "instances".to_string(),
            selected: None,
            show_news: true,
            filter: String::new(),
            active_tab: CenterTab::Overview,
        }
    }
}

struct PrismarineApp {
    instances: Vec<Instance>,
    accounts: Vec<Account>,
    selected: Option<usize>,
    last_selected: Option<usize>,
    show_news: bool,
    status: String,
    filter: String,
    data_dir: String,
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
    launch_profile_dirty: bool,
    processes: HashMap<String, Child>,
    show_add_account_dialog: bool,
    new_account_name: String,
    new_account_token: String,
    modrinth_query: String,
    modrinth_loader: String,
    modrinth_game_version: String,
    modrinth_hits: Vec<ModrinthSearchHit>,
    selected_modrinth_hit: Option<usize>,
}

impl Default for PrismarineApp {
    fn default() -> Self {
        let persisted = load_state().unwrap_or_default();
        let mut app = Self {
            instances: Vec::new(),
            accounts: vec![
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
            ],
            selected: persisted.selected,
            last_selected: None,
            show_news: persisted.show_news,
            status: "Ready".to_string(),
            filter: persisted.filter,
            data_dir: persisted.data_dir,
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
            launch_profile_dirty: false,
            processes: HashMap::new(),
            show_add_account_dialog: false,
            new_account_name: String::new(),
            new_account_token: String::new(),
            modrinth_query: String::new(),
            modrinth_loader: "fabric".to_string(),
            modrinth_game_version: String::new(),
            modrinth_hits: Vec::new(),
            selected_modrinth_hit: None,
        };
        app.reload_instances();
        app
    }
}

impl PrismarineApp {
    fn selected_instance(&self) -> Option<&Instance> {
        self.selected.and_then(|i| self.instances.get(i))
    }

    fn selected_instance_path(&self) -> Option<PathBuf> {
        self.selected_instance().map(|x| PathBuf::from(&x.path))
    }

    fn instance_root(&self) -> PathBuf {
        PathBuf::from(&self.data_dir)
    }

    fn set_active_account(&mut self, idx: usize) {
        for (i, account) in self.accounts.iter_mut().enumerate() {
            account.active = i == idx;
        }
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
            data_dir: self.data_dir.clone(),
            selected: self.selected,
            show_news: self.show_news,
            filter: self.filter.clone(),
            active_tab: self.active_tab.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            if let Some(parent) = Path::new(STATE_FILE).parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(STATE_FILE, text);
        }
    }

    fn reload_instances(&mut self) {
        let root = self.instance_root();
        match scan_instances(&root) {
            Ok(items) => {
                self.instances = items
                    .into_iter()
                    .map(|item| Instance {
                        name: item.name,
                        version: "unknown".to_string(),
                        running: false,
                        path: item.path.display().to_string(),
                    })
                    .collect();
                if self.instances.is_empty() {
                    self.status = format!("No instances found in {}", self.data_dir);
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
                self.status = format!("Failed to scan {}: {}", self.data_dir, err);
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
                self.launch_profile_dirty = false;
            }
            Err(err) => {
                self.launch_profile = default_launch_profile(&path);
                self.launch_profile_dirty = false;
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
        let mut profile = self.launch_profile.clone();
        if profile.working_dir.trim().is_empty() {
            profile.working_dir = instance.path.clone();
        }
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
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .current_dir(working_dir)
            .stdout(Stdio::from(stdout_log))
            .stderr(Stdio::from(stderr_log));

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
                    self.status = format!("Stopped {}", instance.name);
                }
                Err(err) => {
                    self.status = format!("Failed to stop {}: {}", instance.name, err);
                }
            }
        } else {
            self.status = format!("{} is not running", instance.name);
        }
        self.sync_process_states();
    }

    fn save_current_launch_profile(&mut self) {
        let Some(path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        match save_launch_profile(&path, &self.launch_profile) {
            Ok(_) => {
                self.launch_profile_dirty = false;
                self.status = "Launch profile saved".to_string();
            }
            Err(err) => {
                self.status = format!("Failed to save launch profile: {err}");
            }
        }
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
                ui.label("Data dir:");
                ui.text_edit_singleline(&mut self.data_dir);
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

    fn draw_instance_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.filter);
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (idx, instance) in self.instances.iter().enumerate() {
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
                if ui.selectable_label(selected, line).clicked() {
                    self.selected = Some(idx);
                }
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
        let Some(instance) = self.selected_instance() else {
            ui.label("No instance selected");
            return;
        };

        ui.label("Instance Launch Settings");
        ui.separator();
        ui.label(format!("Name: {}", instance.name));
        ui.label(format!("Path: {}", instance.path));
        ui.separator();

        ui.label("Java executable:");
        if ui
            .text_edit_singleline(&mut self.launch_profile.java_path)
            .changed()
        {
            self.launch_profile_dirty = true;
        }
        ui.label("Main class:");
        if ui
            .text_edit_singleline(&mut self.launch_profile.main_class)
            .changed()
        {
            self.launch_profile_dirty = true;
        }
        ui.label("Working directory:");
        if ui
            .text_edit_singleline(&mut self.launch_profile.working_dir)
            .changed()
        {
            self.launch_profile_dirty = true;
        }

        ui.separator();
        ui.label("JVM args (one per line):");
        let mut jvm_args_text = self.launch_profile.jvm_args.join("\n");
        if ui
            .add(egui::TextEdit::multiline(&mut jvm_args_text).desired_rows(4))
            .changed()
        {
            self.launch_profile.jvm_args = jvm_args_text
                .lines()
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(ToString::to_string)
                .collect();
            self.launch_profile_dirty = true;
        }

        ui.label("Game args (one per line):");
        let mut game_args_text = self.launch_profile.game_args.join("\n");
        if ui
            .add(egui::TextEdit::multiline(&mut game_args_text).desired_rows(5))
            .changed()
        {
            self.launch_profile.game_args = game_args_text
                .lines()
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(ToString::to_string)
                .collect();
            self.launch_profile_dirty = true;
        }

        ui.horizontal(|ui| {
            if ui.button("Save Launch Profile").clicked() {
                self.save_current_launch_profile();
            }
            if ui.button("Reload Launch Profile").clicked() {
                self.refresh_selected_content();
            }
            if self.launch_profile_dirty {
                ui.colored_label(egui::Color32::YELLOW, "Unsaved changes");
            }
        });
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
                ui.label("Data dir:");
                let response = ui.text_edit_singleline(&mut self.data_dir);
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.reload_instances();
                }
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
                self.draw_instance_list(&mut cols[0]);

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
    let text = fs::read_to_string(STATE_FILE).ok()?;
    serde_json::from_str(&text).ok()
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
