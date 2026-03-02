use anyhow::Result;
use eframe::{App, Frame, NativeOptions, egui};
use rust_core::{
    CurseForgeProjectDetails, CurseForgeSearchHit, LaunchProfile, LicensedMicrosoftAccount,
    MicrosoftDeviceCode, ModrinthProjectDetails, ModrinthSearchHit, build_java_command,
    complete_microsoft_device_login, copy_instance, create_instance,
    curseforge_get_project_details, curseforge_resolve_primary_file,
    curseforge_search_projects_paged, default_launch_profile, delete_instance,
    download_file_to_path, download_file_to_path_with_progress, format_s3_time, list_logs,
    list_mod_files, load_launch_profile, load_prism_instance_config, modrinth_get_project_details,
    modrinth_resolve_primary_file, modrinth_search_projects_by_type_paged, parse_s3_time,
    read_log_preview, rename_instance, save_launch_profile, scan_instances,
    start_microsoft_device_code, sync_modrinth_managed_mods, validate_minecraft_account,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum CenterTab {
    Overview,
    Mods,
    Logs,
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum DownloadProvider {
    Modrinth,
    CurseForge,
}

impl Default for DownloadProvider {
    fn default() -> Self {
        Self::Modrinth
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum DownloadContentType {
    Mods,
    ResourcePacks,
}

impl DownloadContentType {
    fn modrinth_project_type(&self) -> &'static str {
        match self {
            Self::Mods => "mod",
            Self::ResourcePacks => "resourcepack",
        }
    }

    fn curseforge_class(&self) -> &'static str {
        match self {
            Self::Mods => "mc-mods",
            Self::ResourcePacks => "texture-packs",
        }
    }

    fn curseforge_class_id(&self) -> i32 {
        match self {
            Self::Mods => 6,
            Self::ResourcePacks => 12,
        }
    }
}

impl Default for DownloadContentType {
    fn default() -> Self {
        Self::Mods
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CreateLoader {
    Fabric,
    Forge,
    Quilt,
    NeoForge,
}

impl CreateLoader {
    fn label(&self) -> &'static str {
        match self {
            Self::Fabric => "Fabric",
            Self::Forge => "Forge",
            Self::Quilt => "Quilt",
            Self::NeoForge => "Neo-Forge",
        }
    }

    fn mmc_uid(&self) -> &'static str {
        match self {
            Self::Fabric => "net.fabricmc.fabric-loader",
            Self::Forge => "net.minecraftforge",
            Self::Quilt => "org.quiltmc.quilt-loader",
            Self::NeoForge => "net.neoforged",
        }
    }

    fn cfg_value(&self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Forge => "forge",
            Self::Quilt => "quilt",
            Self::NeoForge => "neoforge",
        }
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ModListEntry {
    name: String,
    file_path: String,
    enabled: bool,
    display_name: String,
    version: String,
    updated_at: String,
    provider: String,
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

const MSA_CLIENT_ID: &str = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb";
const FLAME_API_KEY: &str = "$2a$10$wuAJuNZuted3NORVmpgUC.m8sI.pv1tOPKZyBgLFGjxFp/br0lZCC";

enum DeviceLoginEvent {
    Success(LicensedMicrosoftAccount),
    Error(String),
}

#[derive(Clone, Debug)]
enum DownloadSearchEvent {
    Modrinth {
        request_id: u64,
        append: bool,
        hits: Vec<ModrinthSearchHit>,
    },
    CurseForge {
        request_id: u64,
        append: bool,
        hits: Vec<CurseForgeSearchHit>,
    },
    Error {
        request_id: u64,
        message: String,
    },
}

#[derive(Clone, Debug)]
enum DownloadDetailsEvent {
    Ready {
        request_id: u64,
        cache_key: String,
        details: DownloadDetails,
    },
    Error {
        request_id: u64,
        message: String,
    },
}

#[derive(Clone, Debug, Default)]
struct DownloadDetails {
    title: String,
    markdown: String,
    icon_url: Option<String>,
}

#[derive(Clone, Debug)]
enum DownloadTaskKind {
    Modrinth {
        project_id: String,
        game_version: String,
        loader: String,
    },
    CurseForge {
        mod_id: i64,
        game_version: String,
    },
    DirectUrl {
        url: String,
        file_name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DownloadJobState {
    Queued,
    Resolving,
    Downloading,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
struct DownloadJob {
    id: u64,
    title: String,
    content_type: DownloadContentType,
    instance_path: PathBuf,
    kind: DownloadTaskKind,
    state: DownloadJobState,
    status: String,
    downloaded: u64,
    total: Option<u64>,
    progress: f32,
}

#[derive(Clone, Debug)]
enum DownloadQueueEvent {
    Resolving {
        job_id: u64,
        message: String,
    },
    Progress {
        job_id: u64,
        downloaded: u64,
        total: Option<u64>,
    },
    Finished {
        job_id: u64,
        target_path: PathBuf,
    },
    Error {
        job_id: u64,
        message: String,
    },
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
    create_game_version: String,
    create_loader: Option<CreateLoader>,
    show_rename_dialog: bool,
    rename_name: String,
    show_copy_dialog: bool,
    copy_name: String,
    show_delete_dialog: bool,
    mods_cache: Vec<ModListEntry>,
    logs_cache: Vec<(String, String)>,
    selected_log: Option<usize>,
    log_preview: String,
    launch_profile: LaunchProfile,
    processes: HashMap<String, Child>,
    show_add_account_dialog: bool,
    new_account_name: String,
    new_account_token: String,
    device_login_info: Option<MicrosoftDeviceCode>,
    device_login_receiver: Option<Receiver<DeviceLoginEvent>>,
    device_login_status: String,
    device_login_qr_payload: String,
    show_download_panel: bool,
    download_provider: DownloadProvider,
    download_content_type: DownloadContentType,
    modrinth_query: String,
    modrinth_loader: String,
    modrinth_game_version: String,
    modrinth_hits: Vec<ModrinthSearchHit>,
    selected_modrinth_hit: Option<usize>,
    curseforge_hits: Vec<CurseForgeSearchHit>,
    selected_curseforge_hit: Option<usize>,
    download_details: DownloadDetails,
    markdown_cache_modrinth: egui_commonmark::CommonMarkCache,
    markdown_cache_curseforge: egui_commonmark::CommonMarkCache,
    curseforge_query: String,
    curseforge_download_url: String,
    curseforge_filename: String,
    global_settings: GlobalLaunchSettings,
    settings_subtab: SettingsSubTab,
    icon_cache: HashMap<String, egui::TextureHandle>,
    post_exit_commands: HashMap<String, String>,
    modrinth_project_brief_cache: HashMap<String, (String, Option<String>)>,
    download_search_receiver: Option<Receiver<DownloadSearchEvent>>,
    download_search_request_id: u64,
    download_search_loading: bool,
    download_search_next_offset: usize,
    download_search_has_more: bool,
    download_search_debounce_deadline: Option<Instant>,
    download_search_last_input: String,
    download_details_receiver: Option<Receiver<DownloadDetailsEvent>>,
    download_details_request_id: u64,
    download_details_cache: HashMap<String, (DownloadDetails, Instant)>,
    download_jobs: Vec<DownloadJob>,
    next_download_job_id: u64,
    download_queue_tx: Sender<DownloadQueueEvent>,
    download_queue_rx: Receiver<DownloadQueueEvent>,
    max_parallel_downloads: usize,
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
        let (download_queue_tx, download_queue_rx) = mpsc::channel::<DownloadQueueEvent>();
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
            create_game_version: String::new(),
            create_loader: None,
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
            device_login_info: None,
            device_login_receiver: None,
            device_login_status: String::new(),
            device_login_qr_payload: String::new(),
            show_download_panel: false,
            download_provider: DownloadProvider::Modrinth,
            download_content_type: DownloadContentType::Mods,
            modrinth_query: String::new(),
            modrinth_loader: String::new(),
            modrinth_game_version: String::new(),
            modrinth_hits: Vec::new(),
            selected_modrinth_hit: None,
            curseforge_hits: Vec::new(),
            selected_curseforge_hit: None,
            download_details: DownloadDetails::default(),
            markdown_cache_modrinth: egui_commonmark::CommonMarkCache::default(),
            markdown_cache_curseforge: egui_commonmark::CommonMarkCache::default(),
            curseforge_query: String::new(),
            curseforge_download_url: String::new(),
            curseforge_filename: String::new(),
            global_settings: load_global_settings(),
            settings_subtab: SettingsSubTab::General,
            icon_cache: HashMap::new(),
            post_exit_commands: HashMap::new(),
            modrinth_project_brief_cache: HashMap::new(),
            download_search_receiver: None,
            download_search_request_id: 0,
            download_search_loading: false,
            download_search_next_offset: 0,
            download_search_has_more: true,
            download_search_debounce_deadline: None,
            download_search_last_input: String::new(),
            download_details_receiver: None,
            download_details_request_id: 0,
            download_details_cache: HashMap::new(),
            download_jobs: Vec::new(),
            next_download_job_id: 1,
            download_queue_tx,
            download_queue_rx,
            max_parallel_downloads: 2,
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

    fn apply_licensed_account(&mut self, account: LicensedMicrosoftAccount) {
        self.accounts.push(Account {
            name: account.username.clone(),
            active: false,
            account_type: AccountType::Licensed,
            access_token: Some(account.access_token),
            licensed: account.has_minecraft_license,
        });
        self.status = if account.has_minecraft_license {
            format!("Licensed account added: {}", account.username)
        } else {
            format!(
                "Account added but no Minecraft entitlement: {}",
                account.username
            )
        };
        save_accounts(&self.accounts);
        self.show_add_account_dialog = false;
        self.device_login_info = None;
        self.device_login_receiver = None;
        self.device_login_status.clear();
        self.device_login_qr_payload.clear();
    }

    fn do_start_device_code_login(&mut self) {
        match start_microsoft_device_code(MSA_CLIENT_ID) {
            Ok(device) => {
                let open_url = device
                    .verification_uri_complete
                    .clone()
                    .unwrap_or_else(|| device.verification_uri.clone());
                let _ = Command::new("sh")
                    .arg("-lc")
                    .arg(format!("xdg-open {}", shell_escape(&open_url)))
                    .status();

                let (tx, rx) = mpsc::channel::<DeviceLoginEvent>();
                let device_copy = device.clone();
                std::thread::spawn(move || {
                    let event = match complete_microsoft_device_login(MSA_CLIENT_ID, &device_copy) {
                        Ok(result) => DeviceLoginEvent::Success(result),
                        Err(err) => DeviceLoginEvent::Error(err),
                    };
                    let _ = tx.send(event);
                });

                self.device_login_qr_payload = open_url;
                self.device_login_status =
                    "Ожидание подтверждения в браузере/Microsoft...".to_string();
                self.device_login_info = Some(device);
                self.device_login_receiver = Some(rx);
                self.status = "Microsoft login started".to_string();
            }
            Err(err) => {
                self.status = format!("Failed to start Microsoft device login: {err}");
            }
        }
    }

    fn poll_device_login_events(&mut self) {
        let event = self
            .device_login_receiver
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        let Some(event) = event else {
            return;
        };
        match event {
            DeviceLoginEvent::Success(account) => {
                self.apply_licensed_account(account);
            }
            DeviceLoginEvent::Error(err) => {
                self.device_login_receiver = None;
                self.device_login_info = None;
                self.device_login_status = format!("Ошибка входа: {err}");
                self.status = format!("Microsoft login failed: {err}");
            }
        }
    }

    fn queue_download_search(&mut self) {
        self.download_search_debounce_deadline = Some(Instant::now() + Duration::from_millis(280));
    }

    fn start_download_search(&mut self, append: bool) {
        if self.download_search_loading {
            return;
        }

        let query = match self.download_provider {
            DownloadProvider::Modrinth => self.modrinth_query.trim().to_string(),
            DownloadProvider::CurseForge => self.curseforge_query.trim().to_string(),
        };
        if !append {
            self.download_search_last_input = query.clone();
        }
        if query.is_empty() {
            self.modrinth_hits.clear();
            self.curseforge_hits.clear();
            self.selected_modrinth_hit = None;
            self.selected_curseforge_hit = None;
            self.download_details = DownloadDetails::default();
            self.download_search_next_offset = 0;
            self.download_search_has_more = false;
            return;
        }

        self.download_search_request_id = self.download_search_request_id.wrapping_add(1);
        let request_id = self.download_search_request_id;
        let offset = if append {
            self.download_search_next_offset
        } else {
            0
        };
        if !append {
            self.download_search_next_offset = 0;
            self.download_search_has_more = true;
        }

        let provider = self.download_provider.clone();
        let content_type = self.download_content_type.clone();
        let game_version = self.modrinth_game_version.clone();
        let loader = self.modrinth_loader.clone();
        let (tx, rx) = mpsc::channel::<DownloadSearchEvent>();
        self.download_search_receiver = Some(rx);
        self.download_search_loading = true;

        std::thread::spawn(move || {
            let page_size = 25usize;
            let event = match provider {
                DownloadProvider::Modrinth => match modrinth_search_projects_by_type_paged(
                    &query,
                    page_size,
                    content_type.modrinth_project_type(),
                    offset,
                ) {
                    Ok(hits) => DownloadSearchEvent::Modrinth {
                        request_id,
                        append,
                        hits,
                    },
                    Err(message) => DownloadSearchEvent::Error {
                        request_id,
                        message: format!("Modrinth search failed: {message}"),
                    },
                },
                DownloadProvider::CurseForge => match curseforge_search_projects_paged(
                    FLAME_API_KEY,
                    &query,
                    &game_version,
                    content_type.curseforge_class_id(),
                    page_size,
                    offset,
                ) {
                    Ok(hits) => DownloadSearchEvent::CurseForge {
                        request_id,
                        append,
                        hits,
                    },
                    Err(message) => DownloadSearchEvent::Error {
                        request_id,
                        message: format!("CurseForge search failed: {message}"),
                    },
                },
            };
            let _ = tx.send(event);
            let _ = loader;
        });
    }

    fn open_curseforge_search(&mut self) {
        let mut url = format!(
            "https://www.curseforge.com/minecraft/search?class={}&search=",
            self.download_content_type.curseforge_class()
        );
        url.push_str(&percent_encode_query(self.curseforge_query.trim()));
        if !self.modrinth_game_version.trim().is_empty() {
            url.push_str("&gameVersion=");
            url.push_str(&percent_encode_query(self.modrinth_game_version.trim()));
        }
        let result = Command::new("sh")
            .arg("-lc")
            .arg(format!("xdg-open {}", shell_escape(&url)))
            .status();
        match result {
            Ok(_) => {
                self.status = "Opened CurseForge search in browser".to_string();
            }
            Err(err) => {
                self.status = format!("Failed to open browser: {err}");
            }
        }
    }

    fn poll_download_search_events(&mut self) {
        let event = self
            .download_search_receiver
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        let Some(event) = event else {
            return;
        };
        self.download_search_loading = false;
        match event {
            DownloadSearchEvent::Modrinth {
                request_id,
                append,
                hits,
            } => {
                if request_id != self.download_search_request_id {
                    return;
                }
                if !append {
                    self.modrinth_hits.clear();
                    self.selected_modrinth_hit = None;
                    self.curseforge_hits.clear();
                    self.selected_curseforge_hit = None;
                }
                let count = hits.len();
                self.modrinth_hits.extend(hits);
                self.download_search_next_offset = self.modrinth_hits.len();
                self.download_search_has_more = count >= 25;
                if self.selected_modrinth_hit.is_none() && !self.modrinth_hits.is_empty() {
                    self.selected_modrinth_hit = Some(0);
                    self.request_selected_download_details();
                }
                self.status = format!("Modrinth results: {}", self.modrinth_hits.len());
            }
            DownloadSearchEvent::CurseForge {
                request_id,
                append,
                hits,
            } => {
                if request_id != self.download_search_request_id {
                    return;
                }
                if !append {
                    self.curseforge_hits.clear();
                    self.selected_curseforge_hit = None;
                    self.modrinth_hits.clear();
                    self.selected_modrinth_hit = None;
                }
                let count = hits.len();
                self.curseforge_hits.extend(hits);
                self.download_search_next_offset = self.curseforge_hits.len();
                self.download_search_has_more = count >= 25;
                if self.selected_curseforge_hit.is_none() && !self.curseforge_hits.is_empty() {
                    self.selected_curseforge_hit = Some(0);
                    self.request_selected_download_details();
                }
                self.status = format!("CurseForge results: {}", self.curseforge_hits.len());
            }
            DownloadSearchEvent::Error {
                request_id,
                message,
            } => {
                if request_id == self.download_search_request_id {
                    self.status = message;
                }
            }
        }
    }

    fn details_markdown_from_modrinth(
        hit: &ModrinthSearchHit,
        details: &ModrinthProjectDetails,
    ) -> String {
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", details.title));
        if !hit.author.trim().is_empty() {
            out.push_str(&format!("by **{}**\n\n", hit.author));
        }
        if !details.summary.trim().is_empty() {
            out.push_str(&details.summary);
            out.push_str("\n\n");
        }
        if !details.donate_links.is_empty() {
            out.push_str("## Donate\n\n");
            for (platform, url) in &details.donate_links {
                out.push_str(&format!("- [{}]({})\n", platform, url));
            }
            out.push('\n');
        }
        let mut any_link = false;
        if !details.issues_url.trim().is_empty()
            || !details.source_url.trim().is_empty()
            || !details.wiki_url.trim().is_empty()
            || !details.discord_url.trim().is_empty()
            || !details.website_url.trim().is_empty()
        {
            out.push_str("## External Links\n\n");
            any_link = true;
        }
        if !details.website_url.trim().is_empty() {
            out.push_str(&format!("- [Website]({})\n", details.website_url));
        }
        if !details.issues_url.trim().is_empty() {
            out.push_str(&format!("- [Issues]({})\n", details.issues_url));
        }
        if !details.source_url.trim().is_empty() {
            out.push_str(&format!("- [Source]({})\n", details.source_url));
        }
        if !details.wiki_url.trim().is_empty() {
            out.push_str(&format!("- [Wiki]({})\n", details.wiki_url));
        }
        if !details.discord_url.trim().is_empty() {
            out.push_str(&format!("- [Discord]({})\n", details.discord_url));
        }
        if any_link {
            out.push('\n');
        }
        if !details.body_markdown.trim().is_empty() {
            out.push_str("---\n\n");
            out.push_str(&details.body_markdown);
        }
        out
    }

    fn details_markdown_from_curseforge(
        hit: &CurseForgeSearchHit,
        details: &CurseForgeProjectDetails,
    ) -> String {
        let mut out = String::new();
        out.push_str(&format!("# {}\n\n", details.title));
        if !hit.author.trim().is_empty() {
            out.push_str(&format!("by **{}**\n\n", hit.author));
        }
        if !details.summary.trim().is_empty() {
            out.push_str(&details.summary);
            out.push_str("\n\n");
        }
        out.push_str("## External Links\n\n");
        if !details.website_url.trim().is_empty() {
            out.push_str(&format!("- [Website]({})\n", details.website_url));
        }
        if !details.issues_url.trim().is_empty() {
            out.push_str(&format!("- [Issues]({})\n", details.issues_url));
        }
        if !details.source_url.trim().is_empty() {
            out.push_str(&format!("- [Source]({})\n", details.source_url));
        }
        if !details.wiki_url.trim().is_empty() {
            out.push_str(&format!("- [Wiki]({})\n", details.wiki_url));
        }
        out
    }

    fn request_selected_download_details(&mut self) {
        self.download_details = DownloadDetails::default();
        let (cache_key, request_id) = {
            self.download_details_request_id = self.download_details_request_id.wrapping_add(1);
            let request_id = self.download_details_request_id;
            let cache_key = match self.download_provider {
                DownloadProvider::Modrinth => {
                    let Some(i) = self.selected_modrinth_hit else {
                        return;
                    };
                    let Some(hit) = self.modrinth_hits.get(i) else {
                        return;
                    };
                    format!("modrinth:{}", hit.project_id)
                }
                DownloadProvider::CurseForge => {
                    let Some(i) = self.selected_curseforge_hit else {
                        return;
                    };
                    let Some(hit) = self.curseforge_hits.get(i) else {
                        return;
                    };
                    format!("curseforge:{}", hit.mod_id)
                }
            };
            (cache_key, request_id)
        };

        if let Some((cached, ts)) = self.download_details_cache.get(&cache_key)
            && ts.elapsed() <= Duration::from_secs(300)
        {
            self.download_details = cached.clone();
            return;
        }

        let provider = self.download_provider.clone();
        let modrinth_hit = self
            .selected_modrinth_hit
            .and_then(|i| self.modrinth_hits.get(i).cloned());
        let curseforge_hit = self
            .selected_curseforge_hit
            .and_then(|i| self.curseforge_hits.get(i).cloned());

        let (tx, rx) = mpsc::channel::<DownloadDetailsEvent>();
        self.download_details_receiver = Some(rx);
        std::thread::spawn(move || {
            let event = match provider {
                DownloadProvider::Modrinth => {
                    let Some(hit) = modrinth_hit else {
                        return;
                    };
                    match modrinth_get_project_details(&hit.project_id) {
                        Ok(details) => {
                            let d = DownloadDetails {
                                title: details.title.clone(),
                                icon_url: details.icon_url.clone().or(hit.icon_url.clone()),
                                markdown: PrismarineApp::details_markdown_from_modrinth(
                                    &hit, &details,
                                ),
                            };
                            DownloadDetailsEvent::Ready {
                                request_id,
                                cache_key,
                                details: d,
                            }
                        }
                        Err(err) => DownloadDetailsEvent::Error {
                            request_id,
                            message: format!("Details request failed: {err}"),
                        },
                    }
                }
                DownloadProvider::CurseForge => {
                    let Some(hit) = curseforge_hit else {
                        return;
                    };
                    match curseforge_get_project_details(FLAME_API_KEY, hit.mod_id) {
                        Ok(details) => {
                            let d = DownloadDetails {
                                title: details.title.clone(),
                                icon_url: details.icon_url.clone().or(hit.icon_url.clone()),
                                markdown: PrismarineApp::details_markdown_from_curseforge(
                                    &hit, &details,
                                ),
                            };
                            DownloadDetailsEvent::Ready {
                                request_id,
                                cache_key,
                                details: d,
                            }
                        }
                        Err(err) => DownloadDetailsEvent::Error {
                            request_id,
                            message: format!("Details request failed: {err}"),
                        },
                    }
                }
            };
            let _ = tx.send(event);
        });
    }

    fn poll_download_details_events(&mut self) {
        let event = self
            .download_details_receiver
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        let Some(event) = event else {
            return;
        };
        match event {
            DownloadDetailsEvent::Ready {
                request_id,
                cache_key,
                details,
            } => {
                if request_id != self.download_details_request_id {
                    return;
                }
                self.download_details = details.clone();
                self.download_details_cache
                    .insert(cache_key, (details, Instant::now()));
            }
            DownloadDetailsEvent::Error {
                request_id,
                message,
            } => {
                if request_id == self.download_details_request_id {
                    self.status = message;
                }
            }
        }
        self.download_details_cache
            .retain(|_, (_, ts)| ts.elapsed() <= Duration::from_secs(300));
    }

    fn queue_download_job(
        &mut self,
        title: String,
        content_type: DownloadContentType,
        instance_path: PathBuf,
        kind: DownloadTaskKind,
    ) {
        let job = DownloadJob {
            id: self.next_download_job_id,
            title: title.clone(),
            content_type,
            instance_path,
            kind,
            state: DownloadJobState::Queued,
            status: "Queued".to_string(),
            downloaded: 0,
            total: None,
            progress: 0.0,
        };
        self.next_download_job_id = self.next_download_job_id.wrapping_add(1);
        self.download_jobs.push(job);
        self.status = format!("Queued download: {title}");
    }

    fn queue_modrinth_selected_download(&mut self) {
        let Some(hit_idx) = self.selected_modrinth_hit else {
            self.status = "No Modrinth project selected".to_string();
            return;
        };
        let Some(hit) = self.modrinth_hits.get(hit_idx).cloned() else {
            self.status = "Invalid Modrinth selection".to_string();
            return;
        };
        let Some(instance_path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        let kind = DownloadTaskKind::Modrinth {
            project_id: hit.project_id.clone(),
            game_version: self.modrinth_game_version.trim().to_string(),
            loader: match self.download_content_type {
                DownloadContentType::Mods => self.modrinth_loader.trim().to_string(),
                DownloadContentType::ResourcePacks => String::new(),
            },
        };
        self.queue_download_job(
            format!("{} ({})", hit.title, hit.author),
            self.download_content_type.clone(),
            instance_path,
            kind,
        );
    }

    fn queue_modrinth_download_by_index(&mut self, idx: usize) {
        self.selected_modrinth_hit = Some(idx);
        self.request_selected_download_details();
        self.queue_modrinth_selected_download();
    }

    fn queue_curseforge_selected_download(&mut self) {
        let Some(hit_idx) = self.selected_curseforge_hit else {
            self.status = "No CurseForge project selected".to_string();
            return;
        };
        let Some(hit) = self.curseforge_hits.get(hit_idx).cloned() else {
            self.status = "Invalid CurseForge selection".to_string();
            return;
        };
        let Some(instance_path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        let kind = DownloadTaskKind::CurseForge {
            mod_id: hit.mod_id,
            game_version: self.modrinth_game_version.trim().to_string(),
        };
        self.queue_download_job(
            format!("{} ({})", hit.title, hit.author),
            self.download_content_type.clone(),
            instance_path,
            kind,
        );
    }

    fn queue_curseforge_download_by_index(&mut self, idx: usize) {
        self.selected_curseforge_hit = Some(idx);
        self.request_selected_download_details();
        self.queue_curseforge_selected_download();
    }

    fn queue_direct_url_download(&mut self) {
        let url = self.curseforge_download_url.trim().to_string();
        if url.is_empty() {
            self.status = "CurseForge URL is empty".to_string();
            return;
        }
        let Some(instance_path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };

        let file_name = if self.curseforge_filename.trim().is_empty() {
            url.rsplit('/')
                .next()
                .map(|x| x.split('?').next().unwrap_or(x))
                .filter(|x| !x.trim().is_empty())
                .unwrap_or(match self.download_content_type {
                    DownloadContentType::Mods => "downloaded-mod.jar",
                    DownloadContentType::ResourcePacks => "downloaded-resourcepack.zip",
                })
                .to_string()
        } else {
            self.curseforge_filename.trim().to_string()
        };

        self.queue_download_job(
            format!("Direct URL: {file_name}"),
            self.download_content_type.clone(),
            instance_path,
            DownloadTaskKind::DirectUrl { url, file_name },
        );
    }

    fn spawn_download_job_worker(&self, job_id: u64, job: DownloadJob) {
        let tx = self.download_queue_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(DownloadQueueEvent::Resolving {
                job_id,
                message: "Resolving file...".to_string(),
            });
            let resolved = match job.kind {
                DownloadTaskKind::Modrinth {
                    project_id,
                    game_version,
                    loader,
                } => modrinth_resolve_primary_file(&project_id, &game_version, &loader),
                DownloadTaskKind::CurseForge {
                    mod_id,
                    game_version,
                } => curseforge_resolve_primary_file(FLAME_API_KEY, mod_id, &game_version),
                DownloadTaskKind::DirectUrl { url, file_name } => {
                    Ok(rust_core::ModrinthDownloadFile {
                        url,
                        filename: file_name,
                    })
                }
            };

            let file = match resolved {
                Ok(file) => file,
                Err(err) => {
                    let _ = tx.send(DownloadQueueEvent::Error {
                        job_id,
                        message: format!("Resolve failed: {err}"),
                    });
                    return;
                }
            };
            let target =
                preferred_download_dir(&job.instance_path, &job.content_type).join(&file.filename);
            let mut last_emit = Instant::now() - Duration::from_secs(1);
            let res =
                download_file_to_path_with_progress(&file.url, &target, |downloaded, total| {
                    if last_emit.elapsed() >= Duration::from_millis(80) || total == Some(downloaded)
                    {
                        let _ = tx.send(DownloadQueueEvent::Progress {
                            job_id,
                            downloaded,
                            total,
                        });
                        last_emit = Instant::now();
                    }
                });
            match res {
                Ok(_) => {
                    let _ = tx.send(DownloadQueueEvent::Finished {
                        job_id,
                        target_path: target,
                    });
                }
                Err(err) => {
                    let _ = tx.send(DownloadQueueEvent::Error {
                        job_id,
                        message: format!("Download failed: {err}"),
                    });
                }
            }
        });
    }

    fn process_download_queue(&mut self) {
        let active = self
            .download_jobs
            .iter()
            .filter(|j| {
                j.state == DownloadJobState::Resolving || j.state == DownloadJobState::Downloading
            })
            .count();
        let mut available_slots = self.max_parallel_downloads.saturating_sub(active);
        if available_slots == 0 {
            return;
        }

        let mut to_start = Vec::new();
        for (idx, job) in self.download_jobs.iter_mut().enumerate() {
            if available_slots == 0 {
                break;
            }
            if job.state == DownloadJobState::Queued {
                job.state = DownloadJobState::Resolving;
                job.status = "Waiting for file metadata...".to_string();
                to_start.push(idx);
                available_slots -= 1;
            }
        }
        for idx in to_start {
            let job = self.download_jobs[idx].clone();
            self.spawn_download_job_worker(job.id, job);
        }
    }

    fn poll_download_queue_events(&mut self) {
        let mut has_completed = false;
        loop {
            let Ok(event) = self.download_queue_rx.try_recv() else {
                break;
            };
            match event {
                DownloadQueueEvent::Resolving { job_id, message } => {
                    if let Some(job) = self.download_jobs.iter_mut().find(|j| j.id == job_id) {
                        job.state = DownloadJobState::Resolving;
                        job.status = message;
                    }
                }
                DownloadQueueEvent::Progress {
                    job_id,
                    downloaded,
                    total,
                } => {
                    if let Some(job) = self.download_jobs.iter_mut().find(|j| j.id == job_id) {
                        job.state = DownloadJobState::Downloading;
                        job.downloaded = downloaded;
                        job.total = total;
                        job.progress = total
                            .map(|t| {
                                if t == 0 {
                                    0.0
                                } else {
                                    (downloaded as f32 / t as f32).clamp(0.0, 1.0)
                                }
                            })
                            .unwrap_or(0.0);
                        job.status = match total {
                            Some(t) => {
                                format!(
                                    "Downloading... {} / {}",
                                    human_bytes(downloaded),
                                    human_bytes(t)
                                )
                            }
                            None => format!("Downloading... {}", human_bytes(downloaded)),
                        };
                    }
                }
                DownloadQueueEvent::Finished {
                    job_id,
                    target_path,
                } => {
                    if let Some(job) = self.download_jobs.iter_mut().find(|j| j.id == job_id) {
                        job.state = DownloadJobState::Done;
                        job.progress = 1.0;
                        job.status = format!("Done: {}", target_path.display());
                    }
                    has_completed = true;
                }
                DownloadQueueEvent::Error { job_id, message } => {
                    if let Some(job) = self.download_jobs.iter_mut().find(|j| j.id == job_id) {
                        job.state = DownloadJobState::Failed;
                        job.status = message.clone();
                    }
                    self.status = message;
                }
            }
        }
        if has_completed {
            self.refresh_selected_content();
        }
    }

    fn draw_download_queue(&mut self, ui: &mut egui::Ui) {
        if self.download_jobs.is_empty() {
            return;
        }
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label("Download Queue");
                let active = self
                    .download_jobs
                    .iter()
                    .filter(|j| {
                        j.state == DownloadJobState::Resolving
                            || j.state == DownloadJobState::Downloading
                    })
                    .count();
                ui.monospace(format!("active: {active}"));
                if ui.button("Clear Finished").clicked() {
                    self.download_jobs.retain(|j| {
                        j.state != DownloadJobState::Done && j.state != DownloadJobState::Failed
                    });
                }
            });
            let start = self.download_jobs.len().saturating_sub(6);
            for job in self.download_jobs.iter().skip(start) {
                ui.horizontal(|ui| {
                    ui.label(job.title.as_str());
                    ui.add_space(8.0);
                    ui.small(job.status.as_str());
                });
                if job.state == DownloadJobState::Downloading
                    || job.state == DownloadJobState::Resolving
                {
                    let bar = if job.total.is_some() {
                        job.progress
                    } else {
                        0.0
                    };
                    ui.add(
                        egui::ProgressBar::new(bar)
                            .show_percentage()
                            .desired_width(ui.available_width()),
                    );
                } else if job.state == DownloadJobState::Done {
                    ui.add(egui::ProgressBar::new(1.0).desired_width(ui.available_width()));
                }
            }
        });
        ui.add_space(6.0);
    }

    fn do_curseforge_download_url(&mut self) {
        self.queue_direct_url_download();
    }

    fn do_curseforge_download_selected(&mut self) {
        self.queue_curseforge_selected_download();
    }

    fn do_modrinth_download_selected(&mut self) {
        self.queue_modrinth_selected_download();
    }

    fn do_auto_update_mods(&mut self) {
        let Some(instance_path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        match sync_modrinth_managed_mods(&instance_path) {
            Ok(changed) => {
                if changed > 0 {
                    self.status = format!("Updated {changed} mods from managed Modrinth pack");
                } else {
                    self.status = "Managed Modrinth mods are up to date".to_string();
                }
                self.refresh_selected_content();
            }
            Err(err) => {
                self.status = format!("Mod auto-update failed: {err}");
            }
        }
    }

    fn do_add_jar_to_mods(&mut self) {
        let Some(instance_path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        let Some(file_path) = rfd::FileDialog::new()
            .add_filter("Java archives", &["jar"])
            .pick_file()
        else {
            return;
        };
        let file_name = match file_path.file_name().and_then(|x| x.to_str()) {
            Some(x) if !x.trim().is_empty() => x.to_string(),
            _ => {
                self.status = "Selected file name is invalid".to_string();
                return;
            }
        };
        let target_dir = preferred_mods_dir(&instance_path);
        let target_path = target_dir.join(&file_name);
        match fs::copy(&file_path, &target_path) {
            Ok(_) => {
                self.status = format!("Added jar: {} -> {}", file_name, target_path.display());
                self.refresh_selected_content();
            }
            Err(err) => {
                self.status = format!("Failed to add jar: {err}");
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

    fn detect_instance_version(&self, instance_path: &Path) -> String {
        let cfg = load_prism_instance_config(instance_path).unwrap_or_default();
        if let Some(version) = cfg.intended_version
            && !version.trim().is_empty()
        {
            return version;
        }

        let mmc_pack = instance_path.join("mmc-pack.json");
        if let Ok(text) = fs::read_to_string(mmc_pack)
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(components) = json.get("components").and_then(|v| v.as_array())
        {
            for component in components {
                let uid = component.get("uid").and_then(|v| v.as_str()).unwrap_or("");
                if uid == "net.minecraft"
                    && let Some(version) = component.get("version").and_then(|v| v.as_str())
                    && !version.trim().is_empty()
                {
                    return version.to_string();
                }
            }
        }

        if let Ok(profile) = load_launch_profile(instance_path)
            && let Some((_, version)) = profile
                .game_args
                .windows(2)
                .find(|pair| pair[0] == "--version")
                .map(|pair| (&pair[0], &pair[1]))
            && !version.trim().is_empty()
        {
            return version.to_string();
        }

        "unknown".to_string()
    }

    fn detect_instance_loader(&self, instance_path: &Path) -> String {
        let mmc_pack = instance_path.join("mmc-pack.json");
        if let Ok(text) = fs::read_to_string(mmc_pack)
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(components) = json.get("components").and_then(|v| v.as_array())
        {
            for component in components {
                let uid = component.get("uid").and_then(|v| v.as_str()).unwrap_or("");
                if uid == "net.fabricmc.fabric-loader" {
                    return "fabric".to_string();
                }
                if uid == "org.quiltmc.quilt-loader" {
                    return "quilt".to_string();
                }
                if uid == "net.minecraftforge" {
                    return "forge".to_string();
                }
                if uid.contains("neoforge") || uid == "net.neoforged" {
                    return "neoforge".to_string();
                }
            }
        }
        String::new()
    }

    fn resolve_instance_icon_path(&self, instance_path: &Path) -> Option<String> {
        let cfg = load_prism_instance_config(instance_path).unwrap_or_default();
        if let Some(icon_key) = cfg.icon_key {
            let trimmed = icon_key.trim();
            if !trimmed.is_empty() && trimmed != "default" {
                for ext in ["png", "jpg", "jpeg", "ico"] {
                    let file_name = format!("{trimmed}.{ext}");
                    let candidate = self.data_root.join("icons").join(&file_name);
                    if candidate.is_file() {
                        return Some(candidate.display().to_string());
                    }
                    let candidate_alt = self.instance_root().join("icons").join(&file_name);
                    if candidate_alt.is_file() {
                        return Some(candidate_alt.display().to_string());
                    }
                }
            }
        }

        for candidate in [
            instance_path.join("icon.png"),
            instance_path.join(".minecraft").join("icon.png"),
        ] {
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
        None
    }

    fn resolve_mod_icon_path(&self, instance_path: &Path, mod_file_name: &str) -> Option<String> {
        let mod_file = resolve_mod_file_path(instance_path, mod_file_name)?;
        if !mod_file.is_file() {
            return None;
        }

        let stem = mod_file.file_stem()?.to_str()?.to_string();
        let with_extension = mod_file
            .file_name()
            .and_then(|x| x.to_str())
            .map(|x| x.to_string())?;
        let parent = mod_file.parent()?;
        for candidate in [
            parent.join(format!("{stem}.png")),
            parent.join(format!("{stem}.jpg")),
            parent.join(format!("{stem}.jpeg")),
            parent.join(format!("{with_extension}.png")),
            parent.join(format!("{with_extension}.jpg")),
            parent.join(format!("{with_extension}.jpeg")),
        ] {
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
        None
    }

    fn read_modrinth_managed_metadata(
        &mut self,
        instance_path: &Path,
    ) -> HashMap<String, (String, Option<String>)> {
        let mut out = HashMap::new();
        let index_path = instance_path.join("mrpack").join("modrinth.index.json");
        let Ok(text) = fs::read_to_string(index_path) else {
            return out;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return out;
        };
        let Some(files) = json.get("files").and_then(|x| x.as_array()) else {
            return out;
        };
        for file in files {
            let Some(path) = file.get("path").and_then(|x| x.as_str()) else {
                continue;
            };
            if !path.starts_with("mods/") {
                continue;
            }
            let Some(file_name) = Path::new(path)
                .file_name()
                .and_then(|x| x.to_str())
                .map(|x| x.to_string())
            else {
                continue;
            };
            let project_id = file
                .get("downloads")
                .and_then(|x| x.as_array())
                .and_then(|arr| arr.first())
                .and_then(|x| x.as_str())
                .and_then(extract_modrinth_project_id_from_download_url)
                .unwrap_or_default();
            if project_id.is_empty() {
                continue;
            }
            if let Some(cached) = self.modrinth_project_brief_cache.get(&project_id) {
                out.insert(file_name, cached.clone());
            } else {
                out.insert(
                    file_name,
                    (
                        prettify_mod_name(
                            Path::new(path)
                                .file_name()
                                .and_then(|x| x.to_str())
                                .unwrap_or_default(),
                        ),
                        None,
                    ),
                );
            }
        }
        out
    }

    fn do_toggle_mod_enabled(&mut self, file_path: &str, enable: bool) {
        let from = PathBuf::from(file_path);
        let Some(name) = from.file_name().and_then(|x| x.to_str()) else {
            self.status = "Invalid mod file name".to_string();
            return;
        };
        let to_name = if enable {
            name.strip_suffix(".disabled").unwrap_or(name).to_string()
        } else if name.ends_with(".disabled") {
            name.to_string()
        } else {
            format!("{name}.disabled")
        };
        if to_name == name {
            return;
        }
        let Some(parent) = from.parent() else {
            self.status = "Invalid mod file path".to_string();
            return;
        };
        let to = parent.join(to_name);
        match fs::rename(&from, &to) {
            Ok(_) => {
                self.status = format!("Updated mod state: {}", to.display());
                self.refresh_selected_content();
            }
            Err(err) => {
                self.status = format!("Failed to update mod state: {err}");
            }
        }
    }

    fn reload_instances(&mut self) {
        let root = self.instance_root();
        match scan_instances(&root) {
            Ok(items) => {
                self.instances = items
                    .into_iter()
                    .map(|item| {
                        let version = self.detect_instance_version(&item.path);
                        let icon_path = self.resolve_instance_icon_path(&item.path);
                        Instance {
                            name: item.name,
                            version,
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
            self.modrinth_loader.clear();
            self.modrinth_game_version.clear();
            return;
        };

        match list_mod_files(&path) {
            Ok(mods) => {
                let managed = self.read_modrinth_managed_metadata(&path);
                self.mods_cache = mods
                    .into_iter()
                    .map(|name| {
                        let file_path = resolve_mod_file_path(&path, &name)
                            .map(|x| x.display().to_string())
                            .unwrap_or_else(|| {
                                path.join("minecraft/mods")
                                    .join(&name)
                                    .display()
                                    .to_string()
                            });
                        let enabled = !name.ends_with(".disabled");
                        let clean_name =
                            name.strip_suffix(".disabled").unwrap_or(&name).to_string();
                        let (display_name, icon_path, provider) =
                            if let Some((title, icon)) = managed.get(&clean_name) {
                                (
                                    title.clone(),
                                    icon.clone()
                                        .or_else(|| self.resolve_mod_icon_path(&path, &name)),
                                    "Modrinth".to_string(),
                                )
                            } else {
                                (
                                    prettify_mod_name(&clean_name),
                                    self.resolve_mod_icon_path(&path, &name),
                                    "Local".to_string(),
                                )
                            };
                        let version = extract_version_from_mod_filename(&clean_name);
                        let updated_at = fs::metadata(&file_path)
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .map(format_system_time_ddmmyyyy)
                            .unwrap_or_default();
                        ModListEntry {
                            name: clean_name,
                            file_path,
                            enabled,
                            display_name,
                            version,
                            updated_at,
                            provider,
                            icon_path,
                        }
                    })
                    .collect();
                self.mods_cache.sort_by(|a, b| {
                    a.display_name
                        .to_lowercase()
                        .cmp(&b.display_name.to_lowercase())
                });
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

        let detected_version = self.detect_instance_version(&path);
        self.modrinth_game_version = if detected_version == "unknown" {
            String::new()
        } else {
            detected_version
        };
        self.modrinth_loader = self.detect_instance_loader(&path);
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
        let game_version = self.create_game_version.trim().to_string();
        let Some(loader) = self.create_loader.clone() else {
            self.status = "Choose loader: Fabric / Forge / Quilt / Neo-Forge".to_string();
            return;
        };
        if name.is_empty() {
            self.status = "Instance name must not be empty".to_string();
            return;
        }
        if game_version.is_empty() {
            self.status = "Game version is required".to_string();
            return;
        }
        match create_instance(&self.instance_root(), &name) {
            Ok(created) => {
                let cfg_text = format!(
                    "# PrismarineLauncher instance\nIntendedVersion={}\nManagedLoader={}\n",
                    game_version,
                    loader.cfg_value()
                );
                if let Err(err) = fs::write(created.path.join("instance.cfg"), cfg_text) {
                    self.status =
                        format!("Created instance {name}, but failed to write cfg: {err}");
                    return;
                }

                let mmc_pack = serde_json::json!({
                    "formatVersion": 1,
                    "components": [
                        { "uid": "net.minecraft", "version": game_version },
                        { "uid": loader.mmc_uid(), "version": "0.0.0" }
                    ]
                });
                if let Err(err) = fs::write(
                    created.path.join("mmc-pack.json"),
                    serde_json::to_string_pretty(&mmc_pack).unwrap_or_else(|_| "{}".to_string()),
                ) {
                    self.status = format!(
                        "Created instance {name}, but failed to write mmc-pack.json: {err}"
                    );
                    return;
                }

                let mut profile = default_launch_profile(&created.path);
                upsert_arg_pair(&mut profile.game_args, "--version", game_version.as_str());
                if let Err(err) = save_launch_profile(&created.path, &profile) {
                    self.status = format!(
                        "Created instance {name}, but failed to save launch profile: {err}"
                    );
                    return;
                }

                self.status = format!(
                    "Created instance {name} [{} | {}]",
                    game_version,
                    loader.label()
                );
                self.show_create_dialog = false;
                self.create_name.clear();
                self.create_game_version.clear();
                self.create_loader = None;
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
        match sync_modrinth_managed_mods(&instance_path) {
            Ok(changed) if changed > 0 => {
                self.status = format!("Updated {changed} mods before launch");
            }
            Ok(_) => {}
            Err(err) => {
                self.status = format!("Mod auto-update failed before launch: {err}");
            }
        }
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
        self.create_game_version.clear();
        self.create_loader = None;
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

    fn cache_remote_icon_path(&self, icon_url: &str) -> Option<String> {
        if !icon_url.starts_with("http://") && !icon_url.starts_with("https://") {
            return None;
        }
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        icon_url.hash(&mut h);
        let ext = icon_url
            .rsplit('.')
            .next()
            .map(|x| x.split('?').next().unwrap_or(x))
            .filter(|x| x.len() <= 8 && x.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or("img");
        let path = self.data_root.join("cache").join("ui_icons").join(format!(
            "{:016x}.{}",
            h.finish(),
            ext
        ));
        if path.is_file() {
            return Some(path.display().to_string());
        }
        if download_file_to_path(icon_url, &path).is_ok() {
            return Some(path.display().to_string());
        }
        None
    }

    fn ensure_icon_texture_from_source(
        &mut self,
        ctx: &egui::Context,
        icon_source: &str,
    ) -> Option<egui::TextureHandle> {
        if icon_source.starts_with("http://") || icon_source.starts_with("https://") {
            let local = self.cache_remote_icon_path(icon_source)?;
            return self.ensure_icon_texture(ctx, &local);
        }
        self.ensure_icon_texture(ctx, icon_source)
    }

    fn ensure_qr_texture(
        &mut self,
        ctx: &egui::Context,
        payload: &str,
    ) -> Option<egui::TextureHandle> {
        let key = format!("qr::{payload}");
        if let Some(tex) = self.icon_cache.get(&key) {
            return Some(tex.clone());
        }
        let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
        let qr = code
            .render::<image::Luma<u8>>()
            .min_dimensions(192, 192)
            .max_dimensions(192, 192)
            .build();
        let w = qr.width() as usize;
        let h = qr.height() as usize;
        if w == 0 || h == 0 {
            return None;
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for p in qr.into_raw() {
            rgba.push(p);
            rgba.push(p);
            rgba.push(p);
            rgba.push(255);
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        let tex = ctx.load_texture(key.clone(), image, egui::TextureOptions::NEAREST);
        self.icon_cache.insert(key, tex.clone());
        Some(tex)
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
        self.draw_download_queue(ui);
        ui.horizontal(|ui| {
            if ui.button("Add Jar").clicked() {
                self.do_add_jar_to_mods();
            }
            if ui.button("Auto Update Mods").clicked() {
                self.do_auto_update_mods();
            }
            if ui.button("Download Mods").clicked() {
                self.show_download_panel = !self.show_download_panel;
            }
            ui.label(format!("Total: {}", self.mods_cache.len()));
        });
        ui.separator();

        let mut pending_toggle: Option<(String, bool)> = None;
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace).max(18.0);
        if self.mods_cache.is_empty() {
            ui.label("No mods found");
        } else {
            ui.horizontal(|ui| {
                ui.label("Enable");
                ui.add_space(10.0);
                ui.label("Image");
                ui.add_space(16.0);
                ui.label("Name");
                ui.add_space(220.0);
                ui.label("Version");
                ui.add_space(50.0);
                ui.label("Updated");
                ui.add_space(30.0);
                ui.label("Provider");
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("mods_table_scroll")
                .max_height(300.0)
                .show_rows(
                    ui,
                    row_height + 6.0,
                    self.mods_cache.len(),
                    |ui, row_range| {
                        for idx in row_range {
                            let (
                                enabled_now,
                                file_path,
                                icon_path,
                                display_name,
                                version,
                                updated_at,
                                provider,
                            ) = {
                                let item = &self.mods_cache[idx];
                                (
                                    item.enabled,
                                    item.file_path.clone(),
                                    item.icon_path.clone(),
                                    item.display_name.clone(),
                                    item.version.clone(),
                                    item.updated_at.clone(),
                                    item.provider.clone(),
                                )
                            };
                            ui.horizontal(|ui| {
                                let mut enabled = enabled_now;
                                if ui.checkbox(&mut enabled, "").changed() {
                                    pending_toggle = Some((file_path.clone(), enabled));
                                }
                                if let Some(icon_path) = &icon_path {
                                    if let Some(tex) =
                                        self.ensure_icon_texture_from_source(ui.ctx(), icon_path)
                                    {
                                        ui.image((tex.id(), egui::vec2(row_height, row_height)));
                                    } else {
                                        ui.add_space(row_height);
                                    }
                                } else {
                                    ui.add_space(row_height);
                                }
                                ui.label(display_name.as_str());
                                ui.add_space(20.0);
                                ui.monospace(version.as_str());
                                ui.add_space(20.0);
                                ui.monospace(updated_at.as_str());
                                ui.add_space(20.0);
                                ui.label(provider.as_str());
                            });
                            ui.separator();
                        }
                    },
                );
        }
        if let Some((file_path, enabled)) = pending_toggle {
            self.do_toggle_mod_enabled(&file_path, enabled);
        }

        if !self.show_download_panel {
            return;
        }

        ui.separator();
        ui.group(|ui| {
            let mut provider_changed = false;
            let mut content_changed = false;
            ui.horizontal(|ui| {
                ui.heading("Скачивание");
                content_changed |= ui
                    .selectable_value(
                        &mut self.download_content_type,
                        DownloadContentType::Mods,
                        "Mods",
                    )
                    .changed();
                content_changed |= ui
                    .selectable_value(
                        &mut self.download_content_type,
                        DownloadContentType::ResourcePacks,
                        "Resource Packs",
                    )
                    .changed();
                provider_changed |= ui
                    .selectable_value(
                        &mut self.download_provider,
                        DownloadProvider::Modrinth,
                        "Modrinth",
                    )
                    .changed();
                provider_changed |= ui
                    .selectable_value(
                        &mut self.download_provider,
                        DownloadProvider::CurseForge,
                        "CurseForge",
                    )
                    .changed();
            });
            if provider_changed || content_changed {
                self.request_selected_download_details();
                self.start_download_search(false);
            }
            ui.label(format!(
                "Автоопределено: loader={} | version={}",
                if self.modrinth_loader.is_empty() {
                    "unknown"
                } else {
                    &self.modrinth_loader
                },
                if self.modrinth_game_version.is_empty() {
                    "unknown"
                } else {
                    &self.modrinth_game_version
                }
            ));
            ui.separator();

            match self.download_provider {
                DownloadProvider::Modrinth => {
                    ui.horizontal(|ui| {
                        ui.label("Query:");
                        let changed = ui.text_edit_singleline(&mut self.modrinth_query).changed();
                        if changed {
                            self.queue_download_search();
                        }
                        if ui.button("Search").clicked() {
                            self.start_download_search(false);
                        }
                        if ui.button("Download Selected").clicked() {
                            self.do_modrinth_download_selected();
                        }
                    });
                    ui.columns(2, |cols| {
                        let mut row_end = 0usize;
                        let mut pending_select = None;
                        let mut pending_queue = None;
                        cols[0].label("Результаты");
                        cols[0].separator();
                        egui::ScrollArea::vertical()
                            .id_salt("download_modrinth_results_scroll")
                            .max_height(220.0)
                            .show_rows(
                                &mut cols[0],
                                24.0,
                                self.modrinth_hits.len(),
                                |ui, row_range| {
                                    row_end = row_range.end;
                                    for idx in row_range {
                                        let (icon_url, label) = {
                                            let hit = &self.modrinth_hits[idx];
                                            let mut label = hit.title.clone();
                                            if !hit.author.trim().is_empty() {
                                                label.push_str(&format!(" ({})", hit.author));
                                            } else {
                                                label.push_str(&format!(" ({})", hit.slug));
                                            }
                                            (hit.icon_url.clone(), label)
                                        };
                                        let selected = self.selected_modrinth_hit == Some(idx);
                                        let row_h =
                                            ui.text_style_height(&egui::TextStyle::Body).max(22.0);
                                        ui.horizontal(|ui| {
                                            if let Some(icon) = &icon_url {
                                                if let Some(tex) = self
                                                    .ensure_icon_texture_from_source(ui.ctx(), icon)
                                                {
                                                    ui.image((tex.id(), egui::vec2(row_h, row_h)));
                                                } else {
                                                    ui.add_space(row_h);
                                                }
                                            } else {
                                                ui.add_space(row_h);
                                            }
                                            let response = ui.selectable_label(selected, label);
                                            if response.clicked() {
                                                pending_select = Some(idx);
                                            }
                                            if response.double_clicked() {
                                                pending_queue = Some(idx);
                                            }
                                        });
                                    }
                                },
                            );
                        if let Some(idx) = pending_select {
                            self.selected_modrinth_hit = Some(idx);
                            self.request_selected_download_details();
                        }
                        if let Some(idx) = pending_queue {
                            self.queue_modrinth_download_by_index(idx);
                        }
                        if row_end + 2 >= self.modrinth_hits.len()
                            && self.download_search_has_more
                            && !self.download_search_loading
                        {
                            self.start_download_search(true);
                        }
                        if self.download_search_loading {
                            cols[0].label("Loading...");
                        }

                        cols[1].label("Описание");
                        cols[1].separator();
                        if let Some(icon) = self.download_details.icon_url.clone() {
                            if let Some(tex) =
                                self.ensure_icon_texture_from_source(cols[1].ctx(), &icon)
                            {
                                cols[1].image((tex.id(), egui::vec2(56.0, 56.0)));
                            }
                        }
                        if !self.download_details.title.is_empty() {
                            cols[1].heading(&self.download_details.title);
                        }
                        if self.download_details.markdown.trim().is_empty() {
                            cols[1].label("Проект не выбран");
                        } else {
                            cols[1].push_id("modrinth_markdown_panel", |ui| {
                                egui_commonmark::CommonMarkViewer::new().show(
                                    ui,
                                    &mut self.markdown_cache_modrinth,
                                    &self.download_details.markdown,
                                );
                            });
                        }
                    });
                }
                DownloadProvider::CurseForge => {
                    ui.horizontal(|ui| {
                        ui.label("Query:");
                        let changed = ui
                            .text_edit_singleline(&mut self.curseforge_query)
                            .changed();
                        if changed {
                            self.queue_download_search();
                        }
                        if ui.button("Search").clicked() {
                            self.start_download_search(false);
                        }
                        if ui.button("Open Search").clicked() {
                            self.open_curseforge_search();
                        }
                        if ui.button("Download Selected").clicked() {
                            self.do_curseforge_download_selected();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Direct file URL:");
                        ui.text_edit_singleline(&mut self.curseforge_download_url);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Filename (optional):");
                        ui.text_edit_singleline(&mut self.curseforge_filename);
                        if ui.button("Download URL").clicked() {
                            self.do_curseforge_download_url();
                        }
                    });
                    ui.columns(2, |cols| {
                        let mut row_end = 0usize;
                        let mut pending_select = None;
                        let mut pending_queue = None;
                        cols[0].label("Результаты");
                        cols[0].separator();
                        egui::ScrollArea::vertical()
                            .id_salt("download_curseforge_results_scroll")
                            .max_height(220.0)
                            .show_rows(
                                &mut cols[0],
                                24.0,
                                self.curseforge_hits.len(),
                                |ui, row_range| {
                                    row_end = row_range.end;
                                    for idx in row_range {
                                        let (icon_url, label) = {
                                            let hit = &self.curseforge_hits[idx];
                                            let mut label = hit.title.clone();
                                            if !hit.author.trim().is_empty() {
                                                label.push_str(&format!(" ({})", hit.author));
                                            }
                                            (hit.icon_url.clone(), label)
                                        };
                                        let selected = self.selected_curseforge_hit == Some(idx);
                                        let row_h =
                                            ui.text_style_height(&egui::TextStyle::Body).max(22.0);
                                        ui.horizontal(|ui| {
                                            if let Some(icon) = &icon_url {
                                                if let Some(tex) = self
                                                    .ensure_icon_texture_from_source(ui.ctx(), icon)
                                                {
                                                    ui.image((tex.id(), egui::vec2(row_h, row_h)));
                                                } else {
                                                    ui.add_space(row_h);
                                                }
                                            } else {
                                                ui.add_space(row_h);
                                            }
                                            let response = ui.selectable_label(selected, label);
                                            if response.clicked() {
                                                pending_select = Some(idx);
                                            }
                                            if response.double_clicked() {
                                                pending_queue = Some(idx);
                                            }
                                        });
                                    }
                                },
                            );
                        if let Some(idx) = pending_select {
                            self.selected_curseforge_hit = Some(idx);
                            self.request_selected_download_details();
                        }
                        if let Some(idx) = pending_queue {
                            self.queue_curseforge_download_by_index(idx);
                        }
                        if row_end + 2 >= self.curseforge_hits.len()
                            && self.download_search_has_more
                            && !self.download_search_loading
                        {
                            self.start_download_search(true);
                        }
                        if self.download_search_loading {
                            cols[0].label("Loading...");
                        }
                        cols[1].label("Описание");
                        cols[1].separator();
                        if let Some(icon) = self.download_details.icon_url.clone() {
                            if let Some(tex) =
                                self.ensure_icon_texture_from_source(cols[1].ctx(), &icon)
                            {
                                cols[1].image((tex.id(), egui::vec2(56.0, 56.0)));
                            }
                        }
                        if !self.download_details.title.is_empty() {
                            cols[1].heading(&self.download_details.title);
                        }
                        if self.download_details.markdown.trim().is_empty() {
                            cols[1].label("Проект не выбран");
                        } else {
                            cols[1].push_id("curseforge_markdown_panel", |ui| {
                                egui_commonmark::CommonMarkViewer::new().show(
                                    ui,
                                    &mut self.markdown_cache_curseforge,
                                    &self.download_details.markdown,
                                );
                            });
                        }
                    });
                }
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
                    ui.label("Game version (required):");
                    ui.text_edit_singleline(&mut self.create_game_version);
                    ui.label("Loader (required):");
                    egui::ComboBox::from_id_salt("create_loader")
                        .selected_text(
                            self.create_loader
                                .as_ref()
                                .map(CreateLoader::label)
                                .unwrap_or("Select loader"),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.create_loader,
                                Some(CreateLoader::Fabric),
                                "Fabric",
                            );
                            ui.selectable_value(
                                &mut self.create_loader,
                                Some(CreateLoader::Forge),
                                "Forge",
                            );
                            ui.selectable_value(
                                &mut self.create_loader,
                                Some(CreateLoader::Quilt),
                                "Quilt",
                            );
                            ui.selectable_value(
                                &mut self.create_loader,
                                Some(CreateLoader::NeoForge),
                                "Neo-Forge",
                            );
                        });
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
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .default_width(540.0)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label("Microsoft login (recommended):");
                        ui.horizontal(|ui| {
                            let login_running = self.device_login_receiver.is_some();
                            if ui
                                .add_enabled(
                                    !login_running,
                                    egui::Button::new("Login via Browser + Code/QR"),
                                )
                                .clicked()
                            {
                                self.do_start_device_code_login();
                            }
                            if let Some(device) = &self.device_login_info {
                                let open_url = device
                                    .verification_uri_complete
                                    .clone()
                                    .unwrap_or_else(|| device.verification_uri.clone());
                                if ui.button("Open Microsoft Page").clicked() {
                                    let _ = Command::new("sh")
                                        .arg("-lc")
                                        .arg(format!("xdg-open {}", shell_escape(&open_url)))
                                        .status();
                                }
                            }
                        });
                        if let Some(device) = self.device_login_info.clone() {
                            ui.separator();
                            ui.label(format!("Code: {}", device.user_code));
                            ui.label(format!("URL: {}", device.verification_uri));
                            let qr_payload = self.device_login_qr_payload.clone();
                            if let Some(tex) = self.ensure_qr_texture(ui.ctx(), &qr_payload) {
                                ui.image((tex.id(), egui::vec2(192.0, 192.0)));
                            }
                            if !self.device_login_status.is_empty() {
                                ui.label(&self.device_login_status);
                            }
                        }
                        ui.separator();
                        ui.label("Manual token fallback:");
                    });

                    ui.label("Display name:");
                    ui.text_edit_singleline(&mut self.new_account_name);
                    ui.label("Access token (Minecraft Services):");
                    ui.add(egui::TextEdit::singleline(&mut self.new_account_token).password(true));
                    ui.vertical_centered(|ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Validate + Add").clicked() {
                                self.do_add_licensed_account();
                            }
                            if ui.button("Cancel").clicked() {
                                self.show_add_account_dialog = false;
                                self.device_login_receiver = None;
                                self.device_login_info = None;
                                self.device_login_status.clear();
                                self.device_login_qr_payload.clear();
                            }
                        });
                    });
                });
        }
    }
}

impl App for PrismarineApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.sync_process_states();
        self.poll_device_login_events();
        self.poll_download_search_events();
        self.poll_download_details_events();
        self.poll_download_queue_events();
        self.process_download_queue();

        if let Some(deadline) = self.download_search_debounce_deadline
            && Instant::now() >= deadline
        {
            self.download_search_debounce_deadline = None;
            let current_query = match self.download_provider {
                DownloadProvider::Modrinth => self.modrinth_query.trim().to_string(),
                DownloadProvider::CurseForge => self.curseforge_query.trim().to_string(),
            };
            if current_query != self.download_search_last_input {
                self.start_download_search(false);
            }
        }

        if self.download_search_loading
            || self.download_details_receiver.is_some()
            || self.download_search_debounce_deadline.is_some()
            || self.download_jobs.iter().any(|j| {
                j.state == DownloadJobState::Queued
                    || j.state == DownloadJobState::Resolving
                    || j.state == DownloadJobState::Downloading
            })
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

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

fn percent_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.bytes() {
        match ch {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(ch as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push_str(&format!("{ch:02X}"));
            }
        }
    }
    out
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn preferred_mods_dir(instance_path: &Path) -> PathBuf {
    let candidates = [
        instance_path.join("minecraft/mods"),
        instance_path.join(".minecraft/mods"),
        instance_path.join("mods"),
    ];
    for candidate in candidates {
        if candidate.is_dir() {
            return candidate;
        }
    }
    let fallback = instance_path.join("minecraft/mods");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn resolve_mod_file_path(instance_path: &Path, mod_file_name: &str) -> Option<PathBuf> {
    for candidate in [
        instance_path.join("minecraft/mods").join(mod_file_name),
        instance_path.join(".minecraft/mods").join(mod_file_name),
        instance_path.join("mods").join(mod_file_name),
    ] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn extract_modrinth_project_id_from_download_url(url: &str) -> Option<String> {
    let marker = "/data/";
    let idx = url.find(marker)?;
    let tail = &url[idx + marker.len()..];
    let id = tail.split('/').next()?.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

fn extract_version_from_mod_filename(file_name: &str) -> String {
    let stem = file_name
        .strip_suffix(".jar")
        .or_else(|| file_name.strip_suffix(".zip"))
        .unwrap_or(file_name);
    if let Some(idx) = stem.rfind('-') {
        let ver = stem[idx + 1..].trim();
        if !ver.is_empty() {
            return ver.to_string();
        }
    }
    String::new()
}

fn prettify_mod_name(file_name: &str) -> String {
    let stem = file_name
        .strip_suffix(".jar")
        .or_else(|| file_name.strip_suffix(".zip"))
        .unwrap_or(file_name);
    stem.split('-')
        .next()
        .unwrap_or(stem)
        .replace('_', " ")
        .trim()
        .to_string()
}

fn format_system_time_ddmmyyyy(time: std::time::SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = time.into();
    dt.format("%d.%m.%Y").to_string()
}

fn preferred_resourcepacks_dir(instance_path: &Path) -> PathBuf {
    let candidates = [
        instance_path.join("minecraft/resourcepacks"),
        instance_path.join(".minecraft/resourcepacks"),
        instance_path.join("resourcepacks"),
    ];
    for candidate in candidates {
        if candidate.is_dir() {
            return candidate;
        }
    }
    let fallback = instance_path.join("minecraft/resourcepacks");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn preferred_download_dir(instance_path: &Path, content_type: &DownloadContentType) -> PathBuf {
    match content_type {
        DownloadContentType::Mods => preferred_mods_dir(instance_path),
        DownloadContentType::ResourcePacks => preferred_resourcepacks_dir(instance_path),
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

fn upsert_arg_pair(args: &mut Vec<String>, key: &str, value: &str) {
    let mut i = 0usize;
    while i + 1 < args.len() {
        if args[i] == key {
            args[i + 1] = value.to_string();
            return;
        }
        i += 1;
    }
    args.push(key.to_string());
    args.push(value.to_string());
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
