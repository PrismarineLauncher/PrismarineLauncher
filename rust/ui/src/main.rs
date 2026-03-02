use anyhow::Result;
use eframe::{App, Frame, NativeOptions, egui};
use rust_core::{format_s3_time, parse_s3_time, scan_instances};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const STATE_FILE: &str = "rust/ui/.state.json";

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
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            data_dir: "instances".to_string(),
            selected: None,
            show_news: true,
            filter: String::new(),
        }
    }
}

struct PrismarineApp {
    instances: Vec<Instance>,
    accounts: Vec<Account>,
    selected: Option<usize>,
    show_news: bool,
    status: String,
    filter: String,
    data_dir: String,
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
            show_news: persisted.show_news,
            status: "Ready".to_string(),
            filter: persisted.filter,
            data_dir: persisted.data_dir,
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

    fn persist(&self) {
        let state = PersistedState {
            data_dir: self.data_dir.clone(),
            selected: self.selected,
            show_news: self.show_news,
            filter: self.filter.clone(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&state) {
            if let Some(parent) = Path::new(STATE_FILE).parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(STATE_FILE, text);
        }
    }

    fn reload_instances(&mut self) {
        let root = PathBuf::from(&self.data_dir);
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
                    if self.selected.is_none() || self.selected.unwrap_or(0) >= self.instances.len() {
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
    }

    fn top_menu(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Add Instance").clicked() {
                    self.instances.push(Instance {
                        name: format!("New Instance {}", self.instances.len() + 1),
                        version: "custom".to_string(),
                        running: false,
                        path: self.data_dir.clone(),
                    });
                    self.status = "Instance added".to_string();
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
                if ui.button("Copy Instance").clicked() {
                    if let Some(instance) = self.selected_instance().cloned() {
                        let mut new_instance = instance;
                        new_instance.name.push_str(" (Copy)");
                        new_instance.running = false;
                        self.instances.push(new_instance);
                        self.status = "Instance copied".to_string();
                    }
                    ui.close();
                }
                if ui.button("Delete Instance").clicked() {
                    if let Some(index) = self.selected.take()
                        && index < self.instances.len()
                    {
                        let deleted = self.instances.remove(index);
                        self.selected = if self.instances.is_empty() { None } else { Some(0) };
                        self.status = format!("Deleted {}", deleted.name);
                    }
                    ui.close();
                }
            });

            ui.menu_button("View", |ui| {
                if ui.checkbox(&mut self.show_news, "Show News Bar").clicked() {
                    self.status = "Updated view settings".to_string();
                }
            });

            ui.menu_button("Folders", |ui| {
                ui.label("Data dir:");
                if ui.text_edit_singleline(&mut self.data_dir).changed() {
                    self.status = "Data directory changed".to_string();
                }
                if ui.button("Reload Instances").clicked() {
                    self.reload_instances();
                    ui.close();
                }
            });

            ui.menu_button("Accounts", |ui| {
                for acc in &self.accounts {
                    if acc.active {
                        ui.strong(&acc.name);
                    } else {
                        ui.label(&acc.name);
                    }
                }
            });

            ui.menu_button("Help", |ui| {
                ui.label("PrismarineLauncher Rust UI migration build");
                ui.label("Goal: parity with existing Qt interface and behavior");
            });
        });
    }
}

impl App for PrismarineApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            self.top_menu(ui);
        });

        egui::TopBottomPanel::top("main_toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("Add Instance").clicked() {
                    self.instances.push(Instance {
                        name: format!("New Instance {}", self.instances.len() + 1),
                        version: "custom".to_string(),
                        running: false,
                        path: self.data_dir.clone(),
                    });
                    self.status = "Instance added".to_string();
                }
                if ui.button("Folders").clicked() {
                    self.reload_instances();
                }
                if ui.button("Settings").clicked() {
                    self.status = "Open settings action".to_string();
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
                if ui.add_enabled(has_selected, egui::Button::new("Launch")).clicked()
                    && let Some(instance) = self.selected_instance_mut()
                {
                    instance.running = true;
                    self.status = format!("Launched {}", instance.name);
                }
                if ui.add_enabled(has_selected, egui::Button::new("Kill")).clicked()
                    && let Some(instance) = self.selected_instance_mut()
                {
                    instance.running = false;
                    self.status = format!("Stopped {}", instance.name);
                }
                ui.separator();
                if ui.add_enabled(has_selected, egui::Button::new("Edit")).clicked() {
                    self.status = "Edit instance".to_string();
                }
                if ui.add_enabled(has_selected, egui::Button::new("Copy")).clicked()
                    && let Some(instance) = self.selected_instance().cloned()
                {
                    let mut new_instance = instance;
                    new_instance.name.push_str(" (Copy)");
                    new_instance.running = false;
                    self.instances.push(new_instance);
                    self.status = "Instance copied".to_string();
                }
                if ui.add_enabled(has_selected, egui::Button::new("Delete")).clicked()
                    && let Some(index) = self.selected.take()
                    && index < self.instances.len()
                {
                    let deleted = self.instances.remove(index);
                    self.selected = if self.instances.is_empty() { None } else { Some(0) };
                    self.status = format!("Deleted {}", deleted.name);
                }
                if ui.button("Reload").clicked() {
                    self.reload_instances();
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
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
                    let status = if instance.running { "running" } else { "stopped" };
                    let line = format!("{} [{} | {}]", instance.name, instance.version, status);
                    if ui.selectable_label(selected, line).clicked() {
                        self.selected = Some(idx);
                    }
                }
            });

            ui.separator();
            if let Some(instance) = self.selected_instance() {
                ui.label(format!("Selected: {}", instance.name));
                ui.monospace(format!("Path: {}", instance.path));
            }

            if let Some((ms, offset)) = parse_s3_time("2016-02-29T13:49:54+01:00")
                && let Some(serialized) = format_s3_time(ms, offset)
            {
                ui.label(format!("Core timestamp parity: {}", serialized));
            }
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Status:");
                ui.monospace(&self.status);
            });
        });

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
