use anyhow::Result;
use arboard::Clipboard;
use eframe::{App, Frame, NativeOptions, egui};
use rust_core::{
    CurseForgeProjectDetails, CurseForgeSearchHit, LaunchProfile, LicensedMicrosoftAccount,
    MicrosoftDeviceCode, ModrinthProjectDetails, ModrinthSearchHit, PrismInstanceConfig,
    RuntimeDownloadProgress, build_java_command, complete_microsoft_device_login, copy_instance,
    create_instance, curseforge_get_project_details, curseforge_resolve_primary_file,
    curseforge_search_projects_paged, default_launch_profile, delete_instance,
    download_file_to_path, download_file_to_path_with_progress,
    ensure_fabric_runtime_with_progress, ensure_minecraft_runtime_with_progress, format_s3_time,
    list_logs, list_mod_files, load_launch_profile, load_prism_instance_config,
    modrinth_get_project_details, modrinth_resolve_primary_file,
    modrinth_search_projects_by_type_paged, parse_s3_time, read_log_preview,
    refresh_microsoft_account, rename_instance, save_launch_profile, scan_instances,
    start_microsoft_device_code, sync_modrinth_managed_mods, update_installed_modrinth_mods,
    validate_minecraft_account,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use zip::ZipArchive;

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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum SettingsTarget {
    Global,
    Instance,
}

impl Default for SettingsTarget {
    fn default() -> Self {
        Self::Global
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
    ShaderPacks,
    Worlds,
    Servers,
    Screenshots,
}

impl DownloadContentType {
    fn modrinth_project_type(&self) -> &'static str {
        match self {
            Self::Mods => "mod",
            Self::ResourcePacks => "resourcepack",
            Self::ShaderPacks => "shader",
            Self::Worlds | Self::Servers | Self::Screenshots => "mod",
        }
    }

    fn curseforge_class(&self) -> &'static str {
        match self {
            Self::Mods => "mc-mods",
            Self::ResourcePacks => "texture-packs",
            Self::ShaderPacks => "shader-packs",
            Self::Worlds | Self::Servers | Self::Screenshots => "mc-mods",
        }
    }

    fn curseforge_class_id(&self) -> i32 {
        match self {
            Self::Mods => 6,
            Self::ResourcePacks => 12,
            Self::ShaderPacks => 6552,
            Self::Worlds | Self::Servers | Self::Screenshots => 6,
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

#[derive(Clone, Debug, PartialEq, Eq)]
enum CreateMode {
    Custom,
    Import,
}

impl Default for CreateMode {
    fn default() -> Self {
        Self::Custom
    }
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
    loader: String,
    running: bool,
    path: String,
    icon_path: Option<String>,
    group: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct InstanceGroupMeta {
    name: String,
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

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ContentListEntry {
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
    refresh_token: Option<String>,
    uuid: Option<String>,
    licensed: bool,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            name: "Offline".to_string(),
            active: false,
            account_type: AccountType::Offline,
            access_token: None,
            refresh_token: None,
            uuid: None,
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
            min_memory_mb: 512,
            max_memory_mb: suitable_default_max_mem_mb(),
            permgen_mb: 128,
            advanced_jvm_args: String::new(),
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
    #[serde(default)]
    last_seen_launcher_version: String,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            selected: None,
            show_news: true,
            filter: String::new(),
            active_tab: CenterTab::Overview,
            last_seen_launcher_version: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PersistedRunningProcess {
    instance_path: String,
    pid: u32,
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

fn groups_file_path() -> PathBuf {
    local_data_root().join("instance_groups.json")
}

fn running_processes_file_path() -> PathBuf {
    local_data_root().join("running_processes.json")
}

fn instances_root_path() -> PathBuf {
    local_data_root().join("instances")
}

const MSA_CLIENT_ID: &str = "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb";
const FLAME_API_KEY: &str = "$2a$10$wuAJuNZuted3NORVmpgUC.m8sI.pv1tOPKZyBgLFGjxFp/br0lZCC";
const OFFLINE_SKIN_ID: &str = "d1bf6a06a65d674a";
const LAUNCHER_VERSION_MAJOR: u32 = 1;
const LAUNCHER_VERSION_BUILD: u32 = 11;

fn launcher_version_string() -> String {
    format!("{LAUNCHER_VERSION_MAJOR}.{LAUNCHER_VERSION_BUILD:07}")
}

enum DeviceLoginEvent {
    Success(LicensedMicrosoftAccount),
    Error(String),
}

#[derive(Clone, Debug)]
enum AccountAvatarEvent {
    Ready { key: String },
    Error { key: String },
}

#[derive(Clone, Debug)]
enum ScreenshotThumbEvent {
    Ready {
        source_path: String,
        thumb_path: String,
    },
    Error {
        source_path: String,
    },
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

#[derive(Clone, Debug)]
enum LaunchWorkerEvent {
    Status {
        instance_path: String,
        message: String,
    },
    Ready {
        instance_path: String,
        profile: LaunchProfile,
    },
    Error {
        instance_path: String,
        message: String,
    },
    Progress {
        instance_path: String,
        progress: RuntimeDownloadProgress,
    },
}

#[derive(Clone, Debug)]
enum ImportWorkerEvent {
    Progress {
        stage: String,
        done: usize,
        total: usize,
        message: String,
    },
    Finished {
        instance_name: String,
        instance_path: PathBuf,
        group: String,
        summary: ImportSummary,
    },
    Error {
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
struct PendingLaunchContext {
    instance: Instance,
    pre_commands: Vec<String>,
    env_vars: Vec<(String, String)>,
    post_exit: Option<String>,
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

#[derive(Clone, Debug, Default)]
struct ImportUiProgress {
    stage: String,
    done: usize,
    total: usize,
    message: String,
}

#[derive(Clone, Debug)]
struct LaunchToast {
    title: String,
    subtitle: String,
    icon_path: Option<String>,
    loader: String,
    is_error: bool,
    shown_at: Instant,
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
    groups: Vec<InstanceGroupMeta>,
    accounts: Vec<Account>,
    selected: Option<usize>,
    last_selected: Option<usize>,
    show_news: bool,
    status: String,
    filter: String,
    data_root: PathBuf,
    active_tab: CenterTab,
    show_create_dialog: bool,
    create_mode: CreateMode,
    create_name: String,
    create_group: String,
    create_game_version: String,
    create_loader: Option<CreateLoader>,
    create_import_path: String,
    show_rename_dialog: bool,
    rename_name: String,
    show_copy_dialog: bool,
    copy_name: String,
    show_delete_dialog: bool,
    show_create_group_dialog: bool,
    show_rename_group_dialog: bool,
    show_set_icon_dialog: bool,
    set_icon_target_instance_path: Option<String>,
    set_icon_dialog_section: u8,
    set_icon_search: String,
    set_icon_selected_kind: u8,
    set_icon_selected_value: String,
    create_group_name: String,
    rename_group_old: String,
    rename_group_new: String,
    mods_cache: Vec<ModListEntry>,
    resourcepacks_cache: Vec<ContentListEntry>,
    shaderpacks_cache: Vec<ContentListEntry>,
    worlds_cache: Vec<ContentListEntry>,
    servers_cache: Vec<ContentListEntry>,
    screenshots_cache: Vec<ContentListEntry>,
    mods_filter: String,
    logs_cache: Vec<(String, String)>,
    selected_log: Option<usize>,
    log_preview: String,
    launch_profile: LaunchProfile,
    processes: HashMap<String, Child>,
    show_add_account_dialog: bool,
    show_manage_accounts_dialog: bool,
    manage_account_selected: Option<usize>,
    new_account_name: String,
    new_account_token: String,
    new_offline_account_name: String,
    device_login_info: Option<MicrosoftDeviceCode>,
    device_login_receiver: Option<Receiver<DeviceLoginEvent>>,
    device_login_status: String,
    device_login_qr_payload: String,
    account_avatar_tx: Sender<AccountAvatarEvent>,
    account_avatar_rx: Receiver<AccountAvatarEvent>,
    account_avatar_pending: HashSet<String>,
    screenshot_thumb_tx: Sender<ScreenshotThumbEvent>,
    screenshot_thumb_rx: Receiver<ScreenshotThumbEvent>,
    screenshot_thumb_pending: HashSet<String>,
    screenshot_thumb_cache: HashMap<String, String>,
    show_download_panel: bool,
    content_list_ratio: f32,
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
    settings_target: SettingsTarget,
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
    download_search_last_dispatched_query: String,
    download_details_receiver: Option<Receiver<DownloadDetailsEvent>>,
    download_details_request_id: u64,
    download_details_cache: HashMap<String, (DownloadDetails, Instant)>,
    download_jobs: Vec<DownloadJob>,
    next_download_job_id: u64,
    download_queue_tx: Sender<DownloadQueueEvent>,
    download_queue_rx: Receiver<DownloadQueueEvent>,
    max_parallel_downloads: usize,
    launch_worker_tx: Sender<LaunchWorkerEvent>,
    launch_worker_rx: Receiver<LaunchWorkerEvent>,
    launch_in_progress: HashSet<String>,
    pending_launches: HashMap<String, PendingLaunchContext>,
    launch_progress: HashMap<String, RuntimeDownloadProgress>,
    import_worker_rx: Option<Receiver<ImportWorkerEvent>>,
    import_progress: Option<ImportUiProgress>,
    create_versions_loading: bool,
    create_versions: Vec<String>,
    create_versions_receiver: Option<Receiver<Result<Vec<String>, String>>>,
    dragging_instance_path: Option<String>,
    last_stopped_instance_path: Option<String>,
    launch_toast: Option<LaunchToast>,
    running_process_pids: HashMap<String, u32>,
    show_version_update_dialog: bool,
    previous_launcher_version: Option<String>,
    last_seen_launcher_version: String,
}

impl Default for PrismarineApp {
    fn default() -> Self {
        let persisted = load_state().unwrap_or_default();
        let data_root = local_data_root();
        let _ = fs::create_dir_all(instances_root_path());
        let groups = load_instance_groups();
        let mut accounts = load_accounts();
        if accounts.is_empty() {
            accounts = vec![
                Account {
                    name: "Default Account".to_string(),
                    active: true,
                    account_type: AccountType::Offline,
                    access_token: None,
                    refresh_token: None,
                    uuid: None,
                    licensed: false,
                },
                Account {
                    name: "Offline".to_string(),
                    active: false,
                    account_type: AccountType::Offline,
                    access_token: None,
                    refresh_token: None,
                    uuid: None,
                    licensed: false,
                },
            ];
        }
        let (download_queue_tx, download_queue_rx) = mpsc::channel::<DownloadQueueEvent>();
        let (launch_worker_tx, launch_worker_rx) = mpsc::channel::<LaunchWorkerEvent>();
        let (account_avatar_tx, account_avatar_rx) = mpsc::channel::<AccountAvatarEvent>();
        let (screenshot_thumb_tx, screenshot_thumb_rx) = mpsc::channel::<ScreenshotThumbEvent>();
        let current_version = launcher_version_string();
        let previous_version = persisted.last_seen_launcher_version.trim().to_string();
        let show_version_update_dialog =
            !previous_version.is_empty() && previous_version != current_version;
        let mut app = Self {
            instances: Vec::new(),
            groups,
            accounts,
            selected: persisted.selected,
            last_selected: None,
            show_news: persisted.show_news,
            status: "Ready".to_string(),
            filter: persisted.filter,
            data_root,
            active_tab: persisted.active_tab,
            show_create_dialog: false,
            create_mode: CreateMode::Custom,
            create_name: String::new(),
            create_group: String::new(),
            create_game_version: String::new(),
            create_loader: None,
            create_import_path: String::new(),
            show_rename_dialog: false,
            rename_name: String::new(),
            show_copy_dialog: false,
            copy_name: String::new(),
            show_delete_dialog: false,
            show_create_group_dialog: false,
            show_rename_group_dialog: false,
            show_set_icon_dialog: false,
            set_icon_target_instance_path: None,
            set_icon_dialog_section: 0,
            set_icon_search: String::new(),
            set_icon_selected_kind: 0,
            set_icon_selected_value: String::new(),
            create_group_name: String::new(),
            rename_group_old: String::new(),
            rename_group_new: String::new(),
            mods_cache: Vec::new(),
            resourcepacks_cache: Vec::new(),
            shaderpacks_cache: Vec::new(),
            worlds_cache: Vec::new(),
            servers_cache: Vec::new(),
            screenshots_cache: Vec::new(),
            mods_filter: String::new(),
            logs_cache: Vec::new(),
            selected_log: None,
            log_preview: String::new(),
            launch_profile: default_launch_profile(Path::new(".")),
            processes: HashMap::new(),
            show_add_account_dialog: false,
            show_manage_accounts_dialog: false,
            manage_account_selected: None,
            new_account_name: String::new(),
            new_account_token: String::new(),
            new_offline_account_name: String::new(),
            device_login_info: None,
            device_login_receiver: None,
            device_login_status: String::new(),
            device_login_qr_payload: String::new(),
            account_avatar_tx,
            account_avatar_rx,
            account_avatar_pending: HashSet::new(),
            screenshot_thumb_tx,
            screenshot_thumb_rx,
            screenshot_thumb_pending: HashSet::new(),
            screenshot_thumb_cache: HashMap::new(),
            show_download_panel: false,
            content_list_ratio: 0.58,
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
            settings_target: SettingsTarget::Global,
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
            download_search_last_dispatched_query: String::new(),
            download_details_receiver: None,
            download_details_request_id: 0,
            download_details_cache: HashMap::new(),
            download_jobs: Vec::new(),
            next_download_job_id: 1,
            download_queue_tx,
            download_queue_rx,
            max_parallel_downloads: 2,
            launch_worker_tx,
            launch_worker_rx,
            launch_in_progress: HashSet::new(),
            pending_launches: HashMap::new(),
            launch_progress: HashMap::new(),
            import_worker_rx: None,
            import_progress: None,
            create_versions_loading: false,
            create_versions: Vec::new(),
            create_versions_receiver: None,
            dragging_instance_path: None,
            last_stopped_instance_path: None,
            launch_toast: None,
            running_process_pids: HashMap::new(),
            show_version_update_dialog,
            previous_launcher_version: if show_version_update_dialog {
                Some(previous_version)
            } else {
                None
            },
            last_seen_launcher_version: current_version,
        };
        app.reload_instances();
        app.restore_running_processes();
        app.sync_process_states();
        app
    }
}

impl PrismarineApp {
    fn is_pid_alive_for_instance(instance_path: &str, pid: u32) -> bool {
        #[cfg(target_os = "linux")]
        {
            let proc_dir = PathBuf::from(format!("/proc/{pid}"));
            if !proc_dir.is_dir() {
                return false;
            }
            let cwd = match fs::read_link(proc_dir.join("cwd")) {
                Ok(v) => v,
                Err(_) => return false,
            };
            let inst = Path::new(instance_path);
            cwd.starts_with(inst)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = instance_path;
            let _ = pid;
            false
        }
    }

    fn persist_running_processes(&self) {
        let list: Vec<PersistedRunningProcess> = self
            .running_process_pids
            .iter()
            .map(|(instance_path, pid)| PersistedRunningProcess {
                instance_path: instance_path.clone(),
                pid: *pid,
            })
            .collect();
        let path = running_processes_file_path();
        if list.is_empty() {
            let _ = fs::remove_file(path);
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(
            &path,
            serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".to_string()),
        );
    }

    fn restore_running_processes(&mut self) {
        let path = running_processes_file_path();
        let Ok(text) = fs::read_to_string(path) else {
            return;
        };
        let Ok(list) = serde_json::from_str::<Vec<PersistedRunningProcess>>(&text) else {
            return;
        };
        self.running_process_pids.clear();
        for item in list {
            if Self::is_pid_alive_for_instance(&item.instance_path, item.pid) {
                self.running_process_pids
                    .insert(item.instance_path.clone(), item.pid);
            }
        }
        self.persist_running_processes();
    }

    fn running_pid_for_instance(&mut self, instance_path: &str) -> Option<u32> {
        let pid = self.running_process_pids.get(instance_path).copied()?;
        if Self::is_pid_alive_for_instance(instance_path, pid) {
            Some(pid)
        } else {
            self.running_process_pids.remove(instance_path);
            self.persist_running_processes();
            None
        }
    }

    fn selected_instance(&self) -> Option<&Instance> {
        self.selected.and_then(|i| self.instances.get(i))
    }

    fn make_launch_toast_for_instance(
        &mut self,
        instance: &Instance,
        title: &str,
        subtitle: String,
        is_error: bool,
    ) {
        self.launch_toast = Some(LaunchToast {
            title: title.to_string(),
            subtitle,
            icon_path: instance.icon_path.clone(),
            loader: instance.loader.clone(),
            is_error,
            shown_at: Instant::now(),
        });
    }

    fn make_launch_toast_for_path(
        &mut self,
        instance_path: &str,
        title: &str,
        subtitle: String,
        is_error: bool,
    ) {
        if let Some(instance) = self
            .instances
            .iter()
            .find(|i| i.path == instance_path)
            .cloned()
        {
            self.make_launch_toast_for_instance(&instance, title, subtitle, is_error);
        } else {
            self.launch_toast = Some(LaunchToast {
                title: title.to_string(),
                subtitle,
                icon_path: None,
                loader: "vanilla".to_string(),
                is_error,
                shown_at: Instant::now(),
            });
        }
    }

    fn group_icon_path(&self, group_name: &str) -> Option<String> {
        self.groups
            .iter()
            .find(|g| g.name == group_name)
            .and_then(|g| g.icon_path.clone())
    }

    fn all_group_names(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .groups
            .iter()
            .map(|g| g.name.clone())
            .filter(|x| !x.trim().is_empty())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn sync_groups_with_instances(&mut self) {
        let mut names = self.all_group_names();
        for i in &self.instances {
            let g = i.group.trim();
            if !g.is_empty() && !names.iter().any(|x| x == g) {
                names.push(g.to_string());
            }
        }
        names.sort();
        let mut merged = Vec::new();
        for n in names {
            let existing = self.groups.iter().find(|g| g.name == n).cloned();
            merged.push(existing.unwrap_or(InstanceGroupMeta {
                name: n,
                icon_path: None,
            }));
        }
        self.groups = merged;
        save_instance_groups(&self.groups);
    }

    fn do_create_group(&mut self) {
        let name = self.create_group_name.trim();
        if name.is_empty() {
            self.status = "Group name must not be empty".to_string();
            return;
        }
        if self
            .groups
            .iter()
            .any(|g| g.name.eq_ignore_ascii_case(name))
        {
            self.status = format!("Group already exists: {name}");
            return;
        }
        self.groups.push(InstanceGroupMeta {
            name: name.to_string(),
            icon_path: None,
        });
        self.groups.sort_by(|a, b| a.name.cmp(&b.name));
        save_instance_groups(&self.groups);
        self.create_group = name.to_string();
        self.status = format!("Created group: {name}");
        self.create_group_name.clear();
        self.show_create_group_dialog = false;
    }

    fn do_rename_group(&mut self) {
        let old_name = self.rename_group_old.trim().to_string();
        let new_name = self.rename_group_new.trim().to_string();
        if old_name.is_empty() || new_name.is_empty() {
            self.status = "Old and new group names are required".to_string();
            return;
        }
        if old_name == new_name {
            self.status = "Group name is unchanged".to_string();
            return;
        }
        if self
            .groups
            .iter()
            .any(|g| g.name.eq_ignore_ascii_case(&new_name))
        {
            self.status = format!("Group already exists: {new_name}");
            return;
        }
        let mut found = false;
        for g in &mut self.groups {
            if g.name == old_name {
                g.name = new_name.clone();
                found = true;
                break;
            }
        }
        if !found {
            self.status = format!("Group not found: {old_name}");
            return;
        }
        for instance in &mut self.instances {
            if instance.group == old_name {
                instance.group = new_name.clone();
                let _ = set_instance_cfg_value(Path::new(&instance.path), "Group", Some(&new_name));
            }
        }
        self.groups.sort_by(|a, b| a.name.cmp(&b.name));
        save_instance_groups(&self.groups);
        if self.create_group == old_name {
            self.create_group = new_name.clone();
        }
        self.status = format!("Renamed group: {old_name} -> {new_name}");
        self.show_rename_group_dialog = false;
        self.rename_group_old.clear();
        self.rename_group_new.clear();
    }

    fn do_assign_selected_instance_group(&mut self, group_name: &str) {
        let Some(idx) = self.selected else {
            self.status = "No instance selected".to_string();
            return;
        };
        if idx >= self.instances.len() {
            return;
        }
        let path = self.instances[idx].path.clone();
        self.do_assign_instance_group_by_path(&path, group_name);
    }

    fn do_assign_instance_group_by_path(&mut self, instance_path: &str, group_name: &str) {
        let Some(idx) = self.instances.iter().position(|x| x.path == instance_path) else {
            self.status = "Instance not found".to_string();
            return;
        };
        self.instances[idx].group = group_name.to_string();
        let path = PathBuf::from(&self.instances[idx].path);
        let write_result = if group_name.trim().is_empty() {
            set_instance_cfg_value(&path, "Group", None)
        } else {
            set_instance_cfg_value(&path, "Group", Some(group_name))
        };
        match write_result {
            Ok(_) => {
                self.sync_groups_with_instances();
                self.status = if group_name.trim().is_empty() {
                    format!("Removed instance from group: {}", self.instances[idx].name)
                } else {
                    format!("Moved {} to group {}", self.instances[idx].name, group_name)
                };
            }
            Err(err) => {
                self.status = format!("Failed to update instance group: {err}");
            }
        }
    }

    fn do_set_selected_instance_loader(&mut self, loader: Option<CreateLoader>) {
        let Some(instance_path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        let instance_path_s = instance_path.display().to_string();
        let previous_selected_path = self.selected_instance().map(|x| x.path.clone());
        let version = self.detect_instance_version(&instance_path);

        let mmc_pack_path = instance_path.join("mmc-pack.json");
        let mut mmc_json = if let Ok(text) = fs::read_to_string(&mmc_pack_path) {
            serde_json::from_str::<serde_json::Value>(&text).unwrap_or_else(|_| {
                serde_json::json!({
                    "formatVersion": 1,
                    "components": []
                })
            })
        } else {
            serde_json::json!({
                "formatVersion": 1,
                "components": []
            })
        };

        if mmc_json.get("formatVersion").is_none() {
            mmc_json["formatVersion"] = serde_json::json!(1);
        }
        if !mmc_json.get("components").is_some_and(|v| v.is_array()) {
            mmc_json["components"] = serde_json::json!([]);
        }

        let mut components = mmc_json
            .get("components")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if !components.iter().any(|c| {
            c.get("uid")
                .and_then(|v| v.as_str())
                .is_some_and(|u| u == "net.minecraft")
        }) {
            let mc_version = if version != "unknown" {
                version.clone()
            } else {
                "latest".to_string()
            };
            components.insert(
                0,
                serde_json::json!({
                    "uid": "net.minecraft",
                    "version": mc_version
                }),
            );
        }

        components.retain(|c| {
            let uid = c.get("uid").and_then(|v| v.as_str()).unwrap_or_default();
            uid != "net.fabricmc.fabric-loader"
                && uid != "org.quiltmc.quilt-loader"
                && uid != "net.minecraftforge"
                && uid != "net.neoforged"
                && !uid.contains("neoforge")
        });

        if let Some(loader) = &loader {
            components.push(serde_json::json!({
                "uid": loader.mmc_uid(),
                "version": "0.0.0"
            }));
        }

        mmc_json["components"] = serde_json::Value::Array(components);
        let mmc_text = serde_json::to_string_pretty(&mmc_json).unwrap_or_else(|_| "{}".to_string());
        if let Err(err) = fs::write(&mmc_pack_path, mmc_text) {
            self.status = format!("Failed to update loader in mmc-pack.json: {err}");
            return;
        }

        let managed_loader = loader
            .as_ref()
            .map(CreateLoader::cfg_value)
            .unwrap_or("vanilla");
        if let Err(err) =
            set_instance_cfg_value(&instance_path, "ManagedLoader", Some(managed_loader))
        {
            self.status = format!("Failed to update loader in instance.cfg: {err}");
            return;
        }

        if let Ok(mut profile) = load_launch_profile(&instance_path) {
            profile.classpath.clear();
            if let Err(err) = save_launch_profile(&instance_path, &profile) {
                self.status = format!("Failed to reset launch profile after loader change: {err}");
                return;
            }
        }

        self.reload_instances();
        if let Some(prev_path) = previous_selected_path
            && let Some(pos) = self.instances.iter().position(|x| x.path == prev_path)
        {
            self.selected = Some(pos);
        } else if let Some(pos) = self
            .instances
            .iter()
            .position(|x| x.path == instance_path_s)
        {
            self.selected = Some(pos);
        }
        self.refresh_selected_content();
        self.status = match loader {
            Some(loader) => format!("Loader updated: {}", loader.label()),
            None => "Loader removed (Vanilla)".to_string(),
        };
    }

    #[allow(dead_code)]
    fn do_set_instance_icon(&mut self) {
        let Some(idx) = self.selected else {
            self.status = "No instance selected".to_string();
            return;
        };
        if idx >= self.instances.len() {
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Images",
                &["png", "jpg", "jpeg", "ico", "webp", "gif", "svg"],
            )
            .pick_file()
        else {
            return;
        };
        let Some(icon_key) =
            copy_image_to_icon_store(&self.data_root, &path, &self.instances[idx].name)
        else {
            self.status = "Failed to import instance icon".to_string();
            return;
        };
        match set_instance_cfg_value(
            Path::new(&self.instances[idx].path),
            "iconKey",
            Some(&icon_key),
        ) {
            Ok(_) => {
                self.status = format!("Updated icon for {}", self.instances[idx].name);
                self.reload_instances();
            }
            Err(err) => {
                self.status = format!("Failed to update instance icon: {err}");
            }
        }
    }

    fn open_set_icon_dialog_for_instance(&mut self, idx: usize) {
        if idx >= self.instances.len() {
            self.status = "No instance selected".to_string();
            return;
        }
        self.set_icon_target_instance_path = Some(self.instances[idx].path.clone());
        self.set_icon_dialog_section = 0;
        self.set_icon_search.clear();
        self.set_icon_selected_kind = 0;
        self.set_icon_selected_value.clear();
        self.show_set_icon_dialog = true;
    }

    fn apply_instance_icon_key(&mut self, instance_path: &str, icon_key: Option<&str>) {
        let value = icon_key.and_then(|v| {
            let t = v.trim();
            if t.is_empty() { None } else { Some(t) }
        });
        match set_instance_cfg_value(Path::new(instance_path), "iconKey", value) {
            Ok(_) => {
                self.status = if let Some(key) = value {
                    format!("Updated instance icon: {key}")
                } else {
                    "Instance icon reset to default".to_string()
                };
                self.reload_instances();
            }
            Err(err) => {
                self.status = format!("Failed to set instance icon: {err}");
            }
        }
    }

    fn write_builtin_icon_to_store(&self, key: &str, ext: &str, bytes: &[u8]) -> Option<String> {
        let icons_dir = self.data_root.join("icons");
        let _ = fs::create_dir_all(&icons_dir);
        let target = icons_dir.join(format!("{key}.{ext}"));
        if !target.is_file() {
            fs::write(&target, bytes).ok()?;
        }
        Some(key.to_string())
    }

    fn import_custom_instance_icon(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpeg", "jpg", "webp", "gif", "svg"])
            .pick_file()
        else {
            return;
        };
        let Some(instance_path) = self.set_icon_target_instance_path.clone() else {
            self.status = "No target instance for icon".to_string();
            return;
        };
        let base_name = Path::new(&instance_path)
            .file_name()
            .and_then(|x| x.to_str())
            .unwrap_or("instance");
        let Some(icon_key) = copy_image_to_icon_store(&self.data_root, &path, base_name) else {
            self.status = "Failed to import custom icon".to_string();
            return;
        };
        self.apply_instance_icon_key(&instance_path, Some(&icon_key));
        self.show_set_icon_dialog = false;
    }

    fn list_custom_icon_files(&self) -> Vec<String> {
        let mut out = Vec::new();
        let icons_dir = self.data_root.join("icons");
        let Ok(entries) = fs::read_dir(&icons_dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg"
            ) {
                out.push(path.display().to_string());
            }
        }
        out.sort();
        out
    }

    fn do_set_group_icon(&mut self, group_name: &str) {
        let group_name = group_name.trim().to_string();
        if group_name.is_empty() {
            self.status = "Select group first".to_string();
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "ico", "webp", "gif"])
            .pick_file()
        else {
            return;
        };
        let Some(stored) = copy_image_to_group_store(&self.data_root, &path, &group_name) else {
            self.status = "Failed to import group icon".to_string();
            return;
        };
        if let Some(g) = self.groups.iter_mut().find(|g| g.name == group_name) {
            g.icon_path = Some(stored);
            save_instance_groups(&self.groups);
            self.status = format!("Updated icon for group {group_name}");
        }
    }

    fn do_delete_group(&mut self, group_name: &str) {
        let group_name = group_name.trim();
        if group_name.is_empty() {
            self.status = "Cannot delete default group".to_string();
            return;
        }
        for instance in &mut self.instances {
            if instance.group == group_name {
                instance.group.clear();
                let _ = set_instance_cfg_value(Path::new(&instance.path), "Group", None);
            }
        }
        self.groups.retain(|g| g.name != group_name);
        save_instance_groups(&self.groups);
        self.status = format!("Deleted group: {group_name}");
    }

    fn active_account(&self) -> Option<&Account> {
        self.accounts.iter().find(|x| x.active)
    }

    fn apply_account_launch_args(&mut self, game_args: &mut Vec<String>) {
        remove_arg_pair(game_args, "--username");
        remove_arg_pair(game_args, "--uuid");
        remove_arg_pair(game_args, "--accessToken");
        remove_arg_pair(game_args, "--userType");
        remove_arg_pair(game_args, "--versionType");

        let active_idx = self.accounts.iter().position(|x| x.active);
        if let Some(i) = active_idx {
            if self.accounts[i].account_type == AccountType::Licensed {
                let mut changed = false;
                let mut token_ok = false;
                if let Some(token) = self.accounts[i].access_token.clone()
                    && let Ok(validation) = validate_minecraft_account(&token)
                {
                    self.accounts[i].uuid = Some(validation.uuid);
                    self.accounts[i].name = validation.username;
                    self.accounts[i].licensed = validation.has_minecraft_license;
                    token_ok = true;
                    changed = true;
                }
                if !token_ok && let Some(refresh) = self.accounts[i].refresh_token.clone() {
                    match refresh_microsoft_account(MSA_CLIENT_ID, &refresh) {
                        Ok(refreshed) => {
                            self.accounts[i].name = refreshed.username;
                            self.accounts[i].uuid = Some(refreshed.uuid);
                            self.accounts[i].access_token = Some(refreshed.access_token);
                            self.accounts[i].refresh_token = refreshed.refresh_token;
                            self.accounts[i].licensed = refreshed.has_minecraft_license;
                            changed = true;
                            self.status = format!(
                                "Refreshed Microsoft session for {}",
                                self.accounts[i].name
                            );
                        }
                        Err(err) => {
                            self.accounts[i].licensed = false;
                            changed = true;
                            self.status =
                                format!("Microsoft session expired, re-login required: {err}");
                        }
                    }
                } else if !token_ok {
                    self.accounts[i].licensed = false;
                    changed = true;
                    self.status = "Microsoft session expired, re-login required".to_string();
                }
                if changed {
                    save_accounts(&self.accounts);
                }
            }
        }

        let account = self.active_account().cloned().unwrap_or(Account {
            name: "Player".to_string(),
            active: true,
            account_type: AccountType::Offline,
            access_token: None,
            refresh_token: None,
            uuid: None,
            licensed: false,
        });
        let username = account.name;
        let (uuid, access_token, user_type) = if account.account_type == AccountType::Licensed {
            let normalized_uuid = account
                .uuid
                .as_deref()
                .map(normalize_minecraft_uuid)
                .unwrap_or_else(|| pseudo_uuid_from_name(&username));
            (
                normalized_uuid,
                account.access_token.unwrap_or_else(|| "0".to_string()),
                "msa".to_string(),
            )
        } else {
            (
                pseudo_uuid_from_name(&username),
                "0".to_string(),
                "offline".to_string(),
            )
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

    fn clear_active_account(&mut self) {
        for account in &mut self.accounts {
            account.active = false;
        }
        save_accounts(&self.accounts);
    }

    fn add_offline_account(&mut self) {
        let raw = self.new_offline_account_name.trim().to_string();
        let name = if raw.is_empty() {
            let mut idx = self.accounts.len() + 1;
            loop {
                let candidate = format!("Offline{idx}");
                if !self.accounts.iter().any(|a| a.name == candidate) {
                    break candidate;
                }
                idx += 1;
            }
        } else {
            raw
        };
        self.accounts.push(Account {
            name: name.clone(),
            active: false,
            account_type: AccountType::Offline,
            access_token: None,
            refresh_token: None,
            uuid: None,
            licensed: false,
        });
        save_accounts(&self.accounts);
        self.status = format!("Offline account added: {name}");
        self.new_offline_account_name.clear();
    }

    fn delete_selected_account(&mut self) {
        let Some(idx) = self.manage_account_selected else {
            self.status = "Select account first".to_string();
            return;
        };
        if idx >= self.accounts.len() {
            return;
        }
        let removed = self.accounts.remove(idx);
        if self.accounts.is_empty() {
            self.accounts.push(Account {
                name: "Offline".to_string(),
                active: true,
                account_type: AccountType::Offline,
                access_token: None,
                refresh_token: None,
                uuid: None,
                licensed: false,
            });
        }
        if !self.accounts.iter().any(|a| a.active)
            && let Some(first) = self.accounts.first_mut()
        {
            first.active = true;
        }
        self.manage_account_selected = self.accounts.get(idx).map(|_| idx).or_else(|| {
            if self.accounts.is_empty() {
                None
            } else {
                Some(self.accounts.len() - 1)
            }
        });
        save_accounts(&self.accounts);
        self.status = format!("Removed account: {}", removed.name);
    }

    fn move_selected_account(&mut self, delta: isize) {
        let Some(idx) = self.manage_account_selected else {
            return;
        };
        let new_idx = if delta < 0 {
            idx.saturating_sub(delta.unsigned_abs())
        } else {
            idx.saturating_add(delta as usize)
        };
        if idx >= self.accounts.len() || new_idx >= self.accounts.len() || new_idx == idx {
            return;
        }
        self.accounts.swap(idx, new_idx);
        self.manage_account_selected = Some(new_idx);
        save_accounts(&self.accounts);
    }

    fn refresh_selected_account(&mut self) {
        let Some(idx) = self.manage_account_selected else {
            self.status = "Select account first".to_string();
            return;
        };
        if idx >= self.accounts.len() {
            self.status = "Select account first".to_string();
            return;
        }
        if self.accounts[idx].account_type != AccountType::Licensed {
            self.status = "Offline account does not need refresh".to_string();
            return;
        }

        if let Some(refresh) = self.accounts[idx].refresh_token.clone() {
            match refresh_microsoft_account(MSA_CLIENT_ID, &refresh) {
                Ok(refreshed) => {
                    self.accounts[idx].name = refreshed.username;
                    self.accounts[idx].uuid = Some(refreshed.uuid);
                    self.accounts[idx].access_token = Some(refreshed.access_token);
                    self.accounts[idx].refresh_token = refreshed.refresh_token;
                    self.accounts[idx].licensed = refreshed.has_minecraft_license;
                    save_accounts(&self.accounts);
                    self.status =
                        format!("Microsoft account refreshed: {}", self.accounts[idx].name);
                }
                Err(err) => {
                    self.accounts[idx].licensed = false;
                    save_accounts(&self.accounts);
                    self.status = format!("Account refresh failed, re-login required: {err}");
                }
            }
            return;
        }

        if let Some(token) = self.accounts[idx].access_token.clone() {
            match validate_minecraft_account(&token) {
                Ok(validation) => {
                    self.accounts[idx].name = validation.username;
                    self.accounts[idx].uuid = Some(validation.uuid);
                    self.accounts[idx].licensed = validation.has_minecraft_license;
                    save_accounts(&self.accounts);
                    self.status = format!("Account checked: {}", self.accounts[idx].name);
                }
                Err(err) => {
                    self.accounts[idx].licensed = false;
                    save_accounts(&self.accounts);
                    self.status = format!("Account token expired, re-login required: {err}");
                }
            }
        } else {
            self.status = "No token found for selected account".to_string();
        }
    }

    fn account_head_cache_path(&self, name: &str) -> PathBuf {
        let safe = sanitize_key_fragment(name);
        self.data_root
            .join("cache")
            .join("account_heads")
            .join(format!("{safe}.png"))
    }

    fn ensure_account_head_cached_async(&mut self, name: &str) -> Option<String> {
        let key = name.trim().to_string();
        if key.is_empty() {
            return None;
        }
        let target = self.account_head_cache_path(&key);
        if target.is_file() {
            return Some(target.display().to_string());
        }
        if self.account_avatar_pending.contains(&key) {
            return None;
        }
        self.account_avatar_pending.insert(key.clone());
        let tx = self.account_avatar_tx.clone();
        let urls = minecraft_head_icon_urls(&key);
        std::thread::spawn(move || {
            let mut ok = false;
            for url in urls {
                if download_file_to_path(&url, &target).is_ok() {
                    ok = true;
                    break;
                }
            }
            let _ = if ok {
                tx.send(AccountAvatarEvent::Ready { key })
            } else {
                tx.send(AccountAvatarEvent::Error { key })
            };
        });
        None
    }

    fn poll_account_avatar_events(&mut self) {
        loop {
            let Ok(event) = self.account_avatar_rx.try_recv() else {
                break;
            };
            match event {
                AccountAvatarEvent::Ready { key } => {
                    self.account_avatar_pending.remove(&key);
                }
                AccountAvatarEvent::Error { key } => {
                    self.account_avatar_pending.remove(&key);
                }
            }
        }
    }

    fn screenshot_thumb_cache_path(&self, source_path: &str) -> PathBuf {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        source_path.hash(&mut h);
        self.data_root
            .join("cache")
            .join("screenshot_thumbs")
            .join(format!("{:016x}.png", h.finish()))
    }

    fn ensure_screenshot_thumbnail_texture(
        &mut self,
        ctx: &egui::Context,
        source_path: &str,
    ) -> Option<egui::TextureHandle> {
        if let Some(cached_path) = self.screenshot_thumb_cache.get(source_path).cloned()
            && Path::new(&cached_path).is_file()
        {
            return self.ensure_icon_texture(ctx, &cached_path);
        }
        let thumb_path = self.screenshot_thumb_cache_path(source_path);
        if thumb_path.is_file() {
            let thumb_s = thumb_path.display().to_string();
            self.screenshot_thumb_cache
                .insert(source_path.to_string(), thumb_s.clone());
            return self.ensure_icon_texture(ctx, &thumb_s);
        }
        if self.screenshot_thumb_pending.contains(source_path) {
            return None;
        }
        self.screenshot_thumb_pending
            .insert(source_path.to_string());
        let tx = self.screenshot_thumb_tx.clone();
        let source = source_path.to_string();
        std::thread::spawn(move || {
            let source_pb = PathBuf::from(&source);
            let thumb_pb = local_data_root()
                .join("cache")
                .join("screenshot_thumbs")
                .join({
                    let mut hh = std::collections::hash_map::DefaultHasher::new();
                    source.hash(&mut hh);
                    format!("{:016x}.png", hh.finish())
                });
            let _ = fs::create_dir_all(
                thumb_pb
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| local_data_root().join("cache").join("screenshot_thumbs")),
            );
            let ok = (|| -> Option<()> {
                let bytes = fs::read(&source_pb).ok()?;
                let image = image::load_from_memory(&bytes).ok()?;
                let thumb = image.thumbnail(320, 180).to_rgba8();
                image::DynamicImage::ImageRgba8(thumb)
                    .save_with_format(&thumb_pb, image::ImageFormat::Png)
                    .ok()?;
                Some(())
            })()
            .is_some();
            let _ = if ok {
                tx.send(ScreenshotThumbEvent::Ready {
                    source_path: source.clone(),
                    thumb_path: thumb_pb.display().to_string(),
                })
            } else {
                tx.send(ScreenshotThumbEvent::Error {
                    source_path: source,
                })
            };
        });
        None
    }

    fn poll_screenshot_thumbnail_events(&mut self) {
        loop {
            let Ok(event) = self.screenshot_thumb_rx.try_recv() else {
                break;
            };
            match event {
                ScreenshotThumbEvent::Ready {
                    source_path,
                    thumb_path,
                } => {
                    self.screenshot_thumb_pending.remove(&source_path);
                    self.screenshot_thumb_cache.insert(source_path, thumb_path);
                }
                ScreenshotThumbEvent::Error { source_path } => {
                    self.screenshot_thumb_pending.remove(&source_path);
                }
            }
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
                    active: true,
                    account_type: AccountType::Licensed,
                    access_token: Some(token.to_string()),
                    refresh_token: None,
                    uuid: Some(validation.uuid),
                    licensed: validation.has_minecraft_license,
                });
                if let Some(last) = self.accounts.len().checked_sub(1) {
                    self.set_active_account(last);
                }
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
            active: true,
            account_type: AccountType::Licensed,
            access_token: Some(account.access_token),
            refresh_token: account.refresh_token,
            uuid: Some(account.uuid),
            licensed: account.has_minecraft_license,
        });
        if let Some(last) = self.accounts.len().checked_sub(1) {
            self.set_active_account(last);
        }
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
                // Show code/QR in UI immediately, then open browser in a non-blocking way.
                self.device_login_qr_payload = open_url.clone();
                self.device_login_status =
                    "Waiting for confirmation in browser/Microsoft...".to_string();
                self.device_login_info = Some(device.clone());
                let (tx, rx) = mpsc::channel::<DeviceLoginEvent>();
                let device_copy = device.clone();
                std::thread::spawn(move || {
                    let event = match complete_microsoft_device_login(MSA_CLIENT_ID, &device_copy) {
                        Ok(result) => DeviceLoginEvent::Success(result),
                        Err(err) => DeviceLoginEvent::Error(err),
                    };
                    let _ = tx.send(event);
                });

                self.device_login_receiver = Some(rx);
                self.status = "Microsoft login started".to_string();
                let _ = Command::new("sh")
                    .arg("-lc")
                    .arg(format!("xdg-open {}", shell_escape(&open_url)))
                    .spawn();
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
                self.device_login_status = format!("Login error: {err}");
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
        self.download_search_last_dispatched_query = query.clone();

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
                if count == 0
                    && !append
                    && !self.download_search_last_dispatched_query.trim().is_empty()
                {
                    let keep_query = self.modrinth_query.clone();
                    self.modrinth_query.clear();
                    self.start_download_search(false);
                    self.modrinth_query = keep_query;
                    self.status = "Modrinth: no results, showing popular projects".to_string();
                    return;
                }
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
                if count == 0
                    && !append
                    && !self.download_search_last_dispatched_query.trim().is_empty()
                {
                    let keep_query = self.curseforge_query.clone();
                    self.curseforge_query.clear();
                    self.start_download_search(false);
                    self.curseforge_query = keep_query;
                    self.status = "CurseForge: no results, showing popular projects".to_string();
                    return;
                }
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
        normalize_markdown_html_images(&out)
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
        normalize_markdown_html_images(&out)
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
        if !matches!(
            self.download_content_type,
            DownloadContentType::Mods
                | DownloadContentType::ResourcePacks
                | DownloadContentType::ShaderPacks
        ) {
            self.status =
                "Downloads are only available for Mods/Resource Packs/Shader Packs".to_string();
            return;
        }
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
                DownloadContentType::ShaderPacks => String::new(),
                DownloadContentType::Worlds
                | DownloadContentType::Servers
                | DownloadContentType::Screenshots => String::new(),
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
        if !matches!(
            self.download_content_type,
            DownloadContentType::Mods
                | DownloadContentType::ResourcePacks
                | DownloadContentType::ShaderPacks
        ) {
            self.status =
                "Downloads are only available for Mods/Resource Packs/Shader Packs".to_string();
            return;
        }
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
        if !matches!(
            self.download_content_type,
            DownloadContentType::Mods
                | DownloadContentType::ResourcePacks
                | DownloadContentType::ShaderPacks
        ) {
            self.status =
                "Direct URL download is only available for Mods/Resource Packs/Shader Packs"
                    .to_string();
            return;
        }
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
                    DownloadContentType::ShaderPacks => "downloaded-shaderpack.zip",
                    DownloadContentType::Worlds
                    | DownloadContentType::Servers
                    | DownloadContentType::Screenshots => "downloaded-content.bin",
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
            self.download_jobs
                .retain(|j| j.state != DownloadJobState::Done);
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
            });
            egui::ScrollArea::vertical()
                .id_salt("left_download_queue_scroll")
                .max_height(ui.available_height().max(80.0))
                .show(ui, |ui| {
                    let start = self.download_jobs.len().saturating_sub(12);
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
                        }
                    }
                });
            if self
                .download_jobs
                .iter()
                .any(|j| j.state == DownloadJobState::Failed)
            {
                ui.horizontal(|ui| {
                    ui.small("Failed downloads remain in queue.");
                    if ui.button("Clear Failed").clicked() {
                        self.download_jobs
                            .retain(|j| j.state != DownloadJobState::Failed);
                    }
                });
            }
            if self.download_jobs.is_empty() {
                ui.small("No downloads in queue");
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
        let game_version = self.detect_instance_version(&instance_path);
        let loader = self.detect_instance_loader(&instance_path);

        let managed_result = sync_modrinth_managed_mods(&instance_path);
        let installed_result =
            update_installed_modrinth_mods(&instance_path, &game_version, &loader);

        match (managed_result, installed_result) {
            (Ok(managed), Ok(installed)) => {
                let total = managed + installed;
                if total > 0 {
                    self.status =
                        format!("Updated {total} mods ({managed} managed + {installed} installed)");
                } else {
                    self.status = "All mods are up to date".to_string();
                }
                self.refresh_selected_content();
            }
            (Err(err), Ok(_)) => {
                self.status = format!("Managed mod update failed: {err}");
            }
            (Ok(_), Err(err)) => {
                self.status = format!("Installed mod update failed: {err}");
            }
            (Err(err1), Err(err2)) => {
                self.status = format!(
                    "Mod auto-update failed: managed error: {err1}; installed error: {err2}"
                );
            }
        }
    }

    fn do_add_content_file(&mut self) {
        let Some(instance_path) = self.selected_instance_path() else {
            self.status = "No instance selected".to_string();
            return;
        };
        let content_type = self.download_content_type.clone();
        if matches!(
            content_type,
            DownloadContentType::Worlds
                | DownloadContentType::Servers
                | DownloadContentType::Screenshots
        ) {
            self.status =
                "Add Content is available for Mods/Resource Packs/Shader Packs".to_string();
            return;
        }
        let mut dialog = rfd::FileDialog::new();
        dialog = match content_type {
            DownloadContentType::Mods => dialog.add_filter("Java archives", &["jar"]),
            DownloadContentType::ResourcePacks => {
                dialog.add_filter("Resource packs", &["zip", "jar"])
            }
            DownloadContentType::ShaderPacks => dialog.add_filter("Shader packs", &["zip", "jar"]),
            DownloadContentType::Worlds
            | DownloadContentType::Servers
            | DownloadContentType::Screenshots => dialog,
        };
        let Some(file_path) = dialog.pick_file() else {
            return;
        };
        let file_name = match file_path.file_name().and_then(|x| x.to_str()) {
            Some(x) if !x.trim().is_empty() => x.to_string(),
            _ => {
                self.status = "Selected file name is invalid".to_string();
                return;
            }
        };
        let target_dir = preferred_download_dir(&instance_path, &content_type);
        let target_path = target_dir.join(&file_name);
        match fs::copy(&file_path, &target_path) {
            Ok(_) => {
                self.status = format!("Added content: {} -> {}", file_name, target_path.display());
                self.refresh_selected_content();
            }
            Err(err) => {
                self.status = format!("Failed to add content file: {err}");
            }
        }
    }

    fn probe_java_runtime(path: &Path) -> Option<String> {
        let output = Command::new(path).arg("-version").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let merged = format!("{stderr}\n{stdout}");
        merged
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(ToString::to_string)
    }

    fn discover_java_candidates() -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                out.push(dir.join("java"));
                out.push(dir.join("java.exe"));
            }
        }
        out.push(PathBuf::from("/usr/bin/java"));
        out.push(PathBuf::from("/usr/local/bin/java"));
        if let Ok(home) = std::env::var("HOME") {
            out.push(
                PathBuf::from(&home)
                    .join(".sdkman")
                    .join("candidates")
                    .join("java")
                    .join("current")
                    .join("bin")
                    .join("java"),
            );
            out.push(PathBuf::from(home).join(".local").join("bin").join("java"));
        }
        out
    }

    fn do_find_java(&mut self) {
        let mut seen = HashSet::new();
        for candidate in Self::discover_java_candidates() {
            if !candidate.exists() {
                continue;
            }
            let key = candidate.display().to_string();
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(version) = Self::probe_java_runtime(&candidate) {
                self.global_settings.java_path = key;
                save_global_settings(&self.global_settings);
                self.status = format!("Java found: {version}");
                return;
            }
        }

        self.status =
            "Java not found automatically. Use 'Browse' to select java binary.".to_string();
    }

    fn do_browse_java(&mut self) {
        let mut dialog = rfd::FileDialog::new().set_title("Select Java executable");
        if let Ok(home) = std::env::var("HOME") {
            dialog = dialog.set_directory(home);
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        let version = Self::probe_java_runtime(&path);
        self.global_settings.java_path = path.display().to_string();
        save_global_settings(&self.global_settings);
        self.status = match version {
            Some(v) => format!("Java selected: {v}"),
            None => format!(
                "Selected path saved, but runtime check failed: {}",
                self.global_settings.java_path
            ),
        };
    }

    fn do_check_java_settings(&mut self) {
        let value = self.global_settings.java_path.trim();
        if value.is_empty() {
            self.status = "Java path is empty".to_string();
            return;
        }
        let path = PathBuf::from(value);
        if let Some(version) = Self::probe_java_runtime(&path) {
            self.status = format!("Java OK: {version}");
        } else {
            self.status = format!("Java check failed: {value}");
        }
    }

    fn persist(&self) {
        let state = PersistedState {
            selected: self.selected,
            show_news: self.show_news,
            filter: self.filter.clone(),
            active_tab: self.active_tab.clone(),
            last_seen_launcher_version: self.last_seen_launcher_version.clone(),
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

    fn detect_instance_loader_version(&self, instance_path: &Path, loader: &str) -> Option<String> {
        if loader.trim().is_empty() {
            return None;
        }
        let target_uid = match loader {
            "fabric" => "net.fabricmc.fabric-loader",
            "quilt" => "org.quiltmc.quilt-loader",
            "forge" => "net.minecraftforge",
            "neoforge" => "net.neoforged",
            _ => return None,
        };
        let mmc_pack = instance_path.join("mmc-pack.json");
        let text = fs::read_to_string(mmc_pack).ok()?;
        let json = serde_json::from_str::<serde_json::Value>(&text).ok()?;
        let components = json.get("components")?.as_array()?;
        for component in components {
            let uid = component.get("uid").and_then(|v| v.as_str()).unwrap_or("");
            if uid == target_uid {
                let version = component
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let v = version.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
        None
    }

    fn resolve_instance_icon_path(&self, instance_path: &Path) -> Option<String> {
        let cfg = load_prism_instance_config(instance_path).unwrap_or_default();
        let is_custom_icon = cfg
            .icon_key
            .as_ref()
            .map(|v| {
                let trimmed = v.trim();
                !trimmed.is_empty() && trimmed != "default"
            })
            .unwrap_or(false);

        if is_custom_icon && let Some(icon_key) = cfg.icon_key {
            let trimmed = icon_key.trim();
            for ext in ["png", "jpg", "jpeg", "ico", "webp", "gif", "svg"] {
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

        if !is_custom_icon {
            let mut root_images: Vec<PathBuf> = match fs::read_dir(instance_path) {
                Ok(entries) => entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.is_file())
                    .filter(|p| {
                        p.extension()
                            .and_then(|e| e.to_str())
                            .map(|e| {
                                matches!(
                                    e.to_ascii_lowercase().as_str(),
                                    "png" | "jpg" | "jpeg" | "webp" | "gif" | "svg"
                                )
                            })
                            .unwrap_or(false)
                    })
                    .collect(),
                Err(_) => Vec::new(),
            };
            root_images.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
            if let Some(first) = root_images.first() {
                return Some(first.display().to_string());
            }
        }

        for candidate in [
            instance_path.join("icon.png"),
            instance_path.join("icon.webp"),
            instance_path.join("icon.gif"),
            instance_path.join("icon.svg"),
            instance_path.join("icon.jpg"),
            instance_path.join("icon.jpeg"),
            instance_path.join(".minecraft").join("icon.png"),
            instance_path.join(".minecraft").join("icon.webp"),
            instance_path.join(".minecraft").join("icon.gif"),
            instance_path.join(".minecraft").join("icon.svg"),
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
        self.extract_mod_icon_from_archive(&mod_file)
    }

    fn extract_mod_icon_from_archive(&self, mod_file: &Path) -> Option<String> {
        let file = fs::File::open(mod_file).ok()?;
        let mut zip = ZipArchive::new(file).ok()?;

        let mut icon_candidates: Vec<String> = Vec::new();

        if let Ok(mut fabric) = zip.by_name("fabric.mod.json") {
            let mut text = String::new();
            if fabric.read_to_string(&mut text).is_ok()
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
                && let Some(icon) = json.get("icon")
            {
                if let Some(path) = icon.as_str() {
                    icon_candidates.push(path.to_string());
                } else if let Some(map) = icon.as_object() {
                    let mut best_size = 0u32;
                    let mut best_path = None::<String>;
                    for (k, v) in map {
                        if let Some(path) = v.as_str() {
                            let size = k.parse::<u32>().unwrap_or(0);
                            if size >= best_size {
                                best_size = size;
                                best_path = Some(path.to_string());
                            }
                        }
                    }
                    if let Some(path) = best_path {
                        icon_candidates.push(path);
                    }
                }
            }
        }

        if let Ok(mut quilt) = zip.by_name("quilt.mod.json") {
            let mut text = String::new();
            if quilt.read_to_string(&mut text).is_ok()
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
            {
                if let Some(path) = json
                    .get("quilt_loader")
                    .and_then(|v| v.get("icon"))
                    .and_then(|v| v.as_str())
                {
                    icon_candidates.push(path.to_string());
                }
                if let Some(path) = json
                    .get("metadata")
                    .and_then(|v| v.get("icon"))
                    .and_then(|v| v.as_str())
                {
                    icon_candidates.push(path.to_string());
                }
            }
        }

        if let Ok(mut mods_toml) = zip.by_name("META-INF/mods.toml") {
            let mut text = String::new();
            if mods_toml.read_to_string(&mut text).is_ok() {
                for line in text.lines() {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("logoFile")
                        && let Some((_, value)) = rest.split_once('=')
                    {
                        let path = value.trim().trim_matches('"').trim_matches('\'');
                        if !path.is_empty() {
                            icon_candidates.push(path.to_string());
                        }
                    }
                }
            }
        }
        if let Some(path) = read_mcmod_info_logo_path(&mut zip) {
            icon_candidates.push(path);
        }

        if icon_candidates.is_empty() {
            for fallback in ["icon.png", "assets/icon.png", "logo.png"] {
                if resolve_zip_entry_name(&mut zip, fallback).is_some() {
                    icon_candidates.push(fallback.to_string());
                    break;
                }
            }
            if icon_candidates.is_empty() {
                for i in 0..zip.len() {
                    let Ok(entry) = zip.by_index(i) else {
                        continue;
                    };
                    let entry_name = entry.name().to_string();
                    let lower = entry_name.to_ascii_lowercase();
                    if !(lower.ends_with(".png")
                        || lower.ends_with(".jpg")
                        || lower.ends_with(".jpeg")
                        || lower.ends_with(".webp")
                        || lower.ends_with(".gif"))
                    {
                        continue;
                    }
                    if lower.contains("icon") || lower.contains("logo") {
                        icon_candidates.push(entry_name);
                        if icon_candidates.len() >= 4 {
                            break;
                        }
                    }
                }
            }
        }

        for icon_path in icon_candidates {
            let (resolved_icon_path, bytes) = match read_zip_entry_bytes_fuzzy(&mut zip, &icon_path)
            {
                Some(v) => v,
                None => continue,
            };
            if image::load_from_memory(&bytes).is_err() {
                continue;
            }
            let ext = Path::new(&resolved_icon_path)
                .extension()
                .and_then(|x| x.to_str())
                .filter(|x| !x.trim().is_empty())
                .unwrap_or("png");
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            mod_file.display().to_string().hash(&mut hasher);
            resolved_icon_path.hash(&mut hasher);
            if let Ok(meta) = fs::metadata(mod_file)
                && let Ok(modified) = meta.modified()
                && let Ok(delta) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                delta.as_nanos().hash(&mut hasher);
            }
            let name = format!("{:016x}.{}", hasher.finish(), ext);
            let cache_dir = self.data_root.join("cache").join("mod_icons");
            let _ = fs::create_dir_all(&cache_dir);
            let target = cache_dir.join(name);
            if !target.is_file() {
                let _ = fs::write(&target, &bytes);
            }
            if target.is_file() {
                return Some(target.display().to_string());
            }
        }

        None
    }

    fn extract_mod_metadata_from_archive(&self, mod_file: &Path) -> Option<(String, String)> {
        let file = fs::File::open(mod_file).ok()?;
        let mut zip = ZipArchive::new(file).ok()?;

        let manifest_version = read_manifest_implementation_version(&mut zip);

        if let Some((name, mut version)) =
            read_mods_toml_name_version(&mut zip, "META-INF/mods.toml")
                .or_else(|| read_mods_toml_name_version(&mut zip, "META-INF/neoforge.mods.toml"))
        {
            if version == "${file.jarVersion}" {
                version = manifest_version.unwrap_or_else(|| "NONE".to_string());
            }
            return Some((name, version));
        }

        if let Some((name, version)) = read_mcmod_info_name_version(&mut zip) {
            return Some((name, version));
        }
        if let Some((name, version)) = read_quilt_mod_name_version(&mut zip) {
            return Some((name, version));
        }
        if let Some((name, version)) = read_fabric_mod_name_version(&mut zip) {
            return Some((name, version));
        }
        if let Some((name, version)) = read_forgeversion_properties_name_version(&mut zip) {
            return Some((name, version));
        }
        if let Some((name, version)) = read_litemod_name_version(&mut zip) {
            return Some((name, version));
        }
        None
    }

    fn resolve_content_icon_path(&self, content_file: &Path) -> Option<String> {
        if !content_file.is_file() {
            return None;
        }
        let stem = content_file.file_stem()?.to_str()?.to_string();
        let file_name = content_file.file_name()?.to_str()?.to_string();
        let parent = content_file.parent()?;
        for candidate in [
            parent.join(format!("{stem}.png")),
            parent.join(format!("{stem}.webp")),
            parent.join(format!("{stem}.gif")),
            parent.join(format!("{stem}.jpg")),
            parent.join(format!("{stem}.jpeg")),
            parent.join(format!("{file_name}.png")),
            parent.join(format!("{file_name}.webp")),
            parent.join(format!("{file_name}.gif")),
            parent.join(format!("{file_name}.jpg")),
            parent.join(format!("{file_name}.jpeg")),
        ] {
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
        self.extract_content_icon_from_archive(content_file)
    }

    fn extract_content_icon_from_archive(&self, content_file: &Path) -> Option<String> {
        let ext = content_file
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let archive_ext = if ext == "disabled" {
            content_file
                .file_stem()
                .and_then(|x| Path::new(x).extension())
                .and_then(|x| x.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase()
        } else {
            ext.clone()
        };
        if archive_ext != "zip" && archive_ext != "jar" {
            return None;
        }
        let file = fs::File::open(content_file).ok()?;
        let mut zip = ZipArchive::new(file).ok()?;
        let mut icon_candidates = vec![
            "pack.png".to_string(),
            "icon.png".to_string(),
            "preview.png".to_string(),
            "pack.jpg".to_string(),
            "pack.jpeg".to_string(),
            "pack.webp".to_string(),
            "shaders/icon.png".to_string(),
            "shaders/pack.png".to_string(),
            "shaders/preview.png".to_string(),
            "shaderpacks/icon.png".to_string(),
            "shaderpacks/pack.png".to_string(),
        ];
        if let Ok(mut mcmeta) = zip.by_name("pack.mcmeta") {
            let mut text = String::new();
            if mcmeta.read_to_string(&mut text).is_ok()
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
                && let Some(icon) = json
                    .get("pack")
                    .and_then(|v| v.get("icon"))
                    .and_then(|v| v.as_str())
            {
                icon_candidates.insert(0, icon.to_string());
            }
        }
        for icon_path in icon_candidates {
            let (resolved_icon_path, bytes) = match read_zip_entry_bytes_fuzzy(&mut zip, &icon_path)
            {
                Some(v) => v,
                None => continue,
            };
            if image::load_from_memory(&bytes).is_err() {
                continue;
            }
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            content_file.display().to_string().hash(&mut hasher);
            resolved_icon_path.hash(&mut hasher);
            if let Ok(meta) = fs::metadata(content_file)
                && let Ok(modified) = meta.modified()
                && let Ok(delta) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                delta.as_nanos().hash(&mut hasher);
            }
            let cache_dir = self.data_root.join("cache").join("content_icons");
            let _ = fs::create_dir_all(&cache_dir);
            let target = cache_dir.join(format!("{:016x}.png", hasher.finish()));
            if !target.is_file() {
                let _ = fs::write(&target, &bytes);
            }
            if target.is_file() {
                return Some(target.display().to_string());
            }
        }
        None
    }

    fn extract_content_metadata_from_archive(
        &self,
        content_file: &Path,
        content_type: DownloadContentType,
    ) -> Option<(String, String)> {
        let ext = content_file
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let archive_ext = if ext == "disabled" {
            content_file
                .file_stem()
                .and_then(|x| Path::new(x).extension())
                .and_then(|x| x.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase()
        } else {
            ext
        };
        if archive_ext != "zip" && archive_ext != "jar" {
            return None;
        }
        let file = fs::File::open(content_file).ok()?;
        let mut zip = ZipArchive::new(file).ok()?;

        if let Ok(mut pack) = zip.by_name("pack.mcmeta") {
            let mut text = String::new();
            if pack.read_to_string(&mut text).is_ok()
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
                && let Some(pack_obj) = json.get("pack")
            {
                let name = pack_obj
                    .get("description")
                    .map(json_text_compact)
                    .filter(|x| !x.trim().is_empty())
                    .unwrap_or_else(|| {
                        prettify_mod_name(
                            content_file
                                .file_stem()
                                .and_then(|x| x.to_str())
                                .unwrap_or("content"),
                        )
                    });
                let version = pack_obj
                    .get("pack_format")
                    .and_then(|v| v.as_i64())
                    .map(|x| format!("pack format {x}"))
                    .unwrap_or_else(|| "unknown".to_string());
                return Some((name, version));
            }
        }

        if content_type == DownloadContentType::ShaderPacks
            && let Ok(mut props) = zip.by_name("shaders/shaders.properties")
        {
            let mut text = String::new();
            if props.read_to_string(&mut text).is_ok() {
                for line in text.lines() {
                    let line = line.trim();
                    if line.starts_with('#') || line.is_empty() {
                        continue;
                    }
                    if let Some((key, value)) = line.split_once('=')
                        && key.trim().eq_ignore_ascii_case("version")
                    {
                        let v = value.trim();
                        if !v.is_empty() {
                            let name = prettify_mod_name(
                                content_file
                                    .file_stem()
                                    .and_then(|x| x.to_str())
                                    .unwrap_or("shader pack"),
                            );
                            return Some((name, v.to_string()));
                        }
                    }
                }
            }
        }
        None
    }

    fn build_content_cache(
        &self,
        dir: &Path,
        content_type: DownloadContentType,
    ) -> Vec<ContentListEntry> {
        let mut by_clean_name: HashMap<String, PathBuf> = HashMap::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|x| x.to_str()) {
                Some(v) => v.to_string(),
                None => continue,
            };
            let clean = name.strip_suffix(".disabled").unwrap_or(&name).to_string();
            match by_clean_name.get(&clean) {
                Some(existing) => {
                    let existing_enabled = !existing
                        .file_name()
                        .and_then(|x| x.to_str())
                        .unwrap_or_default()
                        .ends_with(".disabled");
                    let current_enabled = !name.ends_with(".disabled");
                    if current_enabled && !existing_enabled {
                        by_clean_name.insert(clean, path);
                    }
                }
                None => {
                    by_clean_name.insert(clean, path);
                }
            }
        }

        let mut out = Vec::new();
        for (clean_name, path) in by_clean_name {
            let file_name = path
                .file_name()
                .and_then(|x| x.to_str())
                .unwrap_or_default()
                .to_string();
            let enabled = !file_name.ends_with(".disabled");
            let parsed_meta =
                self.extract_content_metadata_from_archive(&path, content_type.clone());
            let display_name = parsed_meta
                .as_ref()
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| prettify_mod_name(&clean_name));
            let version = parsed_meta
                .as_ref()
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty() && v != "unknown")
                .unwrap_or_else(|| extract_version_from_mod_filename(&clean_name));
            let updated_at = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(format_system_time_ddmmyyyy)
                .unwrap_or_default();
            out.push(ContentListEntry {
                name: clean_name,
                file_path: path.display().to_string(),
                enabled,
                display_name,
                version,
                updated_at,
                provider: "Local".to_string(),
                icon_path: self.resolve_content_icon_path(&path),
            });
        }
        out.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
        });
        out
    }

    fn build_worlds_cache(&self, dir: &Path) -> Vec<ContentListEntry> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = match path.file_name().and_then(|x| x.to_str()) {
                Some(v) => v.to_string(),
                None => continue,
            };
            let updated_at = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(format_system_time_ddmmyyyy)
                .unwrap_or_default();
            out.push(ContentListEntry {
                name: name.clone(),
                file_path: path.display().to_string(),
                enabled: true,
                display_name: name,
                version: String::new(),
                updated_at,
                provider: "Local".to_string(),
                icon_path: Some(
                    "https://minecraft.wiki/images/Grass_Block_JE7_BE6.png?2bd37?download"
                        .to_string(),
                ),
            });
        }
        out.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
        });
        out
    }

    fn build_screenshots_cache(&self, dir: &Path) -> Vec<ContentListEntry> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|x| x.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif") {
                continue;
            }
            let name = match path.file_name().and_then(|x| x.to_str()) {
                Some(v) => v.to_string(),
                None => continue,
            };
            let updated_at = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(format_system_time_ddmmyyyy)
                .unwrap_or_default();
            out.push(ContentListEntry {
                name: name.clone(),
                file_path: path.display().to_string(),
                enabled: true,
                display_name: name,
                version: String::new(),
                updated_at,
                provider: "Local".to_string(),
                icon_path: Some(path.display().to_string()),
            });
        }
        out.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
        });
        out
    }

    fn build_servers_cache(&self, instance_path: &Path) -> Vec<ContentListEntry> {
        let mut out = Vec::new();
        for candidate in [
            instance_path.join("minecraft").join("servers.dat"),
            instance_path.join(".minecraft").join("servers.dat"),
            instance_path.join("servers.dat"),
        ] {
            if !candidate.is_file() {
                continue;
            }
            let entries = parse_servers_dat_loose(&candidate);
            if entries.is_empty() {
                out.push(ContentListEntry {
                    name: "servers.dat".to_string(),
                    file_path: candidate.display().to_string(),
                    enabled: true,
                    display_name: "Minecraft Server".to_string(),
                    version: "unknown".to_string(),
                    updated_at: "...".to_string(),
                    provider: "Local".to_string(),
                    icon_path: Some(
                        "https://minecraft.wiki/images/Repeating_Command_Block.gif?7ab3a?download"
                            .to_string(),
                    ),
                });
            } else {
                for (name, addr) in entries {
                    out.push(ContentListEntry {
                        name: name.clone(),
                        file_path: candidate.display().to_string(),
                        enabled: true,
                        display_name: name,
                        version: addr,
                        updated_at: "...".to_string(),
                        provider: "Local".to_string(),
                        icon_path: Some(
                            "https://minecraft.wiki/images/Repeating_Command_Block.gif?7ab3a?download"
                                .to_string(),
                        ),
                    });
                }
            }
            break;
        }
        out
    }

    fn copy_image_file_to_clipboard(&mut self, image_path: &str) {
        let path = PathBuf::from(image_path);
        let Ok(bytes) = fs::read(&path) else {
            self.status = format!("Failed to read screenshot: {}", path.display());
            return;
        };
        let Ok(image) = image::load_from_memory(&bytes) else {
            self.status = format!("Failed to decode image: {}", path.display());
            return;
        };
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        let image_data = arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: std::borrow::Cow::Owned(rgba.into_raw()),
        };
        match Clipboard::new().and_then(|mut cb| cb.set_image(image_data)) {
            Ok(_) => {
                self.status = format!("Copied screenshot to clipboard: {}", path.display());
            }
            Err(err) => {
                self.status = format!("Clipboard copy failed: {err}");
            }
        }
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
        let clean_name = name.strip_suffix(".disabled").unwrap_or(name).to_string();
        let selected_instance_path = self.selected_instance_path();
        let mod_dirs = selected_instance_path
            .as_ref()
            .map(|p| all_existing_mod_dirs(p))
            .unwrap_or_default();

        let mut changed = 0usize;
        let mut errors = Vec::new();

        // Apply toggle to every matching copy across known mod directories.
        // This avoids "duplicate" entries where one copy is enabled and another is disabled.
        for dir in &mod_dirs {
            let enabled_path = dir.join(&clean_name);
            let disabled_path = dir.join(format!("{clean_name}.disabled"));
            if enable {
                if disabled_path.is_file() {
                    if enabled_path.is_file() {
                        if let Err(err) = fs::remove_file(&disabled_path) {
                            errors.push(format!("{}: {err}", disabled_path.display()));
                        } else {
                            changed += 1;
                        }
                    } else if let Err(err) = fs::rename(&disabled_path, &enabled_path) {
                        errors.push(format!(
                            "{} -> {}: {err}",
                            disabled_path.display(),
                            enabled_path.display()
                        ));
                    } else {
                        changed += 1;
                    }
                }
            } else if enabled_path.is_file() {
                if disabled_path.is_file() {
                    if let Err(err) = fs::remove_file(&enabled_path) {
                        errors.push(format!("{}: {err}", enabled_path.display()));
                    } else {
                        changed += 1;
                    }
                } else if let Err(err) = fs::rename(&enabled_path, &disabled_path) {
                    errors.push(format!(
                        "{} -> {}: {err}",
                        enabled_path.display(),
                        disabled_path.display()
                    ));
                } else {
                    changed += 1;
                }
            }
        }

        // Fallback: if the selected instance is unknown or nothing matched, toggle direct file.
        if changed == 0 {
            let to_name = if enable {
                name.strip_suffix(".disabled").unwrap_or(name).to_string()
            } else if name.ends_with(".disabled") {
                name.to_string()
            } else {
                format!("{name}.disabled")
            };
            if to_name != name {
                if let Some(parent) = from.parent() {
                    let to = parent.join(to_name);
                    match fs::rename(&from, &to) {
                        Ok(_) => changed += 1,
                        Err(err) => {
                            errors.push(format!("{} -> {}: {err}", from.display(), to.display()))
                        }
                    }
                } else {
                    errors.push("Invalid mod file path".to_string());
                }
            }
        }

        if !errors.is_empty() {
            self.status = format!("Failed to update mod state: {}", errors.join(" | "));
        } else if changed > 0 {
            self.status = format!("Updated mod state for {clean_name}");
            self.refresh_selected_content();
        } else {
            self.status = format!("No matching files found for {clean_name}");
        }
    }

    fn do_toggle_content_enabled(&mut self, file_path: &str, enable: bool) {
        let from = PathBuf::from(file_path);
        let Some(name) = from.file_name().and_then(|x| x.to_str()) else {
            self.status = "Invalid content file name".to_string();
            return;
        };
        let clean_name = name.strip_suffix(".disabled").unwrap_or(name).to_string();
        let Some(parent) = from.parent() else {
            self.status = "Invalid content file path".to_string();
            return;
        };
        let enabled_path = parent.join(&clean_name);
        let disabled_path = parent.join(format!("{clean_name}.disabled"));

        let result = if enable {
            if disabled_path.is_file() {
                if enabled_path.is_file() {
                    fs::remove_file(&disabled_path)
                } else {
                    fs::rename(&disabled_path, &enabled_path)
                }
            } else {
                Ok(())
            }
        } else if enabled_path.is_file() {
            if disabled_path.is_file() {
                fs::remove_file(&enabled_path)
            } else {
                fs::rename(&enabled_path, &disabled_path)
            }
        } else {
            Ok(())
        };

        match result {
            Ok(_) => {
                self.status = format!("Updated content state for {clean_name}");
                self.refresh_selected_content();
            }
            Err(err) => {
                self.status = format!("Failed to update content state: {err}");
            }
        }
    }

    fn do_delete_mod_file(&mut self, file_path: &str) {
        let path = PathBuf::from(file_path);
        let result = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        match result {
            Ok(_) => {
                self.status = format!("Deleted mod: {}", path.display());
                self.refresh_selected_content();
            }
            Err(err) => {
                self.status = format!("Failed to delete mod {}: {err}", path.display());
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
                        let cfg = load_prism_instance_config(&item.path).unwrap_or_default();
                        let version = self.detect_instance_version(&item.path);
                        let loader = {
                            let detected = self.detect_instance_loader(&item.path);
                            if detected.trim().is_empty() {
                                "vanilla".to_string()
                            } else {
                                detected
                            }
                        };
                        let icon_path = self.resolve_instance_icon_path(&item.path);
                        Instance {
                            name: item.name,
                            version,
                            loader,
                            running: false,
                            path: item.path.display().to_string(),
                            icon_path,
                            group: cfg.group.unwrap_or_default(),
                        }
                    })
                    .collect();
                self.sync_groups_with_instances();
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
        self.resourcepacks_cache.clear();
        self.shaderpacks_cache.clear();
        self.worlds_cache.clear();
        self.servers_cache.clear();
        self.screenshots_cache.clear();
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
                let mut by_clean_name: HashMap<String, String> = HashMap::new();
                for name in mods {
                    let clean = name.strip_suffix(".disabled").unwrap_or(&name).to_string();
                    match by_clean_name.get(&clean) {
                        Some(existing) => {
                            let existing_enabled = !existing.ends_with(".disabled");
                            let current_enabled = !name.ends_with(".disabled");
                            if current_enabled && !existing_enabled {
                                by_clean_name.insert(clean, name);
                            }
                        }
                        None => {
                            by_clean_name.insert(clean, name);
                        }
                    }
                }
                let mut merged_mods: Vec<String> = by_clean_name.into_values().collect();
                merged_mods.sort();
                self.mods_cache = merged_mods
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
                        let parsed_meta = resolve_mod_file_path(&path, &name)
                            .and_then(|p| self.extract_mod_metadata_from_archive(&p));
                        let (display_name, icon_path, provider) =
                            if let Some((title, icon)) = managed.get(&clean_name) {
                                (
                                    parsed_meta
                                        .as_ref()
                                        .map(|(n, _)| n.clone())
                                        .or_else(|| {
                                            if title.trim().is_empty() {
                                                None
                                            } else {
                                                Some(title.clone())
                                            }
                                        })
                                        .unwrap_or_else(|| prettify_mod_name(&clean_name)),
                                    icon.clone()
                                        .or_else(|| self.resolve_mod_icon_path(&path, &name)),
                                    "Modrinth".to_string(),
                                )
                            } else {
                                (
                                    parsed_meta
                                        .as_ref()
                                        .map(|(n, _)| n.clone())
                                        .unwrap_or_else(|| prettify_mod_name(&clean_name)),
                                    self.resolve_mod_icon_path(&path, &name),
                                    "Local".to_string(),
                                )
                            };
                        let version = parsed_meta
                            .as_ref()
                            .map(|(_, v)| v.trim().to_string())
                            .filter(|v| !v.is_empty())
                            .unwrap_or_else(|| extract_version_from_mod_filename(&clean_name));
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

        let resourcepacks_dir = preferred_resourcepacks_dir(&path);
        self.resourcepacks_cache =
            self.build_content_cache(&resourcepacks_dir, DownloadContentType::ResourcePacks);
        let shaderpacks_dir = preferred_shaderpacks_dir(&path);
        self.shaderpacks_cache =
            self.build_content_cache(&shaderpacks_dir, DownloadContentType::ShaderPacks);
        let worlds_dir = preferred_worlds_dir(&path);
        self.worlds_cache = self.build_worlds_cache(&worlds_dir);
        let screenshots_dir = preferred_screenshots_dir(&path);
        self.screenshots_cache = self.build_screenshots_cache(&screenshots_dir);
        self.servers_cache = self.build_servers_cache(&path);

        match list_logs(&path) {
            Ok(logs) => {
                self.logs_cache = logs
                    .into_iter()
                    .map(|x| (x.file_name, x.path.display().to_string()))
                    .collect();
                if !self.logs_cache.is_empty() {
                    let preferred = self
                        .logs_cache
                        .iter()
                        .position(|(name, _)| name == "launcher.log")
                        .or_else(|| {
                            self.logs_cache
                                .iter()
                                .position(|(name, _)| name == "latest.log")
                        })
                        .unwrap_or(0);
                    self.selected_log = Some(preferred);
                    self.open_selected_log_preview();
                }
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
        let stale_external: Vec<String> = self
            .running_process_pids
            .iter()
            .filter_map(|(path, pid)| {
                if self.processes.contains_key(path) || Self::is_pid_alive_for_instance(path, *pid)
                {
                    None
                } else {
                    Some(path.clone())
                }
            })
            .collect();
        for path in stale_external {
            self.running_process_pids.remove(&path);
        }

        let keys: Vec<String> = self.processes.keys().cloned().collect();
        let mut finished: Vec<(String, Option<i32>)> = Vec::new();
        for key in keys {
            if let Some(child) = self.processes.get_mut(&key) {
                match child.try_wait() {
                    Ok(Some(status)) => finished.push((key, status.code())),
                    Ok(None) => {}
                    Err(_) => finished.push((key, None)),
                }
            }
        }
        for (key, exit_code) in finished {
            self.processes.remove(&key);
            self.running_process_pids.remove(&key);
            self.last_stopped_instance_path = Some(key.clone());
            if let Some(post) = self.post_exit_commands.remove(&key) {
                let _ = Command::new("sh")
                    .arg("-lc")
                    .arg(post)
                    .current_dir(&key)
                    .status();
            }
            let name = self
                .instances
                .iter()
                .find(|i| i.path == key)
                .map(|i| i.name.clone())
                .unwrap_or_else(|| key.clone());
            match exit_code {
                Some(0) => {
                    self.status = format!("{name} stopped");
                    self.append_launcher_log(&key, "[Launcher] Process exited with code 0");
                }
                Some(code) => {
                    self.status = format!("{name} crashed (exit code {code})");
                    self.append_launcher_log(
                        &key,
                        &format!("[Launcher] Process crashed with code {code}"),
                    );
                    self.make_launch_toast_for_path(
                        &key,
                        "Minecraft failed to launch",
                        name.clone(),
                        true,
                    );
                }
                None => {
                    self.status = format!("{name} stopped");
                    self.append_launcher_log(&key, "[Launcher] Process exited");
                }
            }
        }
        self.persist_running_processes();
        for instance in &mut self.instances {
            instance.running = self.processes.contains_key(&instance.path)
                || self.running_process_pids.contains_key(&instance.path);
        }
    }

    fn do_create_instance(&mut self) {
        let name = self.create_name.trim().to_string();
        let group = self.create_group.trim().to_string();
        let game_version = self.create_game_version.trim().to_string();
        let loader = self.create_loader.clone();
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
                let managed_loader = loader
                    .as_ref()
                    .map(CreateLoader::cfg_value)
                    .unwrap_or("vanilla");
                let mut cfg_text = format!(
                    "# PrismarineLauncher instance\nIntendedVersion={}\nManagedLoader={}\n",
                    game_version, managed_loader
                );
                if !group.is_empty() {
                    cfg_text.push_str(&format!("Group={group}\n"));
                }
                if let Err(err) = fs::write(created.path.join("instance.cfg"), cfg_text) {
                    self.status =
                        format!("Created instance {name}, but failed to write cfg: {err}");
                    return;
                }

                let mut components = vec![serde_json::json!({
                    "uid": "net.minecraft",
                    "version": game_version
                })];
                if let Some(loader) = &loader {
                    components.push(serde_json::json!({
                        "uid": loader.mmc_uid(),
                        "version": "0.0.0"
                    }));
                }
                let mmc_pack = serde_json::json!({
                    "formatVersion": 1,
                    "components": components
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
                    loader
                        .as_ref()
                        .map(CreateLoader::label)
                        .unwrap_or("Vanilla")
                );
                self.show_create_dialog = false;
                self.create_name.clear();
                self.create_group.clear();
                self.create_game_version.clear();
                self.create_loader = None;
                self.reload_instances();
            }
            Err(err) => {
                self.status = format!("Failed to create instance: {err}");
            }
        }
    }

    fn do_browse_import_archive(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Pack archives", &["mrpack", "zip"])
            .pick_file()
        else {
            return;
        };
        self.create_import_path = path.display().to_string();
        if self.create_name.trim().is_empty() {
            self.create_name = guess_instance_name_from_archive_path(&path);
        }
    }

    fn do_import_instance(&mut self) {
        if self.import_worker_rx.is_some() {
            self.status = "Import already in progress".to_string();
            return;
        }

        let archive_path = PathBuf::from(self.create_import_path.trim());
        let group = self.create_group.trim().to_string();
        if self.create_name.trim().is_empty() {
            self.status = "Instance name must not be empty".to_string();
            return;
        }
        if self.create_import_path.trim().is_empty() || !archive_path.is_file() {
            self.status = "Choose valid .mrpack or .zip file".to_string();
            return;
        }

        let instance_name = self.create_name.trim().to_string();
        let lower = archive_path
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or_default()
            .to_lowercase();

        match create_instance(&self.instance_root(), &instance_name) {
            Ok(created) => {
                let (tx, rx) = mpsc::channel::<ImportWorkerEvent>();
                self.import_worker_rx = Some(rx);
                self.import_progress = Some(ImportUiProgress {
                    stage: "import".to_string(),
                    done: 0,
                    total: 1,
                    message: "Preparing import...".to_string(),
                });
                self.status = format!("Importing instance {}...", instance_name);

                let instance_path = created.path.clone();
                let instance_name_for_thread = instance_name.clone();
                let group_for_thread = group.clone();
                std::thread::spawn(move || {
                    let send_progress = |done: usize, total: usize, message: String| {
                        let _ = tx.send(ImportWorkerEvent::Progress {
                            stage: "import".to_string(),
                            done,
                            total,
                            message,
                        });
                    };

                    let result = if lower == "mrpack" {
                        import_mrpack_archive_with_progress(
                            &archive_path,
                            &instance_path,
                            send_progress,
                        )
                    } else {
                        import_zip_archive_with_progress(
                            &archive_path,
                            &instance_path,
                            send_progress,
                        )
                    };
                    match result {
                        Ok(summary) => {
                            let _ = tx.send(ImportWorkerEvent::Finished {
                                instance_name: instance_name_for_thread,
                                instance_path,
                                group: group_for_thread,
                                summary,
                            });
                        }
                        Err(err) => {
                            let _ = delete_instance(&instance_path);
                            let _ = tx.send(ImportWorkerEvent::Error {
                                message: format!("Failed to import archive: {err}"),
                            });
                        }
                    }
                });
            }
            Err(err) => {
                self.status = format!("Failed to create instance folder: {err}");
            }
        }
    }

    fn poll_import_worker_events(&mut self) {
        let mut clear_receiver = false;
        let mut events = Vec::new();
        if let Some(rx) = &self.import_worker_rx {
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
        }

        for event in events {
            match event {
                ImportWorkerEvent::Progress {
                    stage,
                    done,
                    total,
                    message,
                } => {
                    self.import_progress = Some(ImportUiProgress {
                        stage,
                        done,
                        total,
                        message: message.clone(),
                    });
                    self.status = message;
                }
                ImportWorkerEvent::Finished {
                    instance_name,
                    instance_path,
                    group,
                    summary,
                } => {
                    if !summary.game_version.is_empty() {
                        let loader = summary
                            .loader
                            .as_ref()
                            .map_or("custom".to_string(), |x| x.cfg_value().to_string());
                        let cfg_text = format!(
                            "# PrismarineLauncher instance\nIntendedVersion={}\nManagedLoader={}\n",
                            summary.game_version, loader
                        );
                        let _ = fs::write(instance_path.join("instance.cfg"), cfg_text);
                        if !group.is_empty() {
                            let _ = set_instance_cfg_value(&instance_path, "Group", Some(&group));
                        }

                        let mut components = vec![
                            serde_json::json!({"uid":"net.minecraft","version":summary.game_version}),
                        ];
                        if let Some(loader) = summary.loader {
                            components.push(
                                serde_json::json!({"uid":loader.mmc_uid(),"version":"0.0.0"}),
                            );
                        }
                        let mmc_pack = serde_json::json!({
                            "formatVersion": 1,
                            "components": components
                        });
                        let _ = fs::write(
                            instance_path.join("mmc-pack.json"),
                            serde_json::to_string_pretty(&mmc_pack)
                                .unwrap_or_else(|_| "{}".to_string()),
                        );

                        let mut profile = default_launch_profile(&instance_path);
                        upsert_arg_pair(
                            &mut profile.game_args,
                            "--version",
                            summary.game_version.as_str(),
                        );
                        let _ = save_launch_profile(&instance_path, &profile);
                    }

                    self.status = format!("Imported instance {}", instance_name);
                    self.import_progress = None;
                    self.show_create_dialog = false;
                    self.create_name.clear();
                    self.create_group.clear();
                    self.create_game_version.clear();
                    self.create_loader = None;
                    self.create_import_path.clear();
                    self.reload_instances();
                    clear_receiver = true;
                }
                ImportWorkerEvent::Error { message } => {
                    self.import_progress = None;
                    self.status = message;
                    clear_receiver = true;
                }
            }
        }

        if clear_receiver {
            self.import_worker_rx = None;
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
                self.running_process_pids.remove(&instance.path);
                self.persist_running_processes();
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
        if self.processes.contains_key(&instance.path)
            || self.running_pid_for_instance(&instance.path).is_some()
        {
            self.status = format!("{} is already running", instance.name);
            return;
        }
        if self.launch_in_progress.contains(&instance.path) {
            self.status = format!("{} launch is already preparing", instance.name);
            return;
        }

        let instance_path = PathBuf::from(&instance.path);
        let prism_cfg = load_prism_instance_config(&instance_path).unwrap_or_default();
        let mut profile = self.launch_profile.clone();
        profile.java_path = if self.global_settings.java_path.trim().is_empty() {
            "java".to_string()
        } else {
            self.global_settings.java_path.clone()
        };
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
        let allow_permgen = java_supports_permgen(&profile.java_path);
        if self.global_settings.permgen_mb > 0 && allow_permgen {
            profile
                .jvm_args
                .push(format!("-XX:PermSize={}M", self.global_settings.permgen_mb));
        }
        if let Some(perm) = prism_cfg.perm_gen
            && allow_permgen
        {
            profile.jvm_args.retain(|x| !x.starts_with("-XX:PermSize="));
            profile.jvm_args.push(format!("-XX:PermSize={}M", perm));
        }
        if !allow_permgen {
            profile.jvm_args.retain(|x| !x.starts_with("-XX:PermSize="));
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
        upsert_jvm_system_property(&mut profile.jvm_args, "user.language", "en");
        if profile.working_dir.trim().is_empty() {
            profile.working_dir = instance.path.clone();
        }
        self.apply_account_launch_args(&mut profile.game_args);

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
        let env_vars: Vec<(String, String)> = self
            .global_settings
            .environment_vars
            .lines()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let post_exit = if prism_cfg.override_commands {
            prism_cfg
                .post_exit_command
                .clone()
                .filter(|x| !x.trim().is_empty())
        } else {
            None
        };
        let instance_version = self.detect_instance_version(&instance_path);
        let instance_loader = self.detect_instance_loader(&instance_path);
        let instance_loader_version =
            self.detect_instance_loader_version(&instance_path, &instance_loader);
        let should_prepare_runtime = instance_version != "unknown" && profile.classpath.is_empty();

        self.pending_launches.insert(
            instance.path.clone(),
            PendingLaunchContext {
                instance: instance.clone(),
                pre_commands,
                env_vars,
                post_exit,
            },
        );
        self.launch_in_progress.insert(instance.path.clone());
        self.make_launch_toast_for_instance(
            &instance,
            "Minecraft is launching...",
            instance.name.clone(),
            false,
        );
        self.status = format!("Preparing launch for {}...", instance.name);
        self.append_launcher_log(
            &instance.path,
            &format!("[Launcher] Preparing launch for {}", instance.name),
        );
        self.refresh_selected_content();

        let tx = self.launch_worker_tx.clone();
        let data_root = self.data_root.clone();
        let instance_path_copy = instance_path.clone();
        let instance_key = instance.path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(LaunchWorkerEvent::Status {
                instance_path: instance_key.clone(),
                message: "Preparing launch: checking managed mods...".to_string(),
            });
            if let Err(err) = sync_modrinth_managed_mods(&instance_path_copy) {
                let _ = tx.send(LaunchWorkerEvent::Status {
                    instance_path: instance_key.clone(),
                    message: format!("Mod auto-update warning: {err}"),
                });
            }
            if should_prepare_runtime {
                let _ = tx.send(LaunchWorkerEvent::Status {
                    instance_path: instance_key.clone(),
                    message: format!(
                        "Preparing Minecraft runtime {} (downloading files)...",
                        instance_version
                    ),
                });
                let runtime_result = if instance_loader == "fabric" {
                    ensure_fabric_runtime_with_progress(
                        &data_root,
                        &instance_path_copy,
                        &instance_version,
                        instance_loader_version.as_deref(),
                        &mut profile,
                        |progress| {
                            let _ = tx.send(LaunchWorkerEvent::Progress {
                                instance_path: instance_key.clone(),
                                progress,
                            });
                        },
                    )
                } else {
                    ensure_minecraft_runtime_with_progress(
                        &data_root,
                        &instance_path_copy,
                        &instance_version,
                        &mut profile,
                        |progress| {
                            let _ = tx.send(LaunchWorkerEvent::Progress {
                                instance_path: instance_key.clone(),
                                progress,
                            });
                        },
                    )
                };
                if let Err(err) = runtime_result {
                    let _ = tx.send(LaunchWorkerEvent::Error {
                        instance_path: instance_key,
                        message: format!("Failed to prepare Minecraft runtime: {err}"),
                    });
                    return;
                }
            }
            let _ = tx.send(LaunchWorkerEvent::Ready {
                instance_path: instance_key,
                profile,
            });
        });
    }

    fn poll_launch_worker_events(&mut self) {
        loop {
            let Ok(event) = self.launch_worker_rx.try_recv() else {
                break;
            };
            match event {
                LaunchWorkerEvent::Status {
                    instance_path,
                    message,
                } => {
                    self.append_launcher_log(&instance_path, &format!("[Launcher] {message}"));
                    self.status = message;
                    if self.active_tab == CenterTab::Logs {
                        self.refresh_selected_content();
                    }
                }
                LaunchWorkerEvent::Error {
                    instance_path,
                    message,
                } => {
                    self.launch_in_progress.remove(&instance_path);
                    self.pending_launches.remove(&instance_path);
                    self.launch_progress.remove(&instance_path);
                    self.append_launcher_log(
                        &instance_path,
                        &format!("[Launcher] ERROR: {message}"),
                    );
                    self.status = message;
                    self.make_launch_toast_for_path(
                        &instance_path,
                        "Minecraft failed to launch",
                        "See logs for details".to_string(),
                        true,
                    );
                    self.refresh_selected_content();
                }
                LaunchWorkerEvent::Progress {
                    instance_path,
                    progress,
                } => {
                    self.launch_progress
                        .insert(instance_path.clone(), progress.clone());
                    self.status = progress.message.clone();
                    if self.active_tab == CenterTab::Logs {
                        self.refresh_selected_content();
                    }
                }
                LaunchWorkerEvent::Ready {
                    instance_path,
                    profile,
                } => {
                    self.launch_in_progress.remove(&instance_path);
                    self.launch_progress.remove(&instance_path);
                    if let Some(ctx) = self.pending_launches.remove(&instance_path) {
                        self.spawn_prepared_instance(ctx, profile);
                    }
                }
            }
        }
    }

    fn spawn_prepared_instance(&mut self, ctx: PendingLaunchContext, profile: LaunchProfile) {
        let instance_path = PathBuf::from(&ctx.instance.path);
        let _ = save_launch_profile(&instance_path, &profile);
        self.launch_profile = profile.clone();
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
                self.make_launch_toast_for_instance(
                    &ctx.instance,
                    "Minecraft failed to launch",
                    ctx.instance.name.clone(),
                    true,
                );
                return;
            }
        };
        let _ = writeln!(
            log_file,
            "[Launcher] Starting instance {}",
            ctx.instance.name
        );
        let _ = writeln!(log_file, "[Launcher] Command: {} {}", exe, args.join(" "));
        let stdout_log = match log_file.try_clone() {
            Ok(f) => f,
            Err(err) => {
                self.status = format!("Failed to clone log handle: {err}");
                self.make_launch_toast_for_instance(
                    &ctx.instance,
                    "Minecraft failed to launch",
                    ctx.instance.name.clone(),
                    true,
                );
                return;
            }
        };
        let stderr_log = match log_file.try_clone() {
            Ok(f) => f,
            Err(err) => {
                self.status = format!("Failed to clone log handle: {err}");
                self.make_launch_toast_for_instance(
                    &ctx.instance,
                    "Minecraft failed to launch",
                    ctx.instance.name.clone(),
                    true,
                );
                return;
            }
        };

        let working_dir = PathBuf::from(&profile.working_dir);
        for cmd_line in ctx.pre_commands {
            let _ = writeln!(log_file, "[Launcher] Pre-Launch: {}", cmd_line);
            let _ = Command::new("sh")
                .arg("-lc")
                .arg(cmd_line)
                .current_dir(&working_dir)
                .status();
        }

        let mut cmd = Command::new(exe);
        cmd.args(args);
        cmd.current_dir(working_dir)
            .stdout(Stdio::from(stdout_log))
            .stderr(Stdio::from(stderr_log));
        for (k, v) in ctx.env_vars {
            cmd.env(k, v);
        }

        match cmd.spawn() {
            Ok(mut child) => {
                if let Ok(Some(status)) = child.try_wait() {
                    let code = status.code().unwrap_or(-1);
                    self.status =
                        format!("{} failed to start (exit code {code})", ctx.instance.name);
                    self.append_launcher_log(
                        &ctx.instance.path,
                        &format!("[Launcher] Process exited immediately with code {code}"),
                    );
                    self.make_launch_toast_for_instance(
                        &ctx.instance,
                        "Minecraft failed to launch",
                        ctx.instance.name.clone(),
                        true,
                    );
                    self.refresh_selected_content();
                    return;
                }
                let pid = child.id();
                self.processes.insert(ctx.instance.path.clone(), child);
                self.running_process_pids
                    .insert(ctx.instance.path.clone(), pid);
                self.persist_running_processes();
                self.last_stopped_instance_path = None;
                if let Some(post) = ctx.post_exit {
                    self.post_exit_commands
                        .insert(ctx.instance.path.clone(), post);
                } else {
                    self.post_exit_commands.remove(&ctx.instance.path);
                }
                self.status = format!("{} is running", ctx.instance.name);
                self.sync_process_states();
                self.refresh_selected_content();
            }
            Err(err) => {
                self.status = format!("Failed to launch {}: {}", ctx.instance.name, err);
                self.append_launcher_log(
                    &ctx.instance.path,
                    &format!("[Launcher] Failed to spawn process: {err}"),
                );
                self.make_launch_toast_for_instance(
                    &ctx.instance,
                    "Minecraft failed to launch",
                    ctx.instance.name.clone(),
                    true,
                );
                self.refresh_selected_content();
            }
        }
    }

    fn append_launcher_log(&self, instance_path: &str, line: &str) {
        let logs_dir = PathBuf::from(instance_path).join("logs");
        let _ = fs::create_dir_all(&logs_dir);
        let path = logs_dir.join("launcher.log");
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{}", line);
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
                    self.running_process_pids.remove(&instance.path);
                    self.persist_running_processes();
                    self.last_stopped_instance_path = Some(instance.path.clone());
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
        } else if let Some(pid) = self.running_pid_for_instance(&instance.path) {
            let result = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .status();
            match result {
                Ok(s) if s.success() => {
                    self.running_process_pids.remove(&instance.path);
                    self.persist_running_processes();
                    self.last_stopped_instance_path = Some(instance.path.clone());
                    self.status = format!("Stopped {}", instance.name);
                }
                Ok(_) => {
                    self.status = format!("Failed to stop {} (pid {})", instance.name, pid);
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
        self.create_mode = CreateMode::Custom;
        self.create_name.clear();
        self.create_group.clear();
        self.create_game_version.clear();
        self.create_loader = None;
        self.create_import_path.clear();
        self.request_create_versions();
        self.show_create_dialog = true;
    }

    fn request_create_versions(&mut self) {
        if self.create_versions_loading {
            return;
        }
        let (tx, rx) = mpsc::channel::<Result<Vec<String>, String>>();
        self.create_versions_receiver = Some(rx);
        self.create_versions_loading = true;
        std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .user_agent("PrismarineLauncher-Rust")
                .build()
                .map_err(|e| format!("failed to build http client: {e}"));
            let result = (|| -> Result<Vec<String>, String> {
                let client = client?;
                let response = client
                    .get("https://piston-meta.mojang.com/mc/game/version_manifest_v2.json")
                    .send()
                    .map_err(|e| format!("failed to fetch version manifest: {e}"))?;
                if !response.status().is_success() {
                    return Err(format!("version manifest returned {}", response.status()));
                }
                let json = response
                    .json::<serde_json::Value>()
                    .map_err(|e| format!("failed to parse version manifest: {e}"))?;
                let mut releases = Vec::new();
                let mut others = Vec::new();
                if let Some(arr) = json.get("versions").and_then(|x| x.as_array()) {
                    for item in arr {
                        let Some(id) = item.get("id").and_then(|x| x.as_str()) else {
                            continue;
                        };
                        let kind = item
                            .get("type")
                            .and_then(|x| x.as_str())
                            .unwrap_or("release");
                        if kind == "release" {
                            releases.push(id.to_string());
                        } else {
                            others.push(id.to_string());
                        }
                    }
                }
                releases.extend(others.into_iter().take(60));
                Ok(releases.into_iter().take(180).collect())
            })();
            let _ = tx.send(result);
        });
    }

    fn poll_create_versions(&mut self) {
        let Some(rx) = self.create_versions_receiver.as_ref() else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        self.create_versions_loading = false;
        self.create_versions_receiver = None;
        match result {
            Ok(list) => {
                self.create_versions = list;
            }
            Err(err) => {
                self.status = format!("Failed to load Minecraft versions: {err}");
            }
        }
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
                ui.checkbox(&mut self.show_download_panel, "Show Download Panel");
            });

            ui.menu_button("Folders", |ui| {
                ui.label(format!("Data root: {}", self.data_root.display()));
                ui.label(format!("Instances: {}", self.instance_root().display()));
                if ui.button("Reload Instances").clicked() {
                    self.reload_instances();
                    ui.close();
                }
            });

            let active = self.active_account().cloned().unwrap_or_default();
            let active_name = active.name.clone();
            if active.account_type == AccountType::Offline {
                let tex = self.ensure_offline_head_texture(ui.ctx());
                let resp = ui.menu_image_text_button(
                    (tex.id(), egui::vec2(14.0, 14.0)),
                    "Accounts",
                    |ui| self.draw_accounts_menu_contents(ui),
                );
                resp.response
                    .on_hover_text(format!("Active account: {active_name}"));
            } else if let Some(head_path) = self.ensure_account_head_cached_async(&active_name) {
                if let Some(tex) = self.ensure_icon_texture(ui.ctx(), &head_path) {
                    let resp = ui.menu_image_text_button(
                        (tex.id(), egui::vec2(14.0, 14.0)),
                        "Accounts",
                        |ui| self.draw_accounts_menu_contents(ui),
                    );
                    resp.response
                        .on_hover_text(format!("Active account: {active_name}"));
                } else {
                    ui.menu_button("Accounts", |ui| self.draw_accounts_menu_contents(ui));
                }
            } else {
                ui.menu_button("Accounts", |ui| self.draw_accounts_menu_contents(ui));
            }

            ui.menu_button("Help", |ui| {
                ui.label("PrismarineLauncher Rust UI migration build");
                ui.label("Goal: parity with existing Qt interface and behavior");
            });
        });
    }

    fn draw_accounts_menu_contents(&mut self, ui: &mut egui::Ui) {
        for idx in 0..self.accounts.len() {
            let active = self.accounts[idx].active;
            let name = self.accounts[idx].name.clone();
            let is_offline = self.accounts[idx].account_type == AccountType::Offline;
            ui.horizontal(|ui| {
                if is_offline {
                    let tex = self.ensure_offline_head_texture(ui.ctx());
                    let avatar_resp = ui.image((tex.id(), egui::vec2(16.0, 16.0)));
                    avatar_resp.on_hover_text(format!("Player: {name}"));
                } else if let Some(head_path) = self.ensure_account_head_cached_async(&name) {
                    if let Some(tex) = self.ensure_icon_texture(ui.ctx(), &head_path) {
                        let avatar_resp = ui.image((tex.id(), egui::vec2(16.0, 16.0)));
                        avatar_resp.on_hover_text(format!("Player: {name}"));
                    } else {
                        ui.add_space(16.0);
                    }
                } else {
                    ui.add_space(16.0);
                }
                if ui.selectable_label(active, name).clicked() {
                    self.set_active_account(idx);
                }
            });
        }
        ui.separator();
        if ui.button("Manage Accounts...").clicked() {
            self.manage_account_selected = self
                .accounts
                .iter()
                .position(|a| a.active)
                .or_else(|| (!self.accounts.is_empty()).then_some(0));
            self.show_manage_accounts_dialog = true;
            ui.close();
        }
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
        let ext = Path::new(icon_path)
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let (rgba, width, height) = if ext == "svg" {
            decode_svg_rgba(&bytes)?
        } else {
            let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
            let w = decoded.width() as usize;
            let h = decoded.height() as usize;
            (decoded.into_raw(), w, h)
        };
        let size = [width, height];
        if size[0] == 0 || size[1] == 0 {
            return None;
        }
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
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

    fn ensure_builtin_icon_texture(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        bytes: &[u8],
    ) -> Option<egui::TextureHandle> {
        if let Some(tex) = self.icon_cache.get(key) {
            return Some(tex.clone());
        }
        let decoded = image::load_from_memory(bytes).ok()?.to_rgba8();
        let size = [decoded.width() as usize, decoded.height() as usize];
        if size[0] == 0 || size[1] == 0 {
            return None;
        }
        let color = egui::ColorImage::from_rgba_unmultiplied(size, decoded.as_raw());
        let tex = ctx.load_texture(key.to_string(), color, egui::TextureOptions::LINEAR);
        self.icon_cache.insert(key.to_string(), tex.clone());
        Some(tex)
    }

    fn ensure_loader_icon_texture(
        &mut self,
        ctx: &egui::Context,
        loader: &str,
    ) -> Option<egui::TextureHandle> {
        match loader {
            "fabric" => self.ensure_builtin_icon_texture(
                ctx,
                "loader_icon_fabric",
                include_bytes!("../ui/assets/loader_icons/fabric.png"),
            ),
            "forge" => self.ensure_builtin_icon_texture(
                ctx,
                "loader_icon_forge",
                include_bytes!("../ui/assets/loader_icons/forge.png"),
            ),
            "quilt" => self.ensure_builtin_icon_texture(
                ctx,
                "loader_icon_quilt",
                include_bytes!("../ui/assets/loader_icons/quilt.png"),
            ),
            "neoforge" => self.ensure_builtin_icon_texture(
                ctx,
                "loader_icon_neoforge",
                include_bytes!("../ui/assets/loader_icons/neoforge.png"),
            ),
            _ => self.ensure_builtin_icon_texture(
                ctx,
                "loader_icon_vanilla",
                include_bytes!("../ui/assets/loader_icons/vanilla.png"),
            ),
        }
    }

    fn ensure_create_loader_icon_texture(
        &mut self,
        ctx: &egui::Context,
        loader: &CreateLoader,
    ) -> Option<egui::TextureHandle> {
        self.ensure_loader_icon_texture(ctx, loader.cfg_value())
    }

    fn ensure_offline_head_texture(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        let key = "offline_head_texture_custom";
        if let Some(tex) = self.icon_cache.get(key) {
            return tex.clone();
        }
        let cache_path = self
            .data_root
            .join("cache")
            .join("offline")
            .join("offline_head.png");
        if !cache_path.is_file() {
            let _ = download_file_to_path(
                &format!("https://minotar.net/helm/{OFFLINE_SKIN_ID}/64.png"),
                &cache_path,
            );
        }
        if let Some(tex) = self.ensure_icon_texture(ctx, &cache_path.display().to_string()) {
            self.icon_cache.insert(key.to_string(), tex.clone());
            return tex;
        }
        let fallback_key = "offline_head_texture_fallback";
        if let Some(tex) = self.icon_cache.get(fallback_key) {
            return tex.clone();
        }
        let mut rgba = Vec::with_capacity(8 * 8 * 4);
        for y in 0..8 {
            for x in 0..8 {
                let c = if (x + y) % 2 == 0 { 118 } else { 90 };
                rgba.extend_from_slice(&[c, c, c, 255]);
            }
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([8, 8], &rgba);
        let tex = ctx.load_texture(
            fallback_key.to_string(),
            image,
            egui::TextureOptions::NEAREST,
        );
        self.icon_cache
            .insert(fallback_key.to_string(), tex.clone());
        tex
    }

    fn draw_instance_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let old_selectable = ui.style().interaction.selectable_labels;
        ui.style_mut().interaction.selectable_labels = false;
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.filter);
        });
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut bucket_order = vec![String::new()];
            for g in self.all_group_names() {
                if !bucket_order.iter().any(|x| x == &g) {
                    bucket_order.push(g);
                }
            }
            let mut rendered_any = false;
            let mut pending_drop_group: Option<String> = None;
            let mut pending_create_group = false;
            let mut pending_set_group_icon: Option<String> = None;
            let mut pending_delete_group: Option<String> = None;
            let mut pending_assign_group: Option<String> = None;
            let mut pending_launch_selected = false;
            let filter_lc = self.filter.to_lowercase();

            for group in bucket_order {
                let group_top = ui.cursor().min.y;
                let visible_indices: Vec<usize> = (0..self.instances.len())
                    .filter(|idx| {
                        let inst = &self.instances[*idx];
                        if inst.group != group {
                            return false;
                        }
                        if filter_lc.is_empty() {
                            return true;
                        }
                        inst.name.to_lowercase().contains(&filter_lc)
                    })
                    .collect();
                if visible_indices.is_empty() && self.dragging_instance_path.is_none() {
                    continue;
                }
                rendered_any = true;

                let title = if group.is_empty() {
                    "No group".to_string()
                } else {
                    format!("Group: {}", group)
                };
                let header_resp = ui
                    .horizontal(|ui| {
                        if !group.is_empty()
                            && let Some(icon) = self.group_icon_path(&group)
                            && let Some(tex) = self.ensure_icon_texture(ctx, &icon)
                        {
                            ui.image((tex.id(), egui::vec2(16.0, 16.0)));
                        }
                        ui.strong(title);
                        if self.dragging_instance_path.is_some() {
                            ui.label("← drop here");
                        }
                    })
                    .response;
                header_resp.context_menu(|ui| {
                    if ui.button("Create Group").clicked() {
                        pending_create_group = true;
                        ui.close();
                    }
                    if !group.is_empty() {
                        if ui.button("Rename Group").clicked() {
                            self.rename_group_old = group.clone();
                            self.rename_group_new = group.clone();
                            self.show_rename_group_dialog = true;
                            ui.close();
                        }
                        if ui.button("Set Group Icon").clicked() {
                            pending_set_group_icon = Some(group.clone());
                            ui.close();
                        }
                        if ui.button("Delete Group").clicked() {
                            pending_delete_group = Some(group.clone());
                            ui.close();
                        }
                    }
                });

                for idx in visible_indices {
                    let instance = self.instances[idx].clone();
                    let mut meta = vec![instance.version.clone(), instance.loader.clone()];
                    if instance.running {
                        meta.push("running".to_string());
                    } else if self.last_stopped_instance_path.as_deref() == Some(&instance.path) {
                        meta.push("stopped".to_string());
                    }
                    let icon_size = 22.0;
                    let row_height = (ui.text_style_height(&egui::TextStyle::Body) * 2.0 + 6.0)
                        .max(icon_size + 4.0);
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(icon_size + 4.0, row_height),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                if let Some(icon_path) = &instance.icon_path {
                                    if let Some(tex) = self.ensure_icon_texture(ctx, icon_path) {
                                        ui.image((tex.id(), egui::vec2(icon_size, icon_size)));
                                    } else {
                                        ui.add_space(icon_size);
                                    }
                                } else if let Some(tex) =
                                    self.ensure_loader_icon_texture(ctx, &instance.loader)
                                {
                                    ui.image((tex.id(), egui::vec2(icon_size, icon_size)));
                                } else {
                                    ui.add_space(icon_size);
                                }
                            },
                        );
                        let is_selected = self.selected == Some(idx);
                        let line = format!("{}\n[{}]", instance.name, meta.join(" | "));
                        let text = egui::RichText::new(line).size(18.0);
                        let fill = if is_selected {
                            ui.visuals().selection.bg_fill
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        let response = egui::Frame::NONE
                            .fill(fill)
                            .show(ui, |ui| {
                                let mut out = None;
                                ui.allocate_ui_with_layout(
                                    egui::vec2(ui.available_width(), row_height),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        out = Some(
                                            ui.add(
                                                egui::Label::new(text)
                                                    .wrap()
                                                    .sense(egui::Sense::click_and_drag()),
                                            ),
                                        );
                                    },
                                );
                                out.expect("instance row response")
                            })
                            .inner;
                        if response.clicked() {
                            self.selected = Some(idx);
                        }
                        if response.double_clicked() {
                            self.selected = Some(idx);
                            pending_launch_selected = true;
                        }
                        if response.drag_started() {
                            self.selected = Some(idx);
                            self.dragging_instance_path = Some(instance.path.clone());
                        }
                        response.context_menu(|ui| {
                            let is_running = instance.running;
                            if ui
                                .button(if is_running { "Kill" } else { "Launch" })
                                .clicked()
                            {
                                self.selected = Some(idx);
                                if is_running {
                                    self.do_kill_instance();
                                } else {
                                    self.do_launch_instance();
                                }
                                ui.close();
                            }
                            ui.separator();
                            ui.menu_button("Group Management", |ui| {
                                if ui.button("Move to No group").clicked() {
                                    self.selected = Some(idx);
                                    pending_assign_group = Some(String::new());
                                    ui.close();
                                }
                                for g in self.all_group_names() {
                                    if ui.button(format!("Move to {}", g)).clicked() {
                                        self.selected = Some(idx);
                                        pending_assign_group = Some(g);
                                        ui.close();
                                    }
                                }
                                if ui.button("Create Group").clicked() {
                                    pending_create_group = true;
                                    ui.close();
                                }
                            });
                            ui.separator();
                            if ui.button("Set Icon").clicked() {
                                self.selected = Some(idx);
                                self.open_set_icon_dialog_for_instance(idx);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Delete Instance").clicked() {
                                self.selected = Some(idx);
                                self.show_delete_dialog = true;
                                ui.close();
                            }
                        });
                    });
                }
                ui.add_space(6.0);

                let group_bottom = ui.cursor().min.y;
                let group_rect = egui::Rect::from_min_max(
                    egui::pos2(ui.min_rect().left(), group_top),
                    egui::pos2(ui.max_rect().right(), group_bottom),
                );
                if self.dragging_instance_path.is_some() {
                    let pointer_pos = ui.input(|i| i.pointer.latest_pos());
                    if let Some(pos) = pointer_pos
                        && group_rect.contains(pos)
                    {
                        let painter = ui.painter();
                        painter.rect_stroke(
                            group_rect,
                            2.0,
                            egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                            egui::StrokeKind::Outside,
                        );
                        if ui.input(|i| i.pointer.any_released()) && pending_drop_group.is_none() {
                            pending_drop_group = Some(group.clone());
                        }
                    }
                }
            }

            if !rendered_any {
                let empty_resp = ui.label("No instances");
                empty_resp.context_menu(|ui| {
                    if ui.button("Create Group").clicked() {
                        pending_create_group = true;
                        ui.close();
                    }
                });
            }

            if pending_create_group {
                self.show_create_group_dialog = true;
                self.create_group_name.clear();
            }
            if let Some(group_name) = pending_set_group_icon {
                self.do_set_group_icon(&group_name);
            }
            if let Some(group_name) = pending_delete_group {
                self.do_delete_group(&group_name);
            }
            if let Some(group_name) = pending_assign_group {
                self.do_assign_selected_instance_group(&group_name);
            }
            if pending_launch_selected {
                self.do_launch_instance();
            }
            if let Some(target_group) = pending_drop_group
                && let Some(path) = self.dragging_instance_path.clone()
            {
                self.do_assign_instance_group_by_path(&path, &target_group);
            }
            if ui.input(|i| i.pointer.any_released()) {
                self.dragging_instance_path = None;
            }
        });
        ui.style_mut().interaction.selectable_labels = old_selectable;
    }

    fn tab_selector(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.active_tab, CenterTab::Overview, "Overview");
            ui.selectable_value(&mut self.active_tab, CenterTab::Mods, "Content");
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
                "Group: {}",
                if instance.group.trim().is_empty() {
                    "No group"
                } else {
                    &instance.group
                }
            ));
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
        let current_total = match self.download_content_type {
            DownloadContentType::Mods => self.mods_cache.len(),
            DownloadContentType::ResourcePacks => self.resourcepacks_cache.len(),
            DownloadContentType::ShaderPacks => self.shaderpacks_cache.len(),
            DownloadContentType::Worlds => self.worlds_cache.len(),
            DownloadContentType::Servers => self.servers_cache.len(),
            DownloadContentType::Screenshots => self.screenshots_cache.len(),
        };
        let search_label = match self.download_content_type {
            DownloadContentType::Mods => "Installed mods search:",
            DownloadContentType::ResourcePacks => "Installed resource packs search:",
            DownloadContentType::ShaderPacks => "Installed shader packs search:",
            DownloadContentType::Worlds => "Installed worlds search:",
            DownloadContentType::Servers => "Installed servers search:",
            DownloadContentType::Screenshots => "Installed screenshots search:",
        };
        ui.horizontal(|ui| {
            if ui.button("Add Content").clicked() {
                self.do_add_content_file();
            }
            if self.download_content_type == DownloadContentType::Mods
                && ui.button("Auto Update Mods").clicked()
            {
                self.do_auto_update_mods();
            }
            if ui.button("Download Content").clicked() {
                self.show_download_panel = !self.show_download_panel;
            }
            ui.separator();
            ui.label(format!("Total: {}", current_total));
        });
        egui::SidePanel::left("content_categories_left")
            .resizable(false)
            .exact_width(122.0)
            .show_inside(ui, |ui| {
                let items: &[(DownloadContentType, &str, &str)] = &[
                    (
                        DownloadContentType::Mods,
                        "Mods",
                        "https://minecraft.wiki/images/Impulse_Command_Block.gif?fb024?download",
                    ),
                    (
                        DownloadContentType::ResourcePacks,
                        "Resource Packs",
                        "https://minecraft.wiki/images/White_Dye_JE2_BE2.png?f9a07?download",
                    ),
                    (
                        DownloadContentType::ShaderPacks,
                        "Shader Packs",
                        "https://ru.minecraft.wiki/images/%D0%A1%D0%B2%D0%B5%D1%82%D1%8F%D1%89%D0%B8%D0%B9%D1%81%D1%8F_%D1%87%D0%B5%D1%80%D0%BD%D0%B8%D0%BB%D1%8C%D0%BD%D1%8B%D0%B9_%D0%BC%D0%B5%D1%88%D0%BE%D0%BA_JE1.png?377c1?download",
                    ),
                    (
                        DownloadContentType::Worlds,
                        "Worlds",
                        "https://minecraft.wiki/images/Grass_Block_JE7_BE6.png?2bd37?download",
                    ),
                    (
                        DownloadContentType::Servers,
                        "Servers",
                        "https://minecraft.wiki/images/Repeating_Command_Block.gif?7ab3a?download",
                    ),
                    (
                        DownloadContentType::Screenshots,
                        "Screenshots",
                        "https://minecraft.wiki/images/Painting_JE2_BE2.png?45334?download",
                    ),
                ];
                for (idx, (kind, title, icon_url)) in items.iter().enumerate() {
                    let selected = self.download_content_type == *kind;
                    let mut clicked = false;
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.vertical_centered(|ui| {
                        if let Some(tex) = self.ensure_icon_texture_from_source(ui.ctx(), icon_url) {
                            if ui
                                .add(
                                    egui::Button::image((tex.id(), egui::vec2(34.0, 34.0)))
                                        .selected(selected),
                                )
                                .clicked()
                            {
                                clicked = true;
                            }
                        }
                        let label = truncate_with_ellipsis(title, 15);
                        if ui
                            .add_sized(
                                [104.0, 18.0],
                                egui::Button::new(egui::RichText::new(label).size(12.5))
                                    .selected(selected)
                                    .frame(false),
                            )
                            .clicked()
                        {
                            clicked = true;
                        }
                        });
                    });
                    if clicked {
                        self.download_content_type = kind.clone();
                        if self.show_download_panel
                            && matches!(
                                self.download_content_type,
                                DownloadContentType::Mods
                                    | DownloadContentType::ResourcePacks
                                    | DownloadContentType::ShaderPacks
                            )
                        {
                            self.request_selected_download_details();
                            self.start_download_search(false);
                        } else if !matches!(
                            self.download_content_type,
                            DownloadContentType::Mods
                                | DownloadContentType::ResourcePacks
                                | DownloadContentType::ShaderPacks
                        ) {
                            self.show_download_panel = false;
                        }
                    }
                    if idx + 1 < items.len() {
                        let (sep_rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 2.0),
                            egui::Sense::hover(),
                        );
                        let y = sep_rect.center().y;
                        ui.painter().line_segment(
                            [egui::pos2(sep_rect.left(), y), egui::pos2(sep_rect.right(), y)],
                            egui::Stroke::new(
                                1.0,
                                ui.visuals().widgets.noninteractive.bg_stroke.color,
                            ),
                        );
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.label(search_label);
            ui.text_edit_singleline(&mut self.mods_filter);
        });
        ui.separator();

        let supports_downloads = matches!(
            self.download_content_type,
            DownloadContentType::Mods
                | DownloadContentType::ResourcePacks
                | DownloadContentType::ShaderPacks
        );
        if self.show_download_panel && !supports_downloads {
            self.show_download_panel = false;
        }

        let mut top_panel_max_height = f32::INFINITY;
        let mut split_total_height = 0.0f32;
        if self.show_download_panel {
            let avail = ui.available_height().max(360.0);
            split_total_height = avail;
            let upper = (avail - 120.0).max(120.0);
            top_panel_max_height = (avail * self.content_list_ratio).clamp(72.0, upper);
        }

        let mut pending_delete: Option<String> = None;
        if self.download_content_type == DownloadContentType::Mods {
            let mut pending_toggle: Option<(String, bool)> = None;
            let filtered_mod_indices: Vec<usize> = if self.mods_filter.trim().is_empty() {
                (0..self.mods_cache.len()).collect()
            } else {
                let needle = self.mods_filter.to_lowercase();
                self.mods_cache
                    .iter()
                    .enumerate()
                    .filter(|m| {
                        m.1.display_name.to_lowercase().contains(&needle)
                            || m.1.name.to_lowercase().contains(&needle)
                            || m.1.provider.to_lowercase().contains(&needle)
                    })
                    .map(|(idx, _)| idx)
                    .collect()
            };
            let table_font_size = 17.0;
            let row_height = ui.text_style_height(&egui::TextStyle::Body).max(24.0);
            let icon_size = (row_height + 2.0) * 1.3;
            if filtered_mod_indices.is_empty() {
                ui.label("No content found");
            } else {
                let total_width = ui.available_width().max(520.0);
                let col_enable = 72.0;
                let col_image = 44.0;
                let col_version = 150.0;
                let col_updated = 130.0;
                let col_provider = 110.0;
                let spacing = ui.spacing().item_spacing.x;
                let fixed = col_enable + col_image + col_version + col_updated + col_provider;
                let gaps = 5.0 * spacing;
                let col_name = (total_width - fixed - gaps).max(220.0);
                let mut scroll = egui::ScrollArea::vertical().id_salt("mods_table_scroll");
                if self.show_download_panel {
                    scroll = scroll.max_height(top_panel_max_height);
                }
                scroll.show(ui, |ui| {
                    egui::Grid::new("mods_table_grid")
                        .num_columns(6)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.add_sized(
                                [col_enable, 0.0],
                                egui::Label::new(
                                    egui::RichText::new("Enable").size(table_font_size),
                                ),
                            );
                            ui.add_sized(
                                [col_image, 0.0],
                                egui::Label::new(
                                    egui::RichText::new("Image").size(table_font_size),
                                ),
                            );
                            ui.add_sized(
                                [col_name, 0.0],
                                egui::Label::new(egui::RichText::new("Name").size(table_font_size)),
                            );
                            ui.add_sized(
                                [col_version, 0.0],
                                egui::Label::new(
                                    egui::RichText::new("Version").size(table_font_size),
                                ),
                            );
                            ui.add_sized(
                                [col_updated, 0.0],
                                egui::Label::new(
                                    egui::RichText::new("Updated").size(table_font_size),
                                ),
                            );
                            ui.add_sized(
                                [col_provider, 0.0],
                                egui::Label::new(
                                    egui::RichText::new("Provider").size(table_font_size),
                                ),
                            );
                            ui.end_row();

                            for idx in &filtered_mod_indices {
                                let (
                                    enabled_now,
                                    file_path,
                                    icon_path,
                                    display_name,
                                    version,
                                    updated_at,
                                    provider,
                                ) = {
                                    let item = &self.mods_cache[*idx];
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

                                ui.allocate_ui_with_layout(
                                    egui::vec2(col_enable, row_height + 2.0),
                                    egui::Layout::left_to_right(egui::Align::Min),
                                    |ui| {
                                        let mut enabled = enabled_now;
                                        if ui.checkbox(&mut enabled, "").changed() {
                                            pending_toggle = Some((file_path.clone(), enabled));
                                        }
                                    },
                                );
                                ui.allocate_ui_with_layout(
                                    egui::vec2(col_image, row_height + 2.0),
                                    egui::Layout::left_to_right(egui::Align::Min),
                                    |ui| {
                                        if let Some(icon_path) = &icon_path {
                                            if let Some(tex) = self.ensure_icon_texture_from_source(
                                                ui.ctx(),
                                                icon_path,
                                            ) {
                                                ui.image((
                                                    tex.id(),
                                                    egui::vec2(icon_size, icon_size),
                                                ));
                                            } else {
                                                ui.add_space(icon_size);
                                            }
                                        } else {
                                            ui.add_space(icon_size);
                                        }
                                    },
                                );
                                let name_resp = ui.add_sized(
                                    [col_name, row_height + 2.0],
                                    egui::Label::new(
                                        egui::RichText::new(display_name.as_str())
                                            .size(table_font_size),
                                    )
                                    .truncate(),
                                );
                                name_resp.context_menu(|ui| {
                                    if ui.button("Delete mod").clicked() {
                                        pending_delete = Some(file_path.clone());
                                        ui.close();
                                    }
                                });
                                if name_resp.hovered() {
                                    name_resp.clone().on_hover_text(display_name.clone());
                                }
                                if self.download_content_type == DownloadContentType::Screenshots
                                    && name_resp.double_clicked()
                                {
                                    self.copy_image_file_to_clipboard(&file_path);
                                }
                                ui.add_sized(
                                    [col_version, row_height + 2.0],
                                    egui::Label::new(
                                        egui::RichText::new(compact_mod_version(&version))
                                            .size(table_font_size),
                                    )
                                    .truncate(),
                                );
                                ui.add_sized(
                                    [col_updated, row_height + 2.0],
                                    egui::Label::new(
                                        egui::RichText::new(updated_at).size(table_font_size),
                                    )
                                    .truncate(),
                                );
                                ui.add_sized(
                                    [col_provider, row_height + 2.0],
                                    egui::Label::new(
                                        egui::RichText::new(provider.as_str())
                                            .size(table_font_size),
                                    )
                                    .truncate(),
                                );
                                ui.end_row();
                            }
                        });
                });
            }
            if let Some((file_path, enabled)) = pending_toggle {
                self.do_toggle_mod_enabled(&file_path, enabled);
            }
        } else {
            let mut pending_toggle: Option<(String, bool)> = None;
            let source = match self.download_content_type {
                DownloadContentType::ResourcePacks => &self.resourcepacks_cache,
                DownloadContentType::ShaderPacks => &self.shaderpacks_cache,
                DownloadContentType::Worlds => &self.worlds_cache,
                DownloadContentType::Servers => &self.servers_cache,
                DownloadContentType::Screenshots => &self.screenshots_cache,
                DownloadContentType::Mods => unreachable!(),
            };
            let supports_toggle = matches!(
                self.download_content_type,
                DownloadContentType::ResourcePacks | DownloadContentType::ShaderPacks
            );
            let filtered: Vec<ContentListEntry> = if self.mods_filter.trim().is_empty() {
                source.clone()
            } else {
                let needle = self.mods_filter.to_lowercase();
                source
                    .iter()
                    .filter(|x| {
                        x.display_name.to_lowercase().contains(&needle)
                            || x.name.to_lowercase().contains(&needle)
                            || x.provider.to_lowercase().contains(&needle)
                    })
                    .cloned()
                    .collect()
            };
            if filtered.is_empty() {
                let msg = match self.download_content_type {
                    DownloadContentType::ResourcePacks => "No resource packs found",
                    DownloadContentType::ShaderPacks => "No shader packs found",
                    DownloadContentType::Worlds => "No worlds found",
                    DownloadContentType::Servers => "No server data found",
                    DownloadContentType::Screenshots => "No screenshots found",
                    DownloadContentType::Mods => "No content found",
                };
                ui.label(msg);
            } else {
                if self.download_content_type == DownloadContentType::Screenshots {
                    let min_tile_w = 190.0;
                    let tile_gap = 14.0;
                    let total_w = ui.available_width().max(min_tile_w);
                    let cols =
                        (((total_w + tile_gap) / (min_tile_w + tile_gap)).floor() as usize).max(1);
                    let tile_w = ((total_w - tile_gap * (cols.saturating_sub(1)) as f32)
                        / cols as f32)
                        .max(min_tile_w);
                    let thumb_w = (tile_w - 10.0).clamp(160.0, 320.0);
                    let thumb_h = (thumb_w * 9.0 / 16.0).round();
                    let mut scroll =
                        egui::ScrollArea::vertical().id_salt("screenshots_grid_scroll");
                    if self.show_download_panel {
                        scroll = scroll.max_height(top_panel_max_height);
                    }
                    scroll.show(ui, |ui| {
                        egui::Grid::new("screenshots_grid")
                            .num_columns(cols)
                            .spacing([14.0, 14.0])
                            .show(ui, |ui| {
                                for (idx, item) in filtered.iter().enumerate() {
                                    let file_path = item.file_path.clone();
                                    let mut label = item.display_name.clone();
                                    if let Some(stem) =
                                        Path::new(&label).file_stem().and_then(|x| x.to_str())
                                    {
                                        label = stem.to_string();
                                    }
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(tile_w, thumb_h + 32.0),
                                        egui::Layout::top_down(egui::Align::LEFT),
                                        |ui| {
                                            let image_resp =
                                                if let Some(icon_path) = &item.icon_path {
                                                    if let Some(tex) = self
                                                        .ensure_screenshot_thumbnail_texture(
                                                            ui.ctx(),
                                                            icon_path,
                                                        )
                                                    {
                                                        ui.add(
                                                            egui::Image::new((
                                                                tex.id(),
                                                                egui::vec2(thumb_w, thumb_h),
                                                            ))
                                                            .sense(egui::Sense::click()),
                                                        )
                                                    } else {
                                                        let resp = ui.allocate_response(
                                                            egui::vec2(thumb_w, thumb_h),
                                                            egui::Sense::click(),
                                                        );
                                                        ui.painter().text(
                                                            resp.rect.center(),
                                                            egui::Align2::CENTER_CENTER,
                                                            "Loading...",
                                                            egui::FontId::proportional(12.0),
                                                            ui.visuals().weak_text_color(),
                                                        );
                                                        resp
                                                    }
                                                } else {
                                                    ui.allocate_response(
                                                        egui::vec2(thumb_w, thumb_h),
                                                        egui::Sense::click(),
                                                    )
                                                };
                                            if image_resp.double_clicked() {
                                                self.copy_image_file_to_clipboard(&file_path);
                                            }
                                            image_resp.context_menu(|ui| {
                                                if ui.button("Copy screenshot").clicked() {
                                                    self.copy_image_file_to_clipboard(&file_path);
                                                    ui.close();
                                                }
                                                if ui.button("Delete content").clicked() {
                                                    pending_delete = Some(file_path.clone());
                                                    ui.close();
                                                }
                                            });
                                            ui.add_sized(
                                                [thumb_w, 18.0],
                                                egui::Label::new(
                                                    egui::RichText::new(truncate_with_ellipsis(
                                                        &label, 28,
                                                    ))
                                                    .size(14.0)
                                                    .color(ui.visuals().text_color()),
                                                ),
                                            );
                                        },
                                    );
                                    if idx % cols == cols - 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });
                } else if self.download_content_type == DownloadContentType::Servers {
                    let table_font_size = 17.0;
                    let row_height = ui.text_style_height(&egui::TextStyle::Body).max(28.0);
                    let icon_size = 38.0;
                    let total_width = ui.available_width().max(520.0);
                    let col_name = (total_width * 0.56).max(300.0);
                    let col_addr = (total_width * 0.28).max(180.0);
                    let col_online = (total_width - col_name - col_addr).max(80.0);
                    let mut scroll = egui::ScrollArea::vertical().id_salt("servers_table_scroll");
                    if self.show_download_panel {
                        scroll = scroll.max_height(top_panel_max_height);
                    }
                    scroll.show(ui, |ui| {
                        egui::Grid::new("servers_table_grid")
                            .num_columns(3)
                            .striped(true)
                            .show(ui, |ui| {
                                ui.add_sized(
                                    [col_name, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Name").size(table_font_size),
                                    ),
                                );
                                ui.add_sized(
                                    [col_addr, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Address").size(table_font_size),
                                    ),
                                );
                                ui.add_sized(
                                    [col_online, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Online").size(table_font_size),
                                    ),
                                );
                                ui.end_row();

                                for item in &filtered {
                                    let file_path = item.file_path.clone();
                                    let display_name = item.display_name.clone();
                                    let addr = item.version.clone();
                                    let online = item.updated_at.clone();
                                    let icon_path = item.icon_path.clone();

                                    let name_resp = ui
                                        .allocate_ui_with_layout(
                                            egui::vec2(col_name, row_height + 8.0),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                if let Some(icon) = &icon_path {
                                                    if let Some(tex) = self
                                                        .ensure_icon_texture_from_source(
                                                            ui.ctx(),
                                                            icon,
                                                        )
                                                    {
                                                        ui.image((
                                                            tex.id(),
                                                            egui::vec2(icon_size, icon_size),
                                                        ));
                                                    } else {
                                                        ui.add_space(icon_size);
                                                    }
                                                } else {
                                                    ui.add_space(icon_size);
                                                }
                                                ui.label(
                                                    egui::RichText::new(display_name.as_str())
                                                        .size(table_font_size),
                                                );
                                            },
                                        )
                                        .response;
                                    name_resp.context_menu(|ui| {
                                        if ui.button("Delete content").clicked() {
                                            pending_delete = Some(file_path.clone());
                                            ui.close();
                                        }
                                    });

                                    ui.add_sized(
                                        [col_addr, row_height + 8.0],
                                        egui::Label::new(
                                            egui::RichText::new(addr.as_str())
                                                .size(table_font_size),
                                        )
                                        .truncate(),
                                    );
                                    ui.add_sized(
                                        [col_online, row_height + 8.0],
                                        egui::Label::new(
                                            egui::RichText::new(online.as_str())
                                                .size(table_font_size),
                                        )
                                        .truncate(),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
                } else {
                    let table_font_size = 17.0;
                    let row_height = ui.text_style_height(&egui::TextStyle::Body).max(24.0);
                    let icon_size = (row_height + 2.0) * 1.3;
                    let total_width = ui.available_width().max(520.0);
                    let col_enable = 72.0;
                    let col_image = 44.0;
                    let col_version = 150.0;
                    let col_updated = 130.0;
                    let col_provider = 110.0;
                    let spacing = ui.spacing().item_spacing.x;
                    let fixed = col_enable + col_image + col_version + col_updated + col_provider;
                    let gaps = 5.0 * spacing;
                    let col_name = (total_width - fixed - gaps).max(220.0);
                    let mut scroll = egui::ScrollArea::vertical().id_salt("content_table_scroll");
                    if self.show_download_panel {
                        scroll = scroll.max_height(top_panel_max_height);
                    }
                    scroll.show(ui, |ui| {
                        egui::Grid::new("content_table_grid")
                            .num_columns(6)
                            .striped(true)
                            .show(ui, |ui| {
                                ui.add_sized(
                                    [col_enable, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Enable").size(table_font_size),
                                    ),
                                );
                                ui.add_sized(
                                    [col_image, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Image").size(table_font_size),
                                    ),
                                );
                                ui.add_sized(
                                    [col_name, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Name").size(table_font_size),
                                    ),
                                );
                                ui.add_sized(
                                    [col_version, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Version").size(table_font_size),
                                    ),
                                );
                                ui.add_sized(
                                    [col_updated, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Updated").size(table_font_size),
                                    ),
                                );
                                ui.add_sized(
                                    [col_provider, 0.0],
                                    egui::Label::new(
                                        egui::RichText::new("Provider").size(table_font_size),
                                    ),
                                );
                                ui.end_row();

                                for item in &filtered {
                                    let enabled_now = item.enabled;
                                    let file_path = item.file_path.clone();
                                    let icon_path = item.icon_path.clone();
                                    let display_name = item.display_name.clone();
                                    let version = item.version.clone();
                                    let updated_at = item.updated_at.clone();
                                    let provider = item.provider.clone();

                                    ui.allocate_ui_with_layout(
                                        egui::vec2(col_enable, row_height + 2.0),
                                        egui::Layout::left_to_right(egui::Align::Min),
                                        |ui| {
                                            if supports_toggle {
                                                let mut enabled = enabled_now;
                                                if ui.checkbox(&mut enabled, "").changed() {
                                                    pending_toggle =
                                                        Some((file_path.clone(), enabled));
                                                }
                                            } else {
                                                ui.add_space(18.0);
                                            }
                                        },
                                    );
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(col_image, row_height + 2.0),
                                        egui::Layout::left_to_right(egui::Align::Min),
                                        |ui| {
                                            if let Some(icon_path) = &icon_path {
                                                if let Some(tex) = self
                                                    .ensure_icon_texture_from_source(
                                                        ui.ctx(),
                                                        icon_path,
                                                    )
                                                {
                                                    ui.image((
                                                        tex.id(),
                                                        egui::vec2(icon_size, icon_size),
                                                    ));
                                                } else {
                                                    ui.add_space(icon_size);
                                                }
                                            } else {
                                                ui.add_space(icon_size);
                                            }
                                        },
                                    );
                                    let name_resp = ui.add_sized(
                                        [col_name, row_height + 2.0],
                                        egui::Label::new(
                                            egui::RichText::new(display_name.as_str())
                                                .size(table_font_size),
                                        )
                                        .truncate(),
                                    );
                                    name_resp.context_menu(|ui| {
                                        if ui.button("Delete content").clicked() {
                                            pending_delete = Some(file_path.clone());
                                            ui.close();
                                        }
                                    });
                                    if name_resp.hovered() {
                                        name_resp.on_hover_text(display_name.clone());
                                    }
                                    ui.add_sized(
                                        [col_version, row_height + 2.0],
                                        egui::Label::new(
                                            egui::RichText::new(compact_mod_version(&version))
                                                .size(table_font_size),
                                        )
                                        .truncate(),
                                    );
                                    ui.add_sized(
                                        [col_updated, row_height + 2.0],
                                        egui::Label::new(
                                            egui::RichText::new(updated_at).size(table_font_size),
                                        )
                                        .truncate(),
                                    );
                                    ui.add_sized(
                                        [col_provider, row_height + 2.0],
                                        egui::Label::new(
                                            egui::RichText::new(provider.as_str())
                                                .size(table_font_size),
                                        )
                                        .truncate(),
                                    );
                                    ui.end_row();
                                }
                            });
                    });
                }
            }
            if let Some((file_path, enabled)) = pending_toggle {
                self.do_toggle_content_enabled(&file_path, enabled);
            }
        }
        if let Some(file_path) = pending_delete {
            self.do_delete_mod_file(&file_path);
        }

        if self.show_download_panel {
            let (split_rect, split_resp) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.0), egui::Sense::drag());
            ui.painter().line_segment(
                [
                    egui::pos2(split_rect.left(), split_rect.center().y),
                    egui::pos2(split_rect.right(), split_rect.center().y),
                ],
                egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            );
            if split_resp.hovered() || split_resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            }
            if split_resp.dragged() && split_total_height > 1.0 {
                self.content_list_ratio = (self.content_list_ratio
                    + split_resp.drag_delta().y / split_total_height)
                    .clamp(0.08, 0.92);
            }
        }

        if !self.show_download_panel {
            return;
        }

        ui.separator();
        ui.group(|ui| {
            let mut provider_changed = false;
            let mut content_changed = false;
            let mut close_downloads = false;
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("✕")
                        .on_hover_text("Hide Downloads")
                        .clicked()
                    {
                        close_downloads = true;
                    }
                });
                ui.heading("Downloads");
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
                content_changed |= ui
                    .selectable_value(
                        &mut self.download_content_type,
                        DownloadContentType::ShaderPacks,
                        "Shader Packs",
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
            if close_downloads {
                self.show_download_panel = false;
                return;
            }
            if provider_changed || content_changed {
                self.request_selected_download_details();
                self.start_download_search(false);
            }
            ui.label(format!(
                "Auto-detected: loader={} | version={}",
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
                    let panel_height = ui.available_height().max(140.0);
                    let split_gap = ui.spacing().item_spacing.x.max(2.0);
                    let total_w = ui.available_width().max(2.0);
                    let content_w = (total_w - split_gap).max(2.0);
                    let description_ratio = 0.56_f32;
                    let right_w = (content_w * description_ratio).floor().max(1.0);
                    let left_w = (content_w - right_w).max(1.0);
                    ui.horizontal(|ui| {
                        let mut row_end = 0usize;
                        let mut pending_select = None;
                        let mut pending_queue = None;
                        let list_font_size = 17.0;
                        let list_base_row = ui.text_style_height(&egui::TextStyle::Body).max(24.0);
                        let list_icon_size = (list_base_row + 2.0) * 1.3;
                        let list_row_height = (list_icon_size + 4.0).max(list_base_row + 4.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(left_w, panel_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.label("Results");
                                ui.separator();
                                egui::ScrollArea::vertical()
                                    .id_salt("download_modrinth_results_scroll")
                                    .max_height(panel_height)
                                    .show_rows(
                                        ui,
                                        list_row_height,
                                        self.modrinth_hits.len(),
                                        |ui, row_range| {
                                            row_end = row_range.end;
                                            for idx in row_range {
                                                let (icon_url, label) = {
                                                    let hit = &self.modrinth_hits[idx];
                                                    let mut label = hit.title.clone();
                                                    if !hit.author.trim().is_empty() {
                                                        label.push_str(&format!(
                                                            " ({})",
                                                            hit.author
                                                        ));
                                                    } else {
                                                        label.push_str(&format!(" ({})", hit.slug));
                                                    }
                                                    (hit.icon_url.clone(), label)
                                                };
                                                let selected =
                                                    self.selected_modrinth_hit == Some(idx);
                                                let row_resp = ui
                                                    .horizontal(|ui| {
                                                        if let Some(icon) = &icon_url {
                                                            if let Some(tex) = self
                                                                .ensure_icon_texture_from_source(
                                                                    ui.ctx(),
                                                                    icon,
                                                                )
                                                            {
                                                                ui.image((
                                                                    tex.id(),
                                                                    egui::vec2(
                                                                        list_icon_size,
                                                                        list_icon_size,
                                                                    ),
                                                                ));
                                                            } else {
                                                                ui.add_space(list_icon_size);
                                                            }
                                                        } else {
                                                            ui.add_space(list_icon_size);
                                                        }
                                                        let text_width =
                                                            ui.available_width().max(80.0);
                                                        let max_chars = ((text_width / 9.0).floor()
                                                            as usize)
                                                            .max(12);
                                                        let label_short = truncate_with_ellipsis(
                                                            &label, max_chars,
                                                        );
                                                        let response = ui.add_sized(
                                                            [text_width, list_row_height],
                                                            egui::Button::new(
                                                                egui::RichText::new(label_short)
                                                                    .size(list_font_size),
                                                            )
                                                            .selected(selected)
                                                            .frame(false),
                                                        );
                                                        if response.clicked() {
                                                            pending_select = Some(idx);
                                                        }
                                                        if response.double_clicked() {
                                                            pending_queue = Some(idx);
                                                        }
                                                    })
                                                    .response;
                                                if row_resp.clicked() {
                                                    pending_select = Some(idx);
                                                }
                                                if row_resp.double_clicked() {
                                                    pending_queue = Some(idx);
                                                }
                                            }
                                        },
                                    );
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
                        ui.add_space(split_gap);
                        ui.allocate_ui_with_layout(
                            egui::vec2(right_w, panel_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                if self.download_search_loading {
                                    ui.label("Loading...");
                                } else if self.modrinth_hits.is_empty() {
                                    ui.label("No results");
                                }
                                ui.label("Description");
                                ui.separator();
                                let details_icon = self.download_details.icon_url.clone();
                                egui::ScrollArea::vertical()
                                    .id_salt("download_modrinth_description_scroll")
                                    .max_height(panel_height)
                                    .show(ui, |ui| {
                                        if let Some(icon) = details_icon.as_deref()
                                            && let Some(tex) =
                                                self.ensure_icon_texture_from_source(ui.ctx(), icon)
                                        {
                                            ui.image((tex.id(), egui::vec2(56.0, 56.0)));
                                        }
                                        if !self.download_details.title.is_empty() {
                                            ui.heading(&self.download_details.title);
                                        }
                                        if self.download_details.markdown.trim().is_empty() {
                                            ui.label("No project selected");
                                        } else {
                                            let max_img_w = (ui.available_width() - 28.0)
                                                .clamp(96.0, 260.0)
                                                as usize;
                                            ui.push_id("modrinth_markdown_panel", |ui| {
                                                egui_commonmark::CommonMarkViewer::new()
                                                    .max_image_width(Some(max_img_w))
                                                    .show(
                                                        ui,
                                                        &mut self.markdown_cache_modrinth,
                                                        &self.download_details.markdown,
                                                    );
                                            });
                                        }
                                    });
                            },
                        );
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
                    let panel_height = ui.available_height().max(140.0);
                    let split_gap = ui.spacing().item_spacing.x.max(2.0);
                    let total_w = ui.available_width().max(2.0);
                    let content_w = (total_w - split_gap).max(2.0);
                    let description_ratio = 0.56_f32;
                    let right_w = (content_w * description_ratio).floor().max(1.0);
                    let left_w = (content_w - right_w).max(1.0);
                    ui.horizontal(|ui| {
                        let mut row_end = 0usize;
                        let mut pending_select = None;
                        let mut pending_queue = None;
                        let list_font_size = 17.0;
                        let list_base_row = ui.text_style_height(&egui::TextStyle::Body).max(24.0);
                        let list_icon_size = (list_base_row + 2.0) * 1.3;
                        let list_row_height = (list_icon_size + 4.0).max(list_base_row + 4.0);
                        ui.allocate_ui_with_layout(
                            egui::vec2(left_w, panel_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.label("Results");
                                ui.separator();
                                egui::ScrollArea::vertical()
                                    .id_salt("download_curseforge_results_scroll")
                                    .max_height(panel_height)
                                    .show_rows(
                                        ui,
                                        list_row_height,
                                        self.curseforge_hits.len(),
                                        |ui, row_range| {
                                            row_end = row_range.end;
                                            for idx in row_range {
                                                let (icon_url, label) = {
                                                    let hit = &self.curseforge_hits[idx];
                                                    let mut label = hit.title.clone();
                                                    if !hit.author.trim().is_empty() {
                                                        label.push_str(&format!(
                                                            " ({})",
                                                            hit.author
                                                        ));
                                                    }
                                                    (hit.icon_url.clone(), label)
                                                };
                                                let selected =
                                                    self.selected_curseforge_hit == Some(idx);
                                                let row_resp = ui
                                                    .horizontal(|ui| {
                                                        if let Some(icon) = &icon_url {
                                                            if let Some(tex) = self
                                                                .ensure_icon_texture_from_source(
                                                                    ui.ctx(),
                                                                    icon,
                                                                )
                                                            {
                                                                ui.image((
                                                                    tex.id(),
                                                                    egui::vec2(
                                                                        list_icon_size,
                                                                        list_icon_size,
                                                                    ),
                                                                ));
                                                            } else {
                                                                ui.add_space(list_icon_size);
                                                            }
                                                        } else {
                                                            ui.add_space(list_icon_size);
                                                        }
                                                        let text_width =
                                                            ui.available_width().max(80.0);
                                                        let max_chars = ((text_width / 9.0).floor()
                                                            as usize)
                                                            .max(12);
                                                        let label_short = truncate_with_ellipsis(
                                                            &label, max_chars,
                                                        );
                                                        let response = ui.add_sized(
                                                            [text_width, list_row_height],
                                                            egui::Button::new(
                                                                egui::RichText::new(label_short)
                                                                    .size(list_font_size),
                                                            )
                                                            .selected(selected)
                                                            .frame(false),
                                                        );
                                                        if response.clicked() {
                                                            pending_select = Some(idx);
                                                        }
                                                        if response.double_clicked() {
                                                            pending_queue = Some(idx);
                                                        }
                                                    })
                                                    .response;
                                                if row_resp.clicked() {
                                                    pending_select = Some(idx);
                                                }
                                                if row_resp.double_clicked() {
                                                    pending_queue = Some(idx);
                                                }
                                            }
                                        },
                                    );
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
                        ui.add_space(split_gap);
                        ui.allocate_ui_with_layout(
                            egui::vec2(right_w, panel_height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                if self.download_search_loading {
                                    ui.label("Loading...");
                                } else if self.curseforge_hits.is_empty() {
                                    ui.label("No results");
                                }
                                ui.label("Description");
                                ui.separator();
                                let details_icon = self.download_details.icon_url.clone();
                                egui::ScrollArea::vertical()
                                    .id_salt("download_curseforge_description_scroll")
                                    .max_height(panel_height)
                                    .show(ui, |ui| {
                                        if let Some(icon) = details_icon.as_deref()
                                            && let Some(tex) =
                                                self.ensure_icon_texture_from_source(ui.ctx(), icon)
                                        {
                                            ui.image((tex.id(), egui::vec2(56.0, 56.0)));
                                        }
                                        if !self.download_details.title.is_empty() {
                                            ui.heading(&self.download_details.title);
                                        }
                                        if self.download_details.markdown.trim().is_empty() {
                                            ui.label("No project selected");
                                        } else {
                                            let max_img_w = (ui.available_width() - 28.0)
                                                .clamp(96.0, 260.0)
                                                as usize;
                                            ui.push_id("curseforge_markdown_panel", |ui| {
                                                egui_commonmark::CommonMarkViewer::new()
                                                    .max_image_width(Some(max_img_w))
                                                    .show(
                                                        ui,
                                                        &mut self.markdown_cache_curseforge,
                                                        &self.download_details.markdown,
                                                    );
                                            });
                                        }
                                    });
                            },
                        );
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
            egui::ScrollArea::vertical()
                .id_salt("logs_files_scroll")
                .show(&mut cols[0], |ui| {
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
            egui::ScrollArea::vertical()
                .id_salt("logs_preview_scroll")
                .show(&mut cols[1], |ui| {
                    if self.log_preview.is_empty() {
                        ui.label("No log selected");
                    } else {
                        ui.monospace(&self.log_preview);
                    }
                });
        });
    }

    fn draw_settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.horizontal_wrapped(|ui| {
            ui.label("Scope:");
            ui.selectable_value(&mut self.settings_target, SettingsTarget::Global, "Global");
            ui.selectable_value(
                &mut self.settings_target,
                SettingsTarget::Instance,
                "Selected Instance",
            );
            ui.separator();
            ui.selectable_value(
                &mut self.settings_subtab,
                SettingsSubTab::General,
                "General",
            );
            ui.selectable_value(&mut self.settings_subtab, SettingsSubTab::Java, "Java");
            ui.selectable_value(&mut self.settings_subtab, SettingsSubTab::Launch, "Launch");
            ui.selectable_value(
                &mut self.settings_subtab,
                SettingsSubTab::UserCommands,
                "User Commands",
            );
            ui.selectable_value(
                &mut self.settings_subtab,
                SettingsSubTab::Environment,
                "Environment Variables",
            );
        });
        ui.separator();

        if self.settings_target == SettingsTarget::Global {
            self.draw_global_settings_ui(ui);
        } else {
            self.draw_instance_settings_ui(ui);
        }
    }

    fn draw_global_settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        match self.settings_subtab {
            SettingsSubTab::General => {
                ui.group(|ui| {
                    ui.label(format!("Storage: {}", self.data_root.display()));
                    ui.label(format!("Instances: {}", self.instance_root().display()));
                    ui.label(
                        "All accounts and instances are stored in ~/.local/share/PrismarineLauncher.",
                    );
                });
                ui.add_space(8.0);
                ui.group(|ui| {
                    ui.label("Launch settings mode");
                    changed |= ui
                        .selectable_value(
                            &mut self.global_settings.mode,
                            LaunchSettingsMode::Basic,
                            "Basic",
                        )
                        .changed();
                    changed |= ui
                        .selectable_value(
                            &mut self.global_settings.mode,
                            LaunchSettingsMode::Advanced,
                            "Advanced",
                        )
                        .changed();
                    ui.label("Global defaults for new and unknown instances.");
                });
            }
            SettingsSubTab::Java => {
                ui.group(|ui| {
                    ui.label("Global Java setup");
                    ui.label("Java executable");
                    changed |= ui
                        .text_edit_singleline(&mut self.global_settings.java_path)
                        .changed();
                    ui.horizontal(|ui| {
                        if ui.button("Find").clicked() {
                            self.do_find_java();
                        }
                        if ui.button("Browse").clicked() {
                            self.do_browse_java();
                        }
                    });
                    changed |= ui
                        .checkbox(
                            &mut self.global_settings.skip_java_compat_check,
                            "Skip Java compatibility checks",
                        )
                        .changed();
                    if ui.button("Check settings").clicked() {
                        self.do_check_java_settings();
                    }
                });
            }
            SettingsSubTab::Launch => {
                ui.group(|ui| {
                    ui.label("Memory");
                    ui.horizontal(|ui| {
                        ui.label("Minimum memory:");
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
                        ui.label("Maximum memory:");
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
                        ui.label("PermGen size:");
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
                    ui.label("Java arguments");
                    changed |= ui
                        .add(
                            egui::TextEdit::multiline(&mut self.global_settings.advanced_jvm_args)
                                .desired_rows(8),
                        )
                        .changed();
                    ui.label("Used in Advanced mode.");
                });
            }
            SettingsSubTab::UserCommands => {
                ui.group(|ui| {
                    ui.label("User commands (one per line)");
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
                    ui.label("Environment variables (KEY=VALUE, one per line)");
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

    fn draw_instance_settings_ui(&mut self, ui: &mut egui::Ui) {
        let Some(instance_path) = self.selected_instance_path() else {
            ui.label("Select instance first.");
            return;
        };
        let Some(instance) = self.selected_instance().cloned() else {
            ui.label("Select instance first.");
            return;
        };
        let mut cfg = load_prism_instance_config(&instance_path).unwrap_or_default();
        let mut changed = false;

        ui.group(|ui| {
            ui.label(format!("Instance: {}", instance.name));
            ui.label("Fallback values are taken from Global settings when override is disabled.");
        });
        ui.add_space(8.0);

        match self.settings_subtab {
            SettingsSubTab::General => {
                ui.group(|ui| {
                    ui.label("Instance overrides");
                    changed |= ui
                        .checkbox(&mut cfg.override_java_location, "Override Java executable")
                        .changed();
                    changed |= ui
                        .checkbox(&mut cfg.override_java_args, "Override Java arguments")
                        .changed();
                    changed |= ui
                        .checkbox(&mut cfg.override_memory, "Override memory limits")
                        .changed();
                    changed |= ui
                        .checkbox(&mut cfg.override_commands, "Override launch commands")
                        .changed();
                });
                ui.add_space(8.0);
                ui.group(|ui| {
                    let current_loader = self.detect_instance_loader(&instance_path);
                    let current_text = if current_loader.trim().is_empty() {
                        "Vanilla".to_string()
                    } else {
                        match current_loader.as_str() {
                            "fabric" => "Fabric".to_string(),
                            "forge" => "Forge".to_string(),
                            "quilt" => "Quilt".to_string(),
                            "neoforge" => "Neo-Forge".to_string(),
                            other => other.to_string(),
                        }
                    };
                    ui.label(format!("Current loader: {current_text}"));
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Vanilla").clicked() {
                            self.do_set_selected_instance_loader(None);
                        }
                        if ui.button("Fabric").clicked() {
                            self.do_set_selected_instance_loader(Some(CreateLoader::Fabric));
                        }
                        if ui.button("Forge").clicked() {
                            self.do_set_selected_instance_loader(Some(CreateLoader::Forge));
                        }
                        if ui.button("Quilt").clicked() {
                            self.do_set_selected_instance_loader(Some(CreateLoader::Quilt));
                        }
                        if ui.button("Neo-Forge").clicked() {
                            self.do_set_selected_instance_loader(Some(CreateLoader::NeoForge));
                        }
                    });
                });
            }
            SettingsSubTab::Java => {
                ui.group(|ui| {
                    changed |= ui
                        .checkbox(
                            &mut cfg.override_java_location,
                            "Use custom Java for this instance",
                        )
                        .changed();
                    if cfg.override_java_location {
                        let java = cfg.java_path.get_or_insert_with(String::new);
                        changed |= ui.text_edit_singleline(java).changed();
                        ui.horizontal(|ui| {
                            if ui.button("Find").clicked() {
                                let mut seen = HashSet::new();
                                for candidate in Self::discover_java_candidates() {
                                    if !candidate.exists() {
                                        continue;
                                    }
                                    let key = candidate.display().to_string();
                                    if !seen.insert(key.clone()) {
                                        continue;
                                    }
                                    if let Some(version) = Self::probe_java_runtime(&candidate) {
                                        cfg.java_path = Some(key.clone());
                                        changed = true;
                                        self.status = format!(
                                            "Instance Java found for {}: {version}",
                                            instance.name
                                        );
                                        break;
                                    }
                                }
                            }
                            if ui.button("Browse").clicked() {
                                let mut dialog =
                                    rfd::FileDialog::new().set_title("Select Java executable");
                                if let Ok(home) = std::env::var("HOME") {
                                    dialog = dialog.set_directory(home);
                                }
                                if let Some(path) = dialog.pick_file() {
                                    cfg.java_path = Some(path.display().to_string());
                                    changed = true;
                                }
                            }
                        });
                    }
                    ui.separator();
                    changed |= ui
                        .checkbox(
                            &mut cfg.override_java_args,
                            "Use custom JVM args for this instance",
                        )
                        .changed();
                    if cfg.override_java_args {
                        let args = cfg.java_args.get_or_insert_with(String::new);
                        changed |= ui
                            .add(egui::TextEdit::multiline(args).desired_rows(6))
                            .changed();
                    }
                });
            }
            SettingsSubTab::Launch => {
                ui.group(|ui| {
                    changed |= ui
                        .checkbox(
                            &mut cfg.override_memory,
                            "Use custom memory for this instance",
                        )
                        .changed();
                    if cfg.override_memory {
                        let min = cfg.min_mem_alloc.get_or_insert(512);
                        let max = cfg.max_mem_alloc.get_or_insert(4096);
                        let perm = cfg.perm_gen.get_or_insert(128);
                        ui.horizontal(|ui| {
                            ui.label("Minimum memory:");
                            changed |= ui
                                .add(egui::DragValue::new(min).speed(64).range(256..=131072))
                                .changed();
                            ui.label("MiB (-Xms)");
                        });
                        ui.horizontal(|ui| {
                            ui.label("Maximum memory:");
                            changed |= ui
                                .add(egui::DragValue::new(max).speed(64).range(256..=131072))
                                .changed();
                            ui.label("MiB (-Xmx)");
                        });
                        ui.horizontal(|ui| {
                            ui.label("PermGen size:");
                            changed |= ui
                                .add(egui::DragValue::new(perm).speed(16).range(0..=4096))
                                .changed();
                            ui.label("MiB (-XX:PermSize)");
                        });
                        if *max < *min {
                            *max = *min;
                            changed = true;
                        }
                    }
                });
            }
            SettingsSubTab::UserCommands => {
                ui.group(|ui| {
                    changed |= ui
                        .checkbox(
                            &mut cfg.override_commands,
                            "Use custom commands for this instance",
                        )
                        .changed();
                    if cfg.override_commands {
                        ui.label("Pre-launch command");
                        let pre = cfg.pre_launch_command.get_or_insert_with(String::new);
                        changed |= ui.text_edit_singleline(pre).changed();
                        ui.label("Post-exit command");
                        let post = cfg.post_exit_command.get_or_insert_with(String::new);
                        changed |= ui.text_edit_singleline(post).changed();
                        ui.label("Wrapper command");
                        let wrap = cfg.wrapper_command.get_or_insert_with(String::new);
                        changed |= ui.text_edit_singleline(wrap).changed();
                    }
                });
            }
            SettingsSubTab::Environment => {
                ui.group(|ui| {
                    ui.label("Per-instance environment variables are not supported yet.");
                    ui.label("Use Global -> Environment Variables.");
                });
            }
        }

        if changed {
            match save_instance_launch_overrides(&instance_path, &cfg) {
                Ok(_) => {
                    self.status = format!("Saved instance settings: {}", instance.name);
                }
                Err(err) => {
                    self.status = format!("Failed to save instance settings: {err}");
                }
            }
        }
    }

    fn draw_dialogs(&mut self, ctx: &egui::Context) {
        if self.show_version_update_dialog {
            egui::Window::new("Launcher Updated")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .default_width(540.0)
                .show(ctx, |ui| {
                    let current = launcher_version_string();
                    ui.heading("You updated to a new version");
                    ui.separator();
                    if let Some(prev) = &self.previous_launcher_version {
                        ui.label(format!("Previous version: {prev}"));
                    }
                    ui.label(format!("Current version: {current}"));
                    ui.label("Version format: major.build (for example 1.0000001).");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            self.show_version_update_dialog = false;
                            self.previous_launcher_version = None;
                            self.last_seen_launcher_version = current;
                        }
                    });
                });
        }

        if self.show_set_icon_dialog {
            let target_path = self.set_icon_target_instance_path.clone();
            let target_idx = target_path
                .as_ref()
                .and_then(|p| self.instances.iter().position(|i| &i.path == p));
            if target_idx.is_none() {
                self.show_set_icon_dialog = false;
                self.set_icon_target_instance_path = None;
            } else {
                let target_idx = target_idx.unwrap_or(0);
                let target_name = self.instances[target_idx].name.clone();
                let target_instance_path = self.instances[target_idx].path.clone();
                let other_icons: Vec<(String, String)> = self
                    .instances
                    .iter()
                    .filter(|i| i.path != target_instance_path)
                    .filter_map(|i| i.icon_path.clone().map(|p| (i.name.clone(), p)))
                    .collect();
                let custom_icons = self.list_custom_icon_files();
                let mut import_custom = false;
                let mut remove_selected_custom = false;
                let mut open_icons_folder = false;
                let mut apply_selection = false;
                let search = self.set_icon_search.trim().to_ascii_lowercase();

                egui::Window::new("Set Instance Icon")
                    .collapsible(false)
                    .resizable(true)
                    .default_width(900.0)
                    .default_height(700.0)
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Instance:");
                            ui.monospace(&target_name);
                        });
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label("Search:");
                            ui.add_sized(
                                [ui.available_width(), 28.0],
                                egui::TextEdit::singleline(&mut self.set_icon_search)
                                    .hint_text("Search icons..."),
                            );
                        });
                        ui.add_space(6.0);
                        let builtins = [
                            (
                                "Vanilla",
                                "builtin_vanilla",
                                "set_icon_builtin_vanilla",
                                include_bytes!("../ui/assets/loader_icons/vanilla.png").as_slice(),
                            ),
                            (
                                "Fabric",
                                "builtin_fabric",
                                "set_icon_builtin_fabric",
                                include_bytes!("../ui/assets/loader_icons/fabric.png").as_slice(),
                            ),
                            (
                                "Forge",
                                "builtin_forge",
                                "set_icon_builtin_forge",
                                include_bytes!("../ui/assets/loader_icons/forge.png").as_slice(),
                            ),
                            (
                                "Quilt",
                                "builtin_quilt",
                                "set_icon_builtin_quilt",
                                include_bytes!("../ui/assets/loader_icons/quilt.png").as_slice(),
                            ),
                            (
                                "NeoForge",
                                "builtin_neoforge",
                                "set_icon_builtin_neoforge",
                                include_bytes!("../ui/assets/loader_icons/neoforge.png").as_slice(),
                            ),
                        ];
                        let tile_w = 118.0;
                        let columns = ((ui.available_width() / tile_w).floor().max(1.0)) as usize;

                        egui::ScrollArea::vertical()
                            .id_salt("set_icon_unified_list_scroll")
                            .max_height(470.0)
                            .show(ui, |ui| {
                                ui.heading("Built-in Icons");
                                ui.separator();
                                egui::Grid::new("set_icon_builtin_grid")
                                    .num_columns(columns)
                                    .spacing([14.0, 12.0])
                                    .show(ui, |ui| {
                                        let mut drawn = 0usize;
                                        if search.is_empty() || "default".contains(&search) {
                                            let selected = self.set_icon_selected_kind == 1;
                                            ui.vertical(|ui| {
                                                let button =
                                                    egui::Button::new("Default").selected(selected);
                                                if ui.add_sized([94.0, 58.0], button).clicked() {
                                                    self.set_icon_selected_kind = 1;
                                                    self.set_icon_selected_value.clear();
                                                }
                                                ui.add_sized(
                                                    [94.0, 18.0],
                                                    egui::Label::new("Default").wrap(),
                                                );
                                            });
                                            drawn += 1;
                                            if drawn.is_multiple_of(columns) {
                                                ui.end_row();
                                            }
                                        }
                                        for (title, icon_key, tex_key, bytes) in builtins {
                                            if !search.is_empty()
                                                && !title.to_ascii_lowercase().contains(&search)
                                            {
                                                continue;
                                            }
                                            let selected = self.set_icon_selected_kind == 2
                                                && self.set_icon_selected_value == icon_key;
                                            ui.vertical(|ui| {
                                                if let Some(tex) = self.ensure_builtin_icon_texture(
                                                    ui.ctx(),
                                                    tex_key,
                                                    bytes,
                                                ) {
                                                    let button = egui::Button::image((
                                                        tex.id(),
                                                        egui::vec2(56.0, 56.0),
                                                    ))
                                                    .selected(selected);
                                                    if ui.add_sized([96.0, 64.0], button).clicked()
                                                    {
                                                        self.set_icon_selected_kind = 2;
                                                        self.set_icon_selected_value =
                                                            icon_key.to_string();
                                                    }
                                                }
                                                ui.add_sized(
                                                    [96.0, 18.0],
                                                    egui::Label::new(truncate_with_ellipsis(
                                                        title, 16,
                                                    )),
                                                );
                                            });
                                            drawn += 1;
                                            if drawn.is_multiple_of(columns) {
                                                ui.end_row();
                                            }
                                        }
                                    });

                                ui.add_space(8.0);
                                ui.heading("Icons From Other Instances");
                                ui.separator();
                                if other_icons.is_empty() {
                                    ui.label("No other instance icons found");
                                } else {
                                    egui::Grid::new("set_icon_other_grid")
                                        .num_columns(columns)
                                        .spacing([14.0, 12.0])
                                        .show(ui, |ui| {
                                            let mut drawn = 0usize;
                                            for (name, icon_path) in &other_icons {
                                                if !search.is_empty()
                                                    && !name.to_ascii_lowercase().contains(&search)
                                                {
                                                    continue;
                                                }
                                                let selected = self.set_icon_selected_kind == 3
                                                    && self.set_icon_selected_value == *icon_path;
                                                ui.vertical(|ui| {
                                                    if let Some(tex) = self
                                                        .ensure_icon_texture(ui.ctx(), icon_path)
                                                    {
                                                        let button = egui::Button::image((
                                                            tex.id(),
                                                            egui::vec2(56.0, 56.0),
                                                        ))
                                                        .selected(selected);
                                                        if ui
                                                            .add_sized([96.0, 64.0], button)
                                                            .clicked()
                                                        {
                                                            self.set_icon_selected_kind = 3;
                                                            self.set_icon_selected_value =
                                                                icon_path.clone();
                                                        }
                                                    }
                                                    let short_name =
                                                        truncate_with_ellipsis(name, 16);
                                                    let name_resp = ui.add_sized(
                                                        [96.0, 32.0],
                                                        egui::Label::new(short_name),
                                                    );
                                                    if name_resp.hovered() {
                                                        name_resp.on_hover_text(name);
                                                    }
                                                });
                                                drawn += 1;
                                                if drawn.is_multiple_of(columns) {
                                                    ui.end_row();
                                                }
                                            }
                                        });
                                }

                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    ui.heading("Custom Icons");
                                    ui.add_space(8.0);
                                    if ui
                                        .add_sized(
                                            [170.0, 24.0],
                                            egui::Button::new("Import Custom Icon..."),
                                        )
                                        .clicked()
                                    {
                                        import_custom = true;
                                    }
                                });
                                ui.separator();
                                if custom_icons.is_empty() {
                                    ui.label("No custom icons in icon store");
                                } else {
                                    egui::Grid::new("set_icon_custom_grid")
                                        .num_columns(columns)
                                        .spacing([14.0, 12.0])
                                        .show(ui, |ui| {
                                            let mut drawn = 0usize;
                                            for icon_path in &custom_icons {
                                                let name = Path::new(icon_path)
                                                    .file_stem()
                                                    .and_then(|x| x.to_str())
                                                    .unwrap_or("custom")
                                                    .to_string();
                                                if !search.is_empty()
                                                    && !name.to_ascii_lowercase().contains(&search)
                                                {
                                                    continue;
                                                }
                                                let selected = self.set_icon_selected_kind == 4
                                                    && self.set_icon_selected_value == name;
                                                ui.vertical(|ui| {
                                                    if let Some(tex) = self
                                                        .ensure_icon_texture(ui.ctx(), icon_path)
                                                    {
                                                        let button = egui::Button::image((
                                                            tex.id(),
                                                            egui::vec2(56.0, 56.0),
                                                        ))
                                                        .selected(selected);
                                                        if ui
                                                            .add_sized([96.0, 64.0], button)
                                                            .clicked()
                                                        {
                                                            self.set_icon_selected_kind = 4;
                                                            self.set_icon_selected_value =
                                                                name.clone();
                                                        }
                                                    }
                                                    let short_name =
                                                        truncate_with_ellipsis(&name, 16);
                                                    let name_resp = ui.add_sized(
                                                        [96.0, 32.0],
                                                        egui::Label::new(short_name),
                                                    );
                                                    if name_resp.hovered() {
                                                        name_resp.on_hover_text(name.clone());
                                                    }
                                                });
                                                drawn += 1;
                                                if drawn.is_multiple_of(columns) {
                                                    ui.end_row();
                                                }
                                            }
                                        });
                                }
                            });

                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui
                                .add_sized([88.0, 24.0], egui::Button::new("Add Icon"))
                                .clicked()
                            {
                                import_custom = true;
                            }
                            if ui
                                .add_sized([102.0, 24.0], egui::Button::new("Remove Icon"))
                                .clicked()
                            {
                                remove_selected_custom = true;
                            }
                            if ui
                                .add_sized([102.0, 24.0], egui::Button::new("Open Folder"))
                                .clicked()
                            {
                                open_icons_folder = true;
                            }
                            ui.separator();
                            if ui
                                .add_sized([52.0, 24.0], egui::Button::new("OK"))
                                .clicked()
                            {
                                apply_selection = true;
                            }
                            if ui
                                .add_sized([70.0, 24.0], egui::Button::new("Cancel"))
                                .clicked()
                            {
                                self.show_set_icon_dialog = false;
                            }
                        });
                    });

                if import_custom {
                    self.import_custom_instance_icon();
                }
                if remove_selected_custom {
                    if self.set_icon_selected_kind == 4 {
                        let key = self.set_icon_selected_value.trim().to_string();
                        if key.is_empty() {
                            self.status = "No icon selected".to_string();
                        } else if let Some(path) = custom_icons.iter().find(|p| {
                            Path::new(p)
                                .file_stem()
                                .and_then(|x| x.to_str())
                                .unwrap_or_default()
                                == key
                        }) {
                            match fs::remove_file(path) {
                                Ok(_) => {
                                    self.icon_cache.clear();
                                    self.set_icon_selected_kind = 0;
                                    self.set_icon_selected_value.clear();
                                    self.status = format!("Deleted icon: {key}");
                                }
                                Err(err) => {
                                    self.status = format!("Failed to delete icon: {err}");
                                }
                            }
                        } else {
                            self.status = "Selected custom icon not found".to_string();
                        }
                    } else {
                        self.status = "Select a custom icon first".to_string();
                    }
                }
                if open_icons_folder {
                    let icons_dir = self.data_root.join("icons");
                    let _ = fs::create_dir_all(&icons_dir);
                    #[cfg(target_os = "linux")]
                    {
                        let open_target = icons_dir.display().to_string();
                        let _ = Command::new("sh")
                            .arg("-lc")
                            .arg(format!("xdg-open {}", shell_escape(&open_target)))
                            .spawn();
                    }
                }
                if apply_selection {
                    match self.set_icon_selected_kind {
                        1 => {
                            self.apply_instance_icon_key(&target_instance_path, None);
                            self.show_set_icon_dialog = false;
                        }
                        2 => {
                            let icon_key = self.set_icon_selected_value.trim().to_string();
                            if icon_key.is_empty() {
                                self.status = "No icon selected".to_string();
                            } else {
                                let builtin_bytes = match icon_key.as_str() {
                                    "builtin_vanilla" => Some(
                                        include_bytes!("../ui/assets/loader_icons/vanilla.png")
                                            .as_slice(),
                                    ),
                                    "builtin_fabric" => Some(
                                        include_bytes!("../ui/assets/loader_icons/fabric.png")
                                            .as_slice(),
                                    ),
                                    "builtin_forge" => Some(
                                        include_bytes!("../ui/assets/loader_icons/forge.png")
                                            .as_slice(),
                                    ),
                                    "builtin_quilt" => Some(
                                        include_bytes!("../ui/assets/loader_icons/quilt.png")
                                            .as_slice(),
                                    ),
                                    "builtin_neoforge" => Some(
                                        include_bytes!("../ui/assets/loader_icons/neoforge.png")
                                            .as_slice(),
                                    ),
                                    _ => None,
                                };
                                if let Some(bytes) = builtin_bytes {
                                    if self
                                        .write_builtin_icon_to_store(&icon_key, "png", bytes)
                                        .is_some()
                                    {
                                        self.apply_instance_icon_key(
                                            &target_instance_path,
                                            Some(&icon_key),
                                        );
                                        self.show_set_icon_dialog = false;
                                    } else {
                                        self.status = "Failed to write built-in icon".to_string();
                                    }
                                } else {
                                    self.status = "Unknown built-in icon".to_string();
                                }
                            }
                        }
                        4 => {
                            let icon_key = self.set_icon_selected_value.trim().to_string();
                            if icon_key.is_empty() {
                                self.status = "No icon selected".to_string();
                            } else {
                                self.apply_instance_icon_key(
                                    &target_instance_path,
                                    Some(&icon_key),
                                );
                                self.show_set_icon_dialog = false;
                            }
                        }
                        3 => {
                            let source_path = self.set_icon_selected_value.trim().to_string();
                            if source_path.is_empty() {
                                self.status = "No icon selected".to_string();
                            } else if let Some(icon_key) = copy_image_to_icon_store(
                                &self.data_root,
                                Path::new(&source_path),
                                &target_name,
                            ) {
                                self.apply_instance_icon_key(
                                    &target_instance_path,
                                    Some(&icon_key),
                                );
                                self.show_set_icon_dialog = false;
                            } else {
                                self.status =
                                    "Failed to copy icon from another instance".to_string();
                            }
                        }
                        _ => {
                            self.status = "Select icon first".to_string();
                        }
                    }
                }
            }
        }

        if self.show_create_dialog {
            egui::Window::new("Create Instance")
                .collapsible(false)
                .resizable(true)
                .default_width(760.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut self.create_name);
                    });
                    ui.separator();
                    ui.columns(2, |cols| {
                        cols[0].set_min_width(170.0);
                        cols[0].selectable_value(
                            &mut self.create_mode,
                            CreateMode::Custom,
                            "Custom",
                        );
                        cols[0].selectable_value(
                            &mut self.create_mode,
                            CreateMode::Import,
                            "Import",
                        );

                        cols[1].heading(match self.create_mode {
                            CreateMode::Custom => "Custom",
                            CreateMode::Import => "Import",
                        });
                        cols[1].separator();
                        match self.create_mode {
                            CreateMode::Custom => {
                                cols[1].label("Version (required):");
                                let _ =
                                    cols[1].text_edit_singleline(&mut self.create_game_version);
                                cols[1].label(
                                    "Supported loaders: Vanilla, Fabric, Forge, Quilt, Neo-Forge",
                                );
                                cols[1].horizontal(|ui| {
                                    if self.create_versions_loading {
                                        ui.label("Loading version list...");
                                    } else {
                                        ui.label("Version suggestions:");
                                    }
                                    if ui.button("Refresh").clicked() {
                                        self.request_create_versions();
                                    }
                                });
                                let needle = self.create_game_version.to_lowercase();
                                let filtered: Vec<String> = self
                                    .create_versions
                                    .iter()
                                    .filter(|v| {
                                        needle.is_empty()
                                            || v.to_lowercase().starts_with(&needle)
                                            || v.to_lowercase().contains(&needle)
                                    })
                                    .take(8)
                                    .cloned()
                                    .collect();
                                if !filtered.is_empty() {
                                    egui::ScrollArea::vertical()
                                        .max_height(120.0)
                                        .show(&mut cols[1], |ui| {
                                            for version in filtered {
                                                if ui.selectable_label(false, &version).clicked() {
                                                    self.create_game_version = version;
                                                }
                                            }
                                        });
                                }
                                cols[1].label("Mod loader (optional):");
                                egui::ComboBox::from_id_salt("create_loader")
                                    .selected_text(
                                        self.create_loader
                                            .as_ref()
                                            .map(CreateLoader::label)
                                            .unwrap_or("Vanilla (no loader)"),
                                    )
                                    .show_ui(&mut cols[1], |ui| {
                                        ui.horizontal(|ui| {
                                            if let Some(tex) =
                                                self.ensure_loader_icon_texture(ui.ctx(), "vanilla")
                                            {
                                                ui.image((tex.id(), egui::vec2(16.0, 16.0)));
                                            }
                                            ui.selectable_value(
                                                &mut self.create_loader,
                                                None,
                                                "Vanilla (no loader)",
                                            );
                                        });
                                        for loader in [
                                            CreateLoader::Fabric,
                                            CreateLoader::Forge,
                                            CreateLoader::Quilt,
                                            CreateLoader::NeoForge,
                                        ] {
                                            ui.horizontal(|ui| {
                                                if let Some(tex) = self
                                                    .ensure_create_loader_icon_texture(ui.ctx(), &loader)
                                                {
                                                    ui.image((tex.id(), egui::vec2(16.0, 16.0)));
                                                }
                                                ui.selectable_value(
                                                    &mut self.create_loader,
                                                    Some(loader.clone()),
                                                    loader.label(),
                                                );
                                            });
                                        }
                                    });
                            }
                            CreateMode::Import => {
                                cols[1].label("Pack archive (.mrpack or .zip):");
                                cols[1].horizontal(|ui| {
                                    ui.text_edit_singleline(&mut self.create_import_path);
                                    if ui.button("Browse").clicked() {
                                        self.do_browse_import_archive();
                                    }
                                });
                                if let Some(progress) = &self.import_progress {
                                    cols[1].separator();
                                    cols[1].label(progress.message.as_str());
                                    let total = progress.total.max(1) as f32;
                                    let value = (progress.done as f32 / total).clamp(0.0, 1.0);
                                    cols[1].add(
                                        egui::ProgressBar::new(value)
                                            .desired_width(cols[1].available_width()),
                                    );
                                    cols[1].monospace(format!("{}/{}", progress.done, progress.total));
                                }
                                cols[1].label(
                                    "Import support: only .mrpack and .zip (no FTB/CurseForge/Modrinth API).",
                                );
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        let can_submit = self.import_worker_rx.is_none();
                        if ui.add_enabled(can_submit, egui::Button::new("OK")).clicked() {
                            match self.create_mode {
                                CreateMode::Custom => self.do_create_instance(),
                                CreateMode::Import => self.do_import_instance(),
                            }
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

        if self.show_create_group_dialog {
            egui::Window::new("Create Group")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Group name:");
                        ui.text_edit_singleline(&mut self.create_group_name);
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            self.do_create_group();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_create_group_dialog = false;
                        }
                    });
                });
        }

        if self.show_rename_group_dialog {
            egui::Window::new("Rename Group")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Old:");
                        ui.text_edit_singleline(&mut self.rename_group_old);
                    });
                    ui.horizontal(|ui| {
                        ui.label("New:");
                        ui.text_edit_singleline(&mut self.rename_group_new);
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            self.do_rename_group();
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_rename_group_dialog = false;
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

        if self.show_manage_accounts_dialog {
            egui::Window::new("Accounts")
                .collapsible(false)
                .resizable(true)
                .default_width(760.0)
                .default_height(520.0)
                .show(ctx, |ui| {
                    ui.columns(2, |cols| {
                        cols[0].set_min_width(430.0);
                        cols[0].horizontal(|ui| {
                            ui.label("Username");
                            ui.add_space(72.0);
                            ui.label("Type");
                            ui.add_space(50.0);
                            ui.label("Status");
                        });
                        cols[0].separator();
                        egui::ScrollArea::vertical()
                            .id_salt("accounts_manage_scroll")
                            .show(&mut cols[0], |ui| {
                                for idx in 0..self.accounts.len() {
                                    let selected = self.manage_account_selected == Some(idx);
                                    let account = self.accounts[idx].clone();
                                    let account_type = match account.account_type {
                                        AccountType::Licensed => "Microsoft Account",
                                        AccountType::Offline => "Offline",
                                    };
                                    let status = if account.active {
                                        "Ready"
                                    } else if account.account_type == AccountType::Licensed
                                        && !account.licensed
                                    {
                                        "No license"
                                    } else {
                                        ""
                                    };
                                    ui.horizontal(|ui| {
                                        let active_mark = if account.active { "✓" } else { " " };
                                        ui.monospace(active_mark);
                                        if account.account_type == AccountType::Offline {
                                            let tex = self.ensure_offline_head_texture(ui.ctx());
                                            ui.image((tex.id(), egui::vec2(18.0, 18.0)));
                                        } else if let Some(head_path) =
                                            self.ensure_account_head_cached_async(&account.name)
                                        {
                                            if let Some(tex) =
                                                self.ensure_icon_texture(ui.ctx(), &head_path)
                                            {
                                                ui.image((tex.id(), egui::vec2(18.0, 18.0)));
                                            } else {
                                                ui.add_space(18.0);
                                            }
                                        } else {
                                            ui.add_space(18.0);
                                        }
                                        let label = format!(
                                            "{:<20}  {:<24}  {}",
                                            account.name, account_type, status
                                        );
                                        if ui.selectable_label(selected, label).clicked() {
                                            self.manage_account_selected = Some(idx);
                                        }
                                    });
                                }
                            });

                        cols[1].heading("Account Management");
                        cols[1].separator();
                        let action_btn_w = 170.0;
                        if cols[1]
                            .add_sized(
                                [action_btn_w, 26.0],
                                egui::Button::new("Login via Microsoft"),
                            )
                            .clicked()
                        {
                            self.show_add_account_dialog = true;
                            self.do_start_device_code_login();
                        }
                        cols[1].horizontal(|ui| {
                            ui.label("Add offline:");
                            ui.text_edit_singleline(&mut self.new_offline_account_name);
                        });
                        if cols[1]
                            .add_sized([action_btn_w, 26.0], egui::Button::new("Add offline"))
                            .clicked()
                        {
                            self.add_offline_account();
                        }
                        cols[1].separator();
                        let selected_ok = self
                            .manage_account_selected
                            .map(|i| i < self.accounts.len())
                            .unwrap_or(false);
                        if cols[1]
                            .add_enabled_ui(selected_ok, |ui| {
                                ui.add_sized([action_btn_w, 26.0], egui::Button::new("Refresh"))
                            })
                            .inner
                            .clicked()
                        {
                            self.refresh_selected_account();
                        }
                        if cols[1]
                            .add_enabled_ui(selected_ok, |ui| {
                                ui.add_sized([action_btn_w, 26.0], egui::Button::new("Delete"))
                            })
                            .inner
                            .clicked()
                        {
                            self.delete_selected_account();
                        }
                        if cols[1]
                            .add_enabled_ui(selected_ok, |ui| {
                                ui.add_sized(
                                    [action_btn_w, 26.0],
                                    egui::Button::new("Set as default"),
                                )
                            })
                            .inner
                            .clicked()
                            && let Some(idx) = self.manage_account_selected
                        {
                            self.set_active_account(idx);
                        }
                        if cols[1]
                            .add_sized(
                                [action_btn_w, 26.0],
                                egui::Button::new("Do not use by default"),
                            )
                            .clicked()
                        {
                            self.clear_active_account();
                        }
                        if cols[1]
                            .add_enabled_ui(selected_ok, |ui| {
                                ui.add_sized([action_btn_w, 26.0], egui::Button::new("Move up"))
                            })
                            .inner
                            .clicked()
                        {
                            self.move_selected_account(-1);
                        }
                        if cols[1]
                            .add_enabled_ui(selected_ok, |ui| {
                                ui.add_sized([action_btn_w, 26.0], egui::Button::new("Move down"))
                            })
                            .inner
                            .clicked()
                        {
                            self.move_selected_account(1);
                        }
                        if cols[1]
                            .add_enabled_ui(selected_ok, |ui| {
                                ui.add_sized(
                                    [action_btn_w, 26.0],
                                    egui::Button::new("Skin management"),
                                )
                            })
                            .inner
                            .clicked()
                        {
                            self.status = "Skin management is not implemented yet".to_string();
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized([52.0, 24.0], egui::Button::new("OK"))
                            .clicked()
                        {
                            self.show_manage_accounts_dialog = false;
                        }
                        if ui
                            .add_sized([70.0, 24.0], egui::Button::new("Cancel"))
                            .clicked()
                        {
                            self.show_manage_accounts_dialog = false;
                        }
                        if ui
                            .add_sized([56.0, 24.0], egui::Button::new("Help"))
                            .clicked()
                        {
                            self.status =
                                "Account management: select an account and an action on the right."
                                    .to_string();
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
                                        .spawn();
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
                        if !self.new_account_name.trim().is_empty() {
                            ui.separator();
                            ui.label("Skin Preview:");
                            ui.horizontal(|ui| {
                                let head_url =
                                    minecraft_head_icon_url(self.new_account_name.trim());
                                let body_url =
                                    minecraft_body_icon_url(self.new_account_name.trim());
                                if let Some(tex) =
                                    self.ensure_icon_texture_from_source(ui.ctx(), &head_url)
                                {
                                    ui.image((tex.id(), egui::vec2(48.0, 48.0)));
                                }
                                if let Some(tex) =
                                    self.ensure_icon_texture_from_source(ui.ctx(), &body_url)
                                {
                                    ui.image((tex.id(), egui::vec2(32.0, 64.0)));
                                }
                            });
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

    fn draw_launch_toast(&mut self, ctx: &egui::Context) {
        let Some(toast) = self.launch_toast.clone() else {
            return;
        };
        let lifetime = Duration::from_secs(6);
        let elapsed = toast.shown_at.elapsed();
        if elapsed > lifetime {
            self.launch_toast = None;
            return;
        }

        let t = (elapsed.as_secs_f32() / lifetime.as_secs_f32()).clamp(0.0, 1.0);
        let ease_out_cubic = |x: f32| 1.0 - (1.0 - x).powi(3);
        let ease_in_cubic = |x: f32| x.powi(3);
        let lerp = |a: f32, b: f32, x: f32| a + (b - a) * x;

        let in_end = 0.16;
        let out_start = 0.82;
        let base_x = -14.0;
        let offset_x = if t < in_end {
            let p = (t / in_end).clamp(0.0, 1.0);
            lerp(34.0, base_x, ease_out_cubic(p))
        } else if t > out_start {
            let p = ((t - out_start) / (1.0 - out_start)).clamp(0.0, 1.0);
            lerp(base_x, 34.0, ease_in_cubic(p))
        } else {
            base_x
        };
        let offset_y = 14.0;

        let fill = egui::Color32::from_rgb(29, 31, 36);
        let stroke = egui::Color32::from_rgb(84, 88, 97);
        let title_color = if toast.is_error {
            egui::Color32::from_rgb(232, 119, 119)
        } else {
            egui::Color32::from_rgb(235, 238, 245)
        };
        let subtitle_color = egui::Color32::from_rgb(178, 183, 194);

        egui::Area::new("launch_toast_area".into())
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(offset_x, offset_y))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(fill)
                    .stroke(egui::Stroke::new(1.0, stroke))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let icon_size = 36.0;
                            if let Some(icon_path) = toast.icon_path.as_ref() {
                                if let Some(tex) = self.ensure_icon_texture(ui.ctx(), icon_path) {
                                    ui.image((tex.id(), egui::vec2(icon_size, icon_size)));
                                } else if let Some(tex) =
                                    self.ensure_loader_icon_texture(ui.ctx(), &toast.loader)
                                {
                                    ui.image((tex.id(), egui::vec2(icon_size, icon_size)));
                                } else {
                                    ui.add_space(icon_size);
                                }
                            } else if let Some(tex) =
                                self.ensure_loader_icon_texture(ui.ctx(), &toast.loader)
                            {
                                ui.image((tex.id(), egui::vec2(icon_size, icon_size)));
                            } else {
                                ui.add_space(icon_size);
                            }
                            ui.vertical(|ui| {
                                let title_text = egui::RichText::new(toast.title.as_str())
                                    .size(22.0)
                                    .color(title_color);
                                ui.label(title_text);
                                ui.label(
                                    egui::RichText::new(toast.subtitle.as_str())
                                        .size(15.0)
                                        .color(subtitle_color)
                                        .weak(),
                                );
                            });
                        });
                    });
            });
    }
}

impl App for PrismarineApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        self.sync_process_states();
        self.poll_device_login_events();
        self.poll_account_avatar_events();
        self.poll_screenshot_thumbnail_events();
        self.poll_download_search_events();
        self.poll_download_details_events();
        self.poll_download_queue_events();
        self.poll_launch_worker_events();
        self.poll_import_worker_events();
        self.poll_create_versions();
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
            || !self.launch_in_progress.is_empty()
            || self.import_worker_rx.is_some()
            || self.launch_toast.is_some()
            || self.create_versions_loading
            || !self.screenshot_thumb_pending.is_empty()
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
                let selected = self.selected_instance().cloned();
                let has_selected = selected.is_some();
                let is_preparing = selected
                    .as_ref()
                    .map(|i| self.launch_in_progress.contains(&i.path))
                    .unwrap_or(false);
                if ui
                    .add_enabled(has_selected && !is_preparing, egui::Button::new("Launch"))
                    .clicked()
                {
                    self.do_launch_instance();
                }
                if ui
                    .add_enabled(has_selected && !is_preparing, egui::Button::new("Kill"))
                    .clicked()
                {
                    self.do_kill_instance();
                }
                if is_preparing {
                    ui.label("Preparing launch...");
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
            let panel_height = ui.available_height();
            let total_width = ui.available_width();
            let left_width = (total_width * 0.34).clamp(260.0, 420.0);
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(left_width, panel_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.heading("Instances");
                        ui.separator();
                        let queue_height = if self.download_jobs.is_empty() {
                            0.0
                        } else {
                            (panel_height * 0.28).clamp(130.0, 280.0)
                        };
                        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                            if queue_height > 0.0 {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(ui.available_width(), queue_height),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        self.draw_download_queue(ui);
                                    },
                                );
                                ui.separator();
                            }
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), ui.available_height().max(120.0)),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    self.draw_instance_list(ui, ctx);
                                },
                            );
                        });
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), panel_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.heading("Instance Details");
                        ui.separator();
                        self.tab_selector(ui);
                        match self.active_tab {
                            CenterTab::Overview => self.draw_overview_tab(ui),
                            CenterTab::Mods => self.draw_mods_tab(ui),
                            CenterTab::Logs => self.draw_logs_tab(ui),
                            CenterTab::Settings => self.draw_settings_tab(ui),
                        }
                    },
                );
            });
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Status:");
                ui.monospace(&self.status);
                if !self.launch_progress.is_empty() {
                    ui.separator();
                    if let Some(selected) = self.selected_instance()
                        && let Some(p) = self.launch_progress.get(&selected.path)
                    {
                        let total = p.total.max(1) as f32;
                        let value = (p.done as f32 / total).clamp(0.0, 1.0);
                        ui.label(format!("Download {}:", p.stage));
                        ui.add(egui::ProgressBar::new(value).desired_width(220.0));
                        ui.monospace(format!("{}/{}", p.done, p.total));
                    } else if let Some((_, p)) = self.launch_progress.iter().next() {
                        let total = p.total.max(1) as f32;
                        let value = (p.done as f32 / total).clamp(0.0, 1.0);
                        ui.label(format!("Download {}:", p.stage));
                        ui.add(egui::ProgressBar::new(value).desired_width(220.0));
                        ui.monospace(format!("{}/{}", p.done, p.total));
                    }
                } else if let Some(import) = &self.import_progress {
                    ui.separator();
                    let total = import.total.max(1) as f32;
                    let value = (import.done as f32 / total).clamp(0.0, 1.0);
                    ui.label(format!("Import {}:", import.stage));
                    ui.add(egui::ProgressBar::new(value).desired_width(220.0));
                    ui.monospace(format!("{}/{}", import.done, import.total));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.monospace(format!("Version: {}", launcher_version_string()));
                });
            });
        });

        self.draw_dialogs(ctx);
        self.draw_launch_toast(ctx);
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

