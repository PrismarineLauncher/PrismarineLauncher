use anyhow::Result;
use eframe::{App, Frame, NativeOptions, egui};
use rust_core::{
    copy_instance, create_instance, delete_instance, format_s3_time, list_logs, list_mod_files,
    parse_s3_time, read_log_preview, rename_instance, scan_instances,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const STATE_FILE: &str = "rust/ui/.state.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum CenterTab {
    Overview,
    Mods,
    Logs,
    Settings,
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
                },
                Account {
                    name: "Offline".to_string(),
                    active: false,
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
        };
        app.reload_instances();
        app
    }
}

impl PrismarineApp {
    fn selected_instance_mut(&mut self) -> Option<&mut Instance> {
        self.selected.and_then(|i| self.instances.get_mut(i))
    }

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
                    if let Some(instance) = self.selected_instance_mut() {
                        instance.running = true;
                        self.status = format!("Launched {}", instance.name);
                    }
                    ui.close();
                }
                if ui.button("Kill").clicked() {
                    if let Some(instance) = self.selected_instance_mut() {
                        instance.running = false;
                        self.status = format!("Stopped {}", instance.name);
                    }
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
                    let name = self.accounts[idx].name.clone();
                    if ui.selectable_label(active, name).clicked() {
                        self.set_active_account(idx);
                    }
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

    fn draw_settings_tab(&mut self, ui: &mut egui::Ui) {
        let Some(instance) = self.selected_instance() else {
            ui.label("No instance selected");
            return;
        };

        ui.label("Instance Settings (migration placeholder)");
        ui.separator();
        ui.label(format!("Name: {}", instance.name));
        ui.label(format!("Path: {}", instance.path));
        ui.label("These controls will replace Qt instance settings pages one by one.");
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
    }
}

impl App for PrismarineApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
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
                    && let Some(instance) = self.selected_instance_mut()
                {
                    instance.running = true;
                    self.status = format!("Launched {}", instance.name);
                }
                if ui
                    .add_enabled(has_selected, egui::Button::new("Kill"))
                    .clicked()
                    && let Some(instance) = self.selected_instance_mut()
                {
                    instance.running = false;
                    self.status = format!("Stopped {}", instance.name);
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