fn suitable_default_max_mem_mb() -> u32 {
    let total_mb = {
        #[cfg(target_os = "linux")]
        {
            let text = fs::read_to_string("/proc/meminfo").unwrap_or_default();
            text.lines()
                .find_map(|line| {
                    let trimmed = line.trim();
                    if !trimmed.starts_with("MemTotal:") {
                        return None;
                    }
                    let kb = trimmed
                        .split_whitespace()
                        .nth(1)
                        .and_then(|x| x.parse::<u64>().ok())?;
                    Some((kb / 1024) as u32)
                })
                .unwrap_or(0)
        }
        #[cfg(not(target_os = "linux"))]
        {
            0
        }
    };
    if total_mb == 0 {
        return 4096;
    }
    if (total_mb as f32) < (4096.0 * 1.5) {
        ((total_mb as f32) / 1.5).round().max(512.0) as u32
    } else {
        4096
    }
}

fn upsert_jvm_system_property(args: &mut Vec<String>, key: &str, value: &str) {
    let prefix = format!("-D{key}=");
    if let Some(pos) = args.iter().position(|x| x.starts_with(&prefix)) {
        args[pos] = format!("{prefix}{value}");
    } else {
        args.push(format!("{prefix}{value}"));
    }
}

fn java_supports_permgen(java_path: &str) -> bool {
    let output = Command::new(java_path).arg("-version").output();
    let Ok(output) = output else {
        return false;
    };
    let mut text = String::from_utf8_lossy(&output.stderr).to_string();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stdout).to_string();
    }
    let Some(first_line) = text.lines().next() else {
        return false;
    };

    let Some(start) = first_line.find('"') else {
        return false;
    };
    let rest = &first_line[start + 1..];
    let Some(end_rel) = rest.find('"') else {
        return false;
    };
    let version = &rest[..end_rel];
    let major = if let Some(stripped) = version.strip_prefix("1.") {
        stripped
            .split('.')
            .next()
            .and_then(|x| x.parse::<u32>().ok())
            .unwrap_or(8)
    } else {
        version
            .split('.')
            .next()
            .and_then(|x| x.parse::<u32>().ok())
            .unwrap_or(17)
    };
    major <= 7
}

fn minecraft_head_icon_urls(name: &str) -> Vec<String> {
    let encoded = percent_encode_query(name);
    vec![
        // NameMC first (can be blocked by Cloudflare in some regions/environments).
        format!("https://namemc.com/avatar/{encoded}"),
        // Fallbacks for stable launcher-side rendering.
        format!("https://mc-heads.net/avatar/{encoded}/64"),
        format!("https://minotar.net/helm/{encoded}/64.png"),
    ]
}

fn minecraft_head_icon_url(name: &str) -> String {
    let encoded = percent_encode_query(name);
    format!("https://mc-heads.net/avatar/{encoded}/64")
}

fn minecraft_body_icon_url(name: &str) -> String {
    format!(
        "https://minotar.net/body/{}/64.png",
        percent_encode_query(name)
    )
}

#[derive(Clone, Debug, Default)]
struct ImportSummary {
    game_version: String,
    loader: Option<CreateLoader>,
}

fn guess_instance_name_from_archive_path(path: &Path) -> String {
    let file_name = path
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("Imported Instance");
    file_name
        .strip_suffix(".mrpack")
        .or_else(|| file_name.strip_suffix(".zip"))
        .unwrap_or(file_name)
        .trim()
        .to_string()
}

fn safe_zip_entry_target(base: &Path, entry_name: &str) -> Option<PathBuf> {
    let mut target = base.to_path_buf();
    for component in Path::new(entry_name).components() {
        match component {
            std::path::Component::Normal(part) => target.push(part),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(target)
}

fn extract_zip_file_to_dir_with_progress<F>(
    zip_path: &Path,
    destination: &Path,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(usize, usize, String),
{
    let file = fs::File::open(zip_path)
        .map_err(|e| format!("failed to open archive {}: {e}", zip_path.display()))?;
    let mut zip = ZipArchive::new(file)
        .map_err(|e| format!("failed to parse archive {}: {e}", zip_path.display()))?;
    let total = zip.len();
    for i in 0..total {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("failed to read archive entry #{i}: {e}"))?;
        let name = entry.name().to_string();
        on_progress(i + 1, total, format!("Extracting {name}"));
        let Some(target) = safe_zip_entry_target(destination, &name) else {
            continue;
        };
        if name.ends_with('/') {
            fs::create_dir_all(&target)
                .map_err(|e| format!("failed to create dir {}: {e}", target.display()))?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create dir {}: {e}", parent.display()))?;
        }
        let mut out = fs::File::create(&target)
            .map_err(|e| format!("failed to write file {}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("failed to extract file {}: {e}", target.display()))?;
    }
    Ok(())
}

fn import_zip_archive_with_progress<F>(
    zip_path: &Path,
    instance_path: &Path,
    on_progress: F,
) -> Result<ImportSummary, String>
where
    F: FnMut(usize, usize, String),
{
    extract_zip_file_to_dir_with_progress(zip_path, instance_path, on_progress)?;
    Ok(ImportSummary::default())
}

fn import_mrpack_archive_with_progress<F>(
    mrpack_path: &Path,
    instance_path: &Path,
    mut on_progress: F,
) -> Result<ImportSummary, String>
where
    F: FnMut(usize, usize, String),
{
    let file = fs::File::open(mrpack_path)
        .map_err(|e| format!("failed to open mrpack {}: {e}", mrpack_path.display()))?;
    let mut zip = ZipArchive::new(file)
        .map_err(|e| format!("failed to parse mrpack {}: {e}", mrpack_path.display()))?;

    let mut summary = ImportSummary::default();
    let mut index_json_text = None::<String>;
    let total = zip.len();
    for i in 0..total {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("failed to read mrpack entry #{i}: {e}"))?;
        let name = entry.name().to_string();
        on_progress(i + 1, total, format!("Extracting {name}"));
        if name == "modrinth.index.json" {
            let mut text = String::new();
            entry.read_to_string(&mut text).map_err(|e| {
                format!(
                    "failed to read modrinth.index.json from {}: {e}",
                    mrpack_path.display()
                )
            })?;
            index_json_text = Some(text);
            continue;
        }
        let target = if let Some(rest) = name.strip_prefix("overrides/") {
            safe_zip_entry_target(&instance_path.join("minecraft"), rest)
        } else if let Some(rest) = name.strip_prefix("client-overrides/") {
            safe_zip_entry_target(&instance_path.join("minecraft"), rest)
        } else {
            None
        };
        let Some(target) = target else {
            continue;
        };
        if name.ends_with('/') {
            let _ = fs::create_dir_all(&target);
            continue;
        }
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut out = fs::File::create(&target)
            .map_err(|e| format!("failed to write file {}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("failed to extract file {}: {e}", target.display()))?;
    }

    if let Some(text) = index_json_text {
        on_progress(
            total.max(1),
            total.max(1),
            "Parsing modrinth.index.json".to_string(),
        );
        let mrpack_dir = instance_path.join("mrpack");
        fs::create_dir_all(&mrpack_dir)
            .map_err(|e| format!("failed to create mrpack dir {}: {e}", mrpack_dir.display()))?;
        let index_path = mrpack_dir.join("modrinth.index.json");
        fs::write(&index_path, text.as_bytes())
            .map_err(|e| format!("failed to write {}: {e}", index_path.display()))?;

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(dep) = json.get("dependencies").and_then(|x| x.as_object())
        {
            summary.game_version = dep
                .get("minecraft")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            if dep.get("fabric-loader").is_some() {
                summary.loader = Some(CreateLoader::Fabric);
            } else if dep.get("quilt-loader").is_some() {
                summary.loader = Some(CreateLoader::Quilt);
            } else if dep.get("forge").is_some() {
                summary.loader = Some(CreateLoader::Forge);
            } else if dep.get("neoforge").is_some() {
                summary.loader = Some(CreateLoader::NeoForge);
            }
        }
    }

    Ok(summary)
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

fn all_existing_mod_dirs(instance_path: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for candidate in [
        instance_path.join("minecraft/mods"),
        instance_path.join(".minecraft/mods"),
        instance_path.join("mods"),
    ] {
        if candidate.is_dir() && !dirs.iter().any(|x: &PathBuf| x == &candidate) {
            dirs.push(candidate);
        }
    }
    if dirs.is_empty() {
        dirs.push(preferred_mods_dir(instance_path));
    }
    dirs
}

fn resolve_mod_file_path(instance_path: &Path, mod_file_name: &str) -> Option<PathBuf> {
    [
        instance_path.join("minecraft/mods").join(mod_file_name),
        instance_path.join(".minecraft/mods").join(mod_file_name),
        instance_path.join("mods").join(mod_file_name),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn read_zip_entry_text<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    path: &str,
) -> Option<String> {
    let mut file = zip.by_name(path).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    Some(text)
}

fn normalize_zip_entry_path(path: &str) -> String {
    path.trim()
        .trim_start_matches('/')
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn resolve_zip_entry_name<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    wanted: &str,
) -> Option<String> {
    let wanted_norm = normalize_zip_entry_path(wanted);
    if wanted_norm.is_empty() {
        return None;
    }
    if zip.by_name(&wanted_norm).is_ok() {
        return Some(wanted_norm);
    }
    for i in 0..zip.len() {
        let Ok(entry) = zip.by_index(i) else {
            continue;
        };
        let entry_name = entry.name().to_string();
        let entry_norm = normalize_zip_entry_path(&entry_name);
        if entry_norm == wanted_norm || entry_norm.ends_with(&format!("/{}", wanted_norm)) {
            return Some(entry_name);
        }
    }
    None
}

fn read_zip_entry_bytes_fuzzy<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    wanted: &str,
) -> Option<(String, Vec<u8>)> {
    let resolved = resolve_zip_entry_name(zip, wanted)?;
    let mut file = zip.by_name(&resolved).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some((resolved, bytes))
}

fn read_manifest_implementation_version<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> Option<String> {
    let text = read_zip_entry_text(zip, "META-INF/MANIFEST.MF")?;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed
            .to_ascii_lowercase()
            .starts_with("implementation-version:")
        {
            let value = trimmed
                .split_once(':')
                .map(|(_, v)| v.trim())
                .unwrap_or_default();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn parse_first_mods_toml_table(text: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut in_mods = false;
    let mut mod_id: Option<String> = None;
    let mut display_name: Option<String> = None;
    let mut version: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("[[") && line.ends_with("]]") {
            let section = line.trim_start_matches("[[").trim_end_matches("]]").trim();
            if section.eq_ignore_ascii_case("mods") {
                if in_mods {
                    break;
                }
                in_mods = true;
                continue;
            }
            if in_mods {
                break;
            }
            continue;
        }
        if !in_mods {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let mut value = v.trim();
        if let Some((before, _)) = value.split_once('#') {
            value = before.trim();
        }
        value = value.trim_matches('"').trim_matches('\'').trim();
        if value.is_empty() {
            continue;
        }
        match key {
            "modId" => mod_id = Some(value.to_string()),
            "displayName" => display_name = Some(value.to_string()),
            "version" => version = Some(value.to_string()),
            _ => {}
        }
    }
    (mod_id, display_name, version)
}

fn read_mods_toml_name_version<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    path: &str,
) -> Option<(String, String)> {
    let text = read_zip_entry_text(zip, path)?;
    let (mod_id, display_name, version) = parse_first_mods_toml_table(&text);
    let name = display_name.or(mod_id)?;
    let version = version.unwrap_or_default();
    Some((name, version))
}

fn read_mcmod_info_name_version<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> Option<(String, String)> {
    let text = read_zip_entry_text(zip, "mcmod.info")?;
    let json = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let obj = if let Some(arr) = json.as_array() {
        arr.first()?.as_object()?.clone()
    } else if let Some(root) = json.as_object() {
        if let Some(arr) = root.get("modlist").and_then(|v| v.as_array()) {
            arr.first()?.as_object()?.clone()
        } else if let Some(arr) = root.get("modList").and_then(|v| v.as_array()) {
            arr.first()?.as_object()?.clone()
        } else {
            return None;
        }
    } else {
        return None;
    };
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("modid").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    let version = obj
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((name, version))
}

fn read_mcmod_info_logo_path<R: Read + std::io::Seek>(zip: &mut ZipArchive<R>) -> Option<String> {
    let text = read_zip_entry_text(zip, "mcmod.info")?;
    let json = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let obj = if let Some(arr) = json.as_array() {
        arr.first()?.as_object()?.clone()
    } else if let Some(root) = json.as_object() {
        if let Some(arr) = root.get("modlist").and_then(|v| v.as_array()) {
            arr.first()?.as_object()?.clone()
        } else if let Some(arr) = root.get("modList").and_then(|v| v.as_array()) {
            arr.first()?.as_object()?.clone()
        } else {
            return None;
        }
    } else {
        return None;
    };
    let path = obj
        .get("logoFile")
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("logo").and_then(|v| v.as_str()))
        .unwrap_or("")
        .trim();
    if path.is_empty() {
        return None;
    }
    Some(path.to_string())
}

fn read_quilt_mod_name_version<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> Option<(String, String)> {
    let text = read_zip_entry_text(zip, "quilt.mod.json")?;
    let json = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let loader = json.get("quilt_loader")?;
    let mod_id = loader.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let version = loader
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = loader
        .get("metadata")
        .and_then(|m| m.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or(mod_id)
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    Some((name, version))
}

fn read_fabric_mod_name_version<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> Option<(String, String)> {
    let text = read_zip_entry_text(zip, "fabric.mod.json")?;
    let json = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let mod_id = json.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let version = json
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(mod_id)
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    Some((name, version))
}

fn read_forgeversion_properties_name_version<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> Option<(String, String)> {
    let text = read_zip_entry_text(zip, "forgeversion.properties")?;
    let mut name = None::<String>;
    let mut version = None::<String>;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let value = v.trim();
        if key.eq_ignore_ascii_case("name") {
            name = Some(value.to_string());
        } else if key.eq_ignore_ascii_case("version") || key.eq_ignore_ascii_case("revision") {
            version = Some(value.to_string());
        }
    }
    let name = name?;
    Some((name, version.unwrap_or_default()))
}

fn read_litemod_name_version<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
) -> Option<(String, String)> {
    let text = read_zip_entry_text(zip, "litemod.json")?;
    let json = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if name.is_empty() {
        return None;
    }
    let version = json
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((name, version))
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

fn compact_mod_version(version: &str) -> String {
    let mut v = version.trim().to_string();
    if v.is_empty() {
        return v;
    }
    v = v.replace("fabric.rev.", "f.rev.");
    v = v.replace("+build.", "+b.");
    v = v.replace("+mc", "+");
    v = v.replace("-release", "");
    let limit = 20usize;
    if v.chars().count() > limit {
        let mut out = String::new();
        for (i, ch) in v.chars().enumerate() {
            if i >= limit - 1 {
                break;
            }
            out.push(ch);
        }
        out.push('…');
        out
    } else {
        v
    }
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

fn preferred_shaderpacks_dir(instance_path: &Path) -> PathBuf {
    let candidates = [
        instance_path.join("minecraft/shaderpacks"),
        instance_path.join(".minecraft/shaderpacks"),
        instance_path.join("shaderpacks"),
    ];
    for candidate in candidates {
        if candidate.is_dir() {
            return candidate;
        }
    }
    let fallback = instance_path.join("minecraft/shaderpacks");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn preferred_worlds_dir(instance_path: &Path) -> PathBuf {
    for candidate in [
        instance_path.join("minecraft/saves"),
        instance_path.join(".minecraft/saves"),
        instance_path.join("saves"),
    ] {
        if candidate.is_dir() {
            return candidate;
        }
    }
    let fallback = instance_path.join("minecraft/saves");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn preferred_screenshots_dir(instance_path: &Path) -> PathBuf {
    for candidate in [
        instance_path.join("minecraft/screenshots"),
        instance_path.join(".minecraft/screenshots"),
        instance_path.join("screenshots"),
    ] {
        if candidate.is_dir() {
            return candidate;
        }
    }
    let fallback = instance_path.join("minecraft/screenshots");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn preferred_download_dir(instance_path: &Path, content_type: &DownloadContentType) -> PathBuf {
    match content_type {
        DownloadContentType::Mods => preferred_mods_dir(instance_path),
        DownloadContentType::ResourcePacks => preferred_resourcepacks_dir(instance_path),
        DownloadContentType::ShaderPacks => preferred_shaderpacks_dir(instance_path),
        DownloadContentType::Worlds => preferred_worlds_dir(instance_path),
        DownloadContentType::Servers => instance_path.join("minecraft"),
        DownloadContentType::Screenshots => preferred_screenshots_dir(instance_path),
    }
}

fn load_state() -> Option<PersistedState> {
    let text = fs::read_to_string(state_file_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn load_instance_groups() -> Vec<InstanceGroupMeta> {
    let path = groups_file_path();
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<InstanceGroupMeta>>(&text).unwrap_or_default()
}

fn save_instance_groups(groups: &[InstanceGroupMeta]) {
    let path = groups_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(groups) {
        let _ = fs::write(path, text);
    }
}

fn save_instance_launch_overrides(
    instance_path: &Path,
    cfg: &PrismInstanceConfig,
) -> std::io::Result<()> {
    set_instance_cfg_value(
        instance_path,
        "OverrideJavaLocation",
        Some(if cfg.override_java_location {
            "true"
        } else {
            "false"
        }),
    )?;
    set_instance_cfg_value(instance_path, "JavaPath", cfg.java_path.as_deref())?;
    set_instance_cfg_value(
        instance_path,
        "OverrideJavaArgs",
        Some(if cfg.override_java_args {
            "true"
        } else {
            "false"
        }),
    )?;
    set_instance_cfg_value(instance_path, "JvmArgs", cfg.java_args.as_deref())?;
    set_instance_cfg_value(
        instance_path,
        "OverrideMemory",
        Some(if cfg.override_memory { "true" } else { "false" }),
    )?;
    set_instance_cfg_value(
        instance_path,
        "MinMemAlloc",
        cfg.min_mem_alloc.map(|x| x.to_string()).as_deref(),
    )?;
    set_instance_cfg_value(
        instance_path,
        "MaxMemAlloc",
        cfg.max_mem_alloc.map(|x| x.to_string()).as_deref(),
    )?;
    set_instance_cfg_value(
        instance_path,
        "PermGen",
        cfg.perm_gen.map(|x| x.to_string()).as_deref(),
    )?;
    set_instance_cfg_value(
        instance_path,
        "OverrideCommands",
        Some(if cfg.override_commands {
            "true"
        } else {
            "false"
        }),
    )?;
    set_instance_cfg_value(
        instance_path,
        "PreLaunchCommand",
        cfg.pre_launch_command.as_deref(),
    )?;
    set_instance_cfg_value(
        instance_path,
        "PostExitCommand",
        cfg.post_exit_command.as_deref(),
    )?;
    set_instance_cfg_value(
        instance_path,
        "WrapperCommand",
        cfg.wrapper_command.as_deref(),
    )?;
    Ok(())
}

fn set_instance_cfg_value(
    instance_path: &Path,
    key: &str,
    value: Option<&str>,
) -> std::io::Result<()> {
    let cfg_path = instance_path.join("instance.cfg");
    let existing = fs::read_to_string(&cfg_path)
        .unwrap_or_else(|_| "# PrismarineLauncher instance\n".to_string());
    let mut out = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if let Some((k, _)) = trimmed.split_once('=')
            && k.trim().eq_ignore_ascii_case(key)
        {
            if let Some(v) = value {
                out.push(format!("{key}={v}"));
            }
            replaced = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !replaced && let Some(v) = value {
        out.push(format!("{key}={v}"));
    }
    let mut text = out.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    fs::write(cfg_path, text)
}

fn sanitize_key_fragment(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() {
            out.push('_');
        }
    }
    if out.is_empty() {
        "icon".to_string()
    } else {
        out
    }
}

#[allow(dead_code)]
fn copy_image_to_icon_store(data_root: &Path, source: &Path, base_name: &str) -> Option<String> {
    let ext = source.extension().and_then(|x| x.to_str()).unwrap_or("png");
    let key = format!(
        "{}_{}",
        sanitize_key_fragment(base_name),
        chrono::Local::now().timestamp()
    );
    let icons_dir = data_root.join("icons");
    let _ = fs::create_dir_all(&icons_dir);
    let dest = icons_dir.join(format!("{key}.{ext}"));
    fs::copy(source, dest).ok()?;
    Some(key)
}

fn copy_image_to_group_store(data_root: &Path, source: &Path, group_name: &str) -> Option<String> {
    let ext = source.extension().and_then(|x| x.to_str()).unwrap_or("png");
    let file_name = format!(
        "{}_{}.{}",
        sanitize_key_fragment(group_name),
        chrono::Local::now().timestamp(),
        ext
    );
    let dir = data_root.join("group_icons");
    let _ = fs::create_dir_all(&dir);
    let dest = dir.join(file_name);
    fs::copy(source, &dest).ok()?;
    Some(dest.display().to_string())
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

fn decode_svg_rgba(bytes: &[u8]) -> Option<(Vec<u8>, usize, usize)> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).ok()?;
    let size = tree.size().to_int_size();
    let width = size.width() as usize;
    let height = size.height() as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width as u32, height as u32)?;
    let mut pixmap_mut = pixmap.as_mut();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::default(),
        &mut pixmap_mut,
    );
    Some((pixmap.data().to_vec(), width, height))
}

fn extract_html_img_srcs(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    loop {
        let Some(img_pos) = rest.find("<img") else {
            break;
        };
        let after_img = &rest[img_pos..];
        let Some(src_pos) = after_img.find("src=") else {
            rest = &after_img[4..];
            continue;
        };
        let after_src = &after_img[src_pos + 4..];
        if let Some(stripped) = after_src.strip_prefix('"') {
            if let Some(end) = stripped.find('"') {
                let url = stripped[..end].trim();
                if !url.is_empty() {
                    out.push(url.to_string());
                }
                rest = &stripped[end + 1..];
                continue;
            }
        } else if let Some(stripped) = after_src.strip_prefix('\'')
            && let Some(end) = stripped.find('\'')
        {
            let url = stripped[..end].trim();
            if !url.is_empty() {
                out.push(url.to_string());
            }
            rest = &stripped[end + 1..];
            continue;
        }
        rest = &after_src[1..];
    }
    out
}

fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let keep = max_chars - 1;
    let mut out = String::with_capacity(max_chars);
    for ch in text.chars().take(keep) {
        out.push(ch);
    }
    out.push('…');
    out
}

fn html_line_has_center_image_hint(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("<center>")
        || lower.contains("</center>")
        || lower.contains("align=\"center\"")
        || lower.contains("align='center'")
}

fn normalize_markdown_html_images(input: &str) -> String {
    let mut out = String::new();
    for line in input.lines() {
        let trimmed = line.trim();
        let image_urls = extract_html_img_srcs(trimmed);
        if !image_urls.is_empty() {
            let _centered = html_line_has_center_image_hint(trimmed);
            for url in image_urls {
                // Do not use markdown tables for centering: table borders create visual artifacts.
                out.push_str(&format!("![]({url})\n\n"));
            }
            continue;
        }
        let cleaned = line
            .replace("<center>", "")
            .replace("</center>", "")
            .replace("<p align=\"center\">", "")
            .replace("<p align='center'>", "")
            .replace("</p>", "");
        out.push_str(&cleaned);
        out.push('\n');
    }
    out
}

fn extract_ascii_runs(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    for &b in bytes {
        if (0x20..=0x7e).contains(&b) {
            cur.push(b);
        } else if cur.len() >= 2 {
            if let Ok(s) = String::from_utf8(cur.clone()) {
                out.push(s);
            }
            cur.clear();
        } else {
            cur.clear();
        }
    }
    if cur.len() >= 2
        && let Ok(s) = String::from_utf8(cur)
    {
        out.push(s);
    }
    out
}

fn looks_like_server_address(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.len() > 255 {
        return false;
    }
    t.contains('.') || t.contains(':') || t.eq_ignore_ascii_case("localhost")
}

fn parse_servers_dat_loose(path: &Path) -> Vec<(String, String)> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let tokens = extract_ascii_runs(&bytes);
    if tokens.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut pending_name = String::new();
    let keys = [
        "name",
        "ip",
        "icon",
        "acceptTextures",
        "servers",
        "hidden",
        "enabled",
    ];
    let mut i = 0usize;
    while i + 1 < tokens.len() {
        let key = tokens[i].trim();
        let val = tokens[i + 1].trim();
        if key.eq_ignore_ascii_case("name") && !val.is_empty() {
            pending_name = val.to_string();
            i += 2;
            continue;
        }
        if key.eq_ignore_ascii_case("ip") && looks_like_server_address(val) {
            let display = if pending_name.trim().is_empty() {
                "Minecraft Server".to_string()
            } else {
                pending_name.clone()
            };
            out.push((display, val.to_string()));
            pending_name.clear();
            i += 2;
            continue;
        }
        if looks_like_server_address(key) {
            let prev = if i > 0 { tokens[i - 1].trim() } else { "" };
            let display = if prev.is_empty() || keys.iter().any(|k| prev.eq_ignore_ascii_case(k)) {
                "Minecraft Server".to_string()
            } else {
                prev.to_string()
            };
            out.push((display, key.to_string()));
        }
        i += 1;
    }
    out.sort();
    out.dedup();
    out
}

fn json_text_compact(value: &serde_json::Value) -> String {
    if let Some(s) = value.as_str() {
        return s.to_string();
    }
    if let Some(obj) = value.as_object() {
        if let Some(text) = obj.get("text") {
            let t = json_text_compact(text);
            if !t.trim().is_empty() {
                return t;
            }
        }
        if let Some(extra) = obj.get("extra")
            && let Some(arr) = extra.as_array()
        {
            let merged = arr
                .iter()
                .map(json_text_compact)
                .collect::<Vec<_>>()
                .join("");
            if !merged.trim().is_empty() {
                return merged;
            }
        }
    }
    if let Some(arr) = value.as_array() {
        return arr
            .iter()
            .map(json_text_compact)
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
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
        b & 0x0000_FFFF_FFFF_FFFF
    )
}

fn normalize_minecraft_uuid(raw: &str) -> String {
    let compact: String = raw.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if compact.len() == 32 {
        format!(
            "{}-{}-{}-{}-{}",
            &compact[0..8],
            &compact[8..12],
            &compact[12..16],
            &compact[16..20],
            &compact[20..32]
        )
    } else {
        raw.to_string()
    }
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
