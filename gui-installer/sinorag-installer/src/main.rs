#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod app {
    use sinorag_installer::win_utils::{
        default_install_path, desktop_shortcut_path, start_menu_shortcut_path, to_wide,
        to_wide_str, try_single_instance, write_uninstall_entry,
    };
    use anyhow::{anyhow, Context, Result};
    use freya::prelude::*;
    use rfd::AsyncFileDialog;
    use sinorag::commands::init::{self, InitProgressEvent};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitialize, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    const PAYLOAD_7Z: &[u8] = include_bytes!(env!("SINORAG_PAYLOAD_PATH"));
    const WINDOW_ICON: &[u8] = include_bytes!(env!("SINORAG_ICON_PNG"));
    const SHORTCUT_ICON: &[u8] = include_bytes!(env!("SINORAG_ICON_ICO"));
    /// SinoRAG version bundled in the payload (baked in by build.rs from the
    /// workspace manifest). Used to detect an up-to-date existing install.
    const EXPECTED_SINORAG_VERSION: &str = env!("EXPECTED_SINORAG_VERSION");

    const INK: (u8, u8, u8) = (18, 50, 53);
    const JADE: (u8, u8, u8) = (31, 74, 67);
    const CINNABAR: (u8, u8, u8) = (154, 73, 55);
    const CINNABAR_HOVER: (u8, u8, u8) = (175, 91, 69);
    const BRONZE: (u8, u8, u8) = (197, 154, 61);
    const PAPER: (u8, u8, u8) = (247, 244, 234);
    const PANEL_BG: (u8, u8, u8) = (236, 231, 216);
    const TEXT: (u8, u8, u8) = (23, 36, 38);
    const MUTED: (u8, u8, u8) = (93, 104, 102);
    const BORDER: (u8, u8, u8) = (138, 124, 99);
    const WARNING: (u8, u8, u8) = (166, 107, 24);
    const ERROR: (u8, u8, u8) = (173, 47, 38);

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Phase {
        Ready,
        Installing,
        Complete,
        Failed,
    }

    #[derive(Clone, Debug, PartialEq)]
    struct InstallerState {
        phase: Phase,
        progress: f32,
        step: String,
        logs: Vec<String>,
        install_path: String,
        opencode_missing: bool,
        already_installed: bool,
    }

    impl Default for InstallerState {
        fn default() -> Self {
            Self {
                phase: Phase::Ready,
                progress: 0.0,
                step: "Ready to install SinoRAG.".to_string(),
                logs: vec!["Ready to install SinoRAG.".to_string()],
                install_path: String::new(),
                opencode_missing: false,
                already_installed: false,
            }
        }
    }

    #[derive(Clone, Debug)]
    struct InstallRequest {
        install_path: PathBuf,
        desktop_shortcut: bool,
        start_menu_shortcut: bool,
    }

    #[derive(Debug)]
    enum InstallerEvent {
        Step(String, f32),
        Log(String),
        OpenCodeMissing,
        AlreadyInstalled,
        Done(Result<(), String>),
    }

    pub fn run() {
        let _mutex = match try_single_instance() {
            Some(guard) => guard,
            None => {
                eprintln!("SinoRAG Installer is already running.");
                return;
            }
        };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create installer runtime");
        let _guard = runtime.enter();

        launch(
            LaunchConfig::new().with_window(
                WindowConfig::new(installer_app)
                    .with_title("SinoRAG Installer")
                    .with_size(500., 640.)
                    .with_min_size(500., 640.)
                    .with_max_size(500., 640.)
                    .with_resizable(false)
                    .with_icon(LaunchConfig::window_icon(WINDOW_ICON)),
            ),
        );
    }

    fn sinorag_theme() -> Theme {
        let mut theme = light_theme();
        theme.name = "sinorag";
        theme.colors = ColorsSheet {
            primary: Color::from_rgb(CINNABAR.0, CINNABAR.1, CINNABAR.2),
            secondary: Color::from_rgb(BRONZE.0, BRONZE.1, BRONZE.2),
            tertiary: Color::from_rgb(CINNABAR_HOVER.0, CINNABAR_HOVER.1, CINNABAR_HOVER.2),
            success: Color::from_rgb(JADE.0, JADE.1, JADE.2),
            warning: Color::from_rgb(WARNING.0, WARNING.1, WARNING.2),
            error: Color::from_rgb(ERROR.0, ERROR.1, ERROR.2),
            info: Color::from_rgb(JADE.0, JADE.1, JADE.2),
            background: Color::from_rgb(PANEL_BG.0, PANEL_BG.1, PANEL_BG.2),
            surface_primary: Color::from_rgb(226, 219, 201),
            surface_secondary: Color::from_rgb(240, 235, 222),
            surface_tertiary: Color::from_rgb(PAPER.0, PAPER.1, PAPER.2),
            surface_inverse: Color::from_rgb(138, 124, 99),
            surface_inverse_secondary: Color::from_rgb(115, 102, 79),
            surface_inverse_tertiary: Color::from_rgb(92, 82, 64),
            border: Color::from_rgb(BORDER.0, BORDER.1, BORDER.2),
            border_focus: Color::from_rgb(JADE.0, JADE.1, JADE.2),
            border_disabled: Color::from_rgb(198, 188, 164),
            text_primary: Color::from_rgb(TEXT.0, TEXT.1, TEXT.2),
            text_secondary: Color::from_rgb(MUTED.0, MUTED.1, MUTED.2),
            text_placeholder: Color::from_rgb(128, 121, 102),
            text_inverse: Color::from_rgb(255, 249, 238),
            text_highlight: Color::from_rgb(CINNABAR.0, CINNABAR.1, CINNABAR.2),
            focus: Color::from_rgb(236, 227, 200),
            active: Color::from_rgb(219, 209, 188),
            disabled: Color::from_rgb(216, 207, 185),
            overlay: Color::from_af32rgb(0.35, 18, 50, 53),
            shadow: Color::from_af32rgb(0.18, 18, 50, 53),
        };
        theme
    }

    fn installer_app() -> impl IntoElement {
        use_init_theme(sinorag_theme);
        let state = use_state(InstallerState::default);
        let install_path = use_state(default_install_path_str);
        let desktop_shortcut = use_state(|| true);
        let start_menu_shortcut = use_state(|| true);

        let read = state.read().clone();
        let is_installing = read.phase == Phase::Installing;

        rect().expanded().padding(8.).background(PANEL_BG).child(
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .background(PAPER)
                .border(
                    Border::new()
                        .fill(BORDER)
                        .width(1.)
                        .alignment(BorderAlignment::Inner),
                )
                .corner_radius(8.)
                .padding(14.)
                .spacing(9.)
                .content(Content::Flex)
                .child(header())
                .child(field_label("Install path", is_installing))
                .child(
                    rect()
                        .direction(Direction::Horizontal)
                        .width(Size::fill())
                        .height(Size::px(42.))
                        .content(Content::Flex)
                        .spacing(6.)
                        .child(
                            rect().width(Size::flex(1.)).height(Size::fill()).child(
                                Input::new(install_path)
                                    .enabled(!is_installing)
                                    .width(Size::fill())
                                    .placeholder("Install path"),
                            ),
                        )
                        .child(path_picker_button(install_path, is_installing)),
                )
                .child(option_row(
                    "Create desktop shortcut",
                    desktop_shortcut,
                    is_installing,
                ))
                .child(option_row(
                    "Create Start Menu shortcut",
                    start_menu_shortcut,
                    is_installing,
                ))
                .child(progress_card(&read))
                .child(
                    rect()
                        .width(Size::fill())
                        .height(Size::flex(1.))
                        .child(log_card(&read.logs)),
                )
                .child(
                    rect()
                        .width(Size::fill())
                        .height(Size::px(44.))
                        .child(action_button(
                            &read,
                            state,
                            install_path,
                            desktop_shortcut,
                            start_menu_shortcut,
                        )),
                ),
        )
    }

    fn header() -> Element {
        rect()
            .spacing(4.)
            .child(
                label()
                    .text("SinoRAG")
                    .font_size(38.)
                    .font_weight(FontWeight::BOLD)
                    .color(INK),
            )
            .child(
                label()
                    .text("Buddhist corpus research engine")
                    .font_size(14.)
                    .color(MUTED),
            )
            .into()
    }

    fn field_label(text: &'static str, disabled: bool) -> Element {
        label()
            .text(text)
            .font_size(13.)
            .font_weight(FontWeight::MEDIUM)
            .color(if disabled { MUTED } else { TEXT })
            .into()
    }

    fn path_picker_button(mut install_path: State<String>, disabled: bool) -> Element {
        Button::new()
            .enabled(!disabled)
            .width(Size::px(42.))
            .height(Size::fill())
            .on_press(move |_| {
                spawn(async move {
                    if let Some(folder) = AsyncFileDialog::new().pick_folder().await {
                        install_path.set(folder.path().display().to_string());
                    }
                });
            })
            .child("...")
            .into()
    }

    fn option_row(text: &'static str, mut selected: State<bool>, disabled: bool) -> Element {
        Tile::new()
            .on_select(move |_| {
                if !disabled {
                    selected.toggle();
                }
            })
            .leading(text)
            .child(Checkbox::new().selected(selected()).size(18.))
            .into()
    }

    fn progress_card(read: &InstallerState) -> Element {
        rect()
            .width(Size::fill())
            .background(PAPER)
            .border(
                Border::new()
                    .fill(BORDER)
                    .width(1.)
                    .alignment(BorderAlignment::Inner),
            )
            .corner_radius(5.)
            .padding(10.)
            .spacing(8.)
            .child(
                label()
                    .text(read.step.clone())
                    .font_size(14.)
                    .font_weight(FontWeight::MEDIUM)
                    .color(TEXT),
            )
            .child(ProgressBar::new((read.progress * 100.0).clamp(0.0, 100.0)).show_progress(true))
            .into()
    }

    fn log_card(logs: &[String]) -> Element {
        let visible: Vec<Element> = logs
            .iter()
            .map(|line| {
                label()
                    .text(line.clone())
                    .font_size(12.)
                    .color(log_color(line))
                    .max_lines(1)
                    .into()
            })
            .collect();

        rect()
            .width(Size::fill())
            .height(Size::fill())
            .background(PAPER)
            .border(
                Border::new()
                    .fill(BORDER)
                    .width(1.)
                    .alignment(BorderAlignment::Inner),
            )
            .corner_radius(5.)
            .padding(10.)
            .child(
                ScrollView::new().child(rect().width(Size::fill()).spacing(4.).children(visible)),
            )
            .into()
    }

    fn log_color(line: &str) -> (u8, u8, u8) {
        let lower = line.to_ascii_lowercase();
        if lower.contains("failed") || lower.contains("error") {
            ERROR
        } else if lower.contains("warning") || lower.contains("warn") {
            WARNING
        } else {
            MUTED
        }
    }

    fn action_button(
        read: &InstallerState,
        state: State<InstallerState>,
        install_path: State<String>,
        desktop_shortcut: State<bool>,
        start_menu_shortcut: State<bool>,
    ) -> Element {
        let label_text = match read.phase {
            Phase::Ready => "Install",
            Phase::Installing => "Installing",
            Phase::Complete => "Open install folder",
            Phase::Failed => "Retry install",
        };
        let phase = read.phase;
        Button::new()
            .filled()
            .enabled(phase != Phase::Installing)
            .on_press(move |_| {
                if phase == Phase::Complete {
                    open_install_folder(&install_path.read());
                } else {
                    start_install(state, install_path, desktop_shortcut, start_menu_shortcut);
                }
            })
            .width(Size::fill())
            .height(Size::px(44.))
            .child(label_text)
            .into()
    }

    fn start_install(
        mut state: State<InstallerState>,
        install_path: State<String>,
        desktop_shortcut: State<bool>,
        start_menu_shortcut: State<bool>,
    ) {
        if state.read().phase == Phase::Installing {
            return;
        }
        let install_path_text = install_path.read().trim().to_string();
        let install_path = match validate_install_path(&install_path_text) {
            Ok(path) => path,
            Err(error) => {
                let mut s = state.write();
                s.phase = Phase::Failed;
                s.step = "Install path is not valid.".to_string();
                push_log(&mut s.logs, format!("Error: {error}"));
                return;
            }
        };
        let request = InstallRequest {
            install_path,
            desktop_shortcut: desktop_shortcut(),
            start_menu_shortcut: start_menu_shortcut(),
        };
        {
            let mut s = state.write();
            s.phase = Phase::Installing;
            s.progress = 0.0;
            s.step = "Starting installation".to_string();
            s.install_path = request.install_path.display().to_string();
            s.opencode_missing = false;
            s.already_installed = false;
            s.logs.clear();
            s.logs
                .push(format!("Target path: {}", request.install_path.display()));
        }

        let (tx, rx) = flume::unbounded::<InstallerEvent>();
        thread::spawn(move || {
            let result = install_impl(request, tx.clone()).map_err(|e| e.to_string());
            let _ = tx.send(InstallerEvent::Done(result));
        });

        spawn(async move {
            while let Ok(event) = rx.recv_async().await {
                let done = matches!(event, InstallerEvent::Done(_));
                apply_event(&mut state, event);
                if done {
                    break;
                }
            }
        });
    }

    fn apply_event(state: &mut State<InstallerState>, event: InstallerEvent) {
        let mut s = state.write();
        match event {
            InstallerEvent::Step(step, progress) => {
                s.step = step.clone();
                s.progress = progress.clamp(0.0, 1.0);
                push_log(&mut s.logs, step);
            }
            InstallerEvent::Log(line) => push_log(&mut s.logs, line),
            InstallerEvent::OpenCodeMissing => {
                s.opencode_missing = true;
                push_log(
                    &mut s.logs,
                    "Warning: OpenCode was not found after install attempts.".to_string(),
                );
            }
            InstallerEvent::AlreadyInstalled => {
                s.already_installed = true;
            }
            InstallerEvent::Done(Ok(())) => {
                s.phase = Phase::Complete;
                s.progress = 1.0;
                if s.already_installed {
                    s.step = "SinoRAG is already installed — nothing to do.".to_string();
                    push_log(&mut s.logs, "Already installed; skipped.".to_string());
                } else {
                    s.step = if s.opencode_missing {
                        "Installed. OpenCode was not found; see log.".to_string()
                    } else {
                        "SinoRAG installed successfully.".to_string()
                    };
                    push_log(&mut s.logs, "Installation complete.".to_string());
                }
            }
            InstallerEvent::Done(Err(error)) => {
                s.phase = Phase::Failed;
                s.step = "Install failed.".to_string();
                push_log(&mut s.logs, format!("Install failed: {error}"));
            }
        }
    }

    fn push_log(logs: &mut Vec<String>, line: String) {
        if logs.last().map(|last| last == &line).unwrap_or(false) {
            return;
        }
        logs.push(line);
        if logs.len() > 100 {
            logs.remove(0);
        }
    }

    fn install_impl(request: InstallRequest, tx: flume::Sender<InstallerEvent>) -> Result<()> {
        // Fail-fast: if a matching, complete install is already present, report it
        // and skip the work rather than redownloading the corpus and rebuilding.
        step(&tx, "Checking for an existing installation", 0.01);
        if let Some(summary) = existing_install_summary(&request) {
            log(&tx, summary);
            let _ = tx.send(InstallerEvent::AlreadyInstalled);
            return Ok(());
        }

        step(&tx, "Creating install directory", 0.03);
        fs::create_dir_all(&request.install_path)
            .with_context(|| format!("creating {}", request.install_path.display()))?;

        step(&tx, "Extracting SinoRAG executable", 0.06);
        extract_payload_archive(&request.install_path).with_context(|| {
            format!("extracting payload into {}", request.install_path.display())
        })?;
        let exe_path = request.install_path.join("sinorag.exe");
        if !exe_path.is_file() {
            return Err(anyhow!(
                "payload archive did not produce {}",
                exe_path.display()
            ));
        }
        let icon_path = request.install_path.join("SinoRAG.ico");
        fs::write(&icon_path, SHORTCUT_ICON)
            .with_context(|| format!("writing {}", icon_path.display()))?;

        let data_root = request.install_path.join("data");
        let out_parquet = data_root.join("passages.parquet");
        let pack_url =
            init::PACK_URL.ok_or_else(|| anyhow!("SinoRAG pack URL is not configured"))?;
        let progress_tx = tx.clone();
        let last_download_report = Arc::new(AtomicU64::new(0));
        let callback = move |event: InitProgressEvent| match event {
            InitProgressEvent::Step { label, progress } => {
                let mapped = 0.10 + progress.unwrap_or(0.0) * 0.76;
                let _ = progress_tx.send(InstallerEvent::Step(label, mapped));
            }
            InitProgressEvent::Download { received, total } => {
                let last = last_download_report.load(Ordering::Relaxed);
                if received.saturating_sub(last) < 512 * 1024
                    && total.map(|total| received < total).unwrap_or(true)
                {
                    return;
                }
                last_download_report.store(received, Ordering::Relaxed);
                let percent = total
                    .filter(|total| *total > 0)
                    .map(|total| received as f32 / total as f32)
                    .unwrap_or(0.0);
                let mb = received as f64 / 1024.0 / 1024.0;
                let label = if let Some(total) = total {
                    format!(
                        "Downloading corpus pack: {:.1} / {:.1} MB",
                        mb,
                        total as f64 / 1024.0 / 1024.0
                    )
                } else {
                    format!("Downloading corpus pack: {:.1} MB", mb)
                };
                let _ = progress_tx.send(InstallerEvent::Step(label, 0.10 + percent * 0.25));
            }
            InitProgressEvent::Log(line) => {
                let _ = progress_tx.send(InstallerEvent::Log(line));
            }
            InitProgressEvent::Done(_) => {}
        };
        init::run_from_pack_url_blocking(pack_url, false, data_root, out_parquet, Some(&callback))?;

        install_or_verify_opencode(&tx)?;
        create_shortcuts(&request, &exe_path, &icon_path, &tx)?;

        step(&tx, "Registering uninstaller", 0.99);
        write_uninstall_entry(&request.install_path, EXPECTED_SINORAG_VERSION);

        step(&tx, "SinoRAG is ready.", 1.0);
        Ok(())
    }

    fn install_or_verify_opencode(tx: &flume::Sender<InstallerEvent>) -> Result<()> {
        step(tx, "Checking OpenCode", 0.88);
        if command_ok("opencode", &["--version"]) {
            log(tx, "OpenCode is already installed.");
            return Ok(());
        }

        if which("bash.exe").is_some() {
            step(tx, "Installing OpenCode with official installer", 0.91);
            let status = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    "curl.exe -fsSL https://opencode.ai/install | bash",
                ])
                .status()
                .context("running OpenCode official installer")?;
            if status.success() && command_ok("opencode", &["--version"]) {
                log(tx, "OpenCode installed.");
                return Ok(());
            }
            log(
                tx,
                format!("OpenCode official installer exited with {status}; trying npm."),
            );
        }

        if which("npm.cmd").is_some() || which("npm.exe").is_some() || which("npm").is_some() {
            step(tx, "Installing OpenCode with npm", 0.93);
            let status = Command::new("npm")
                .args(["install", "-g", "opencode-ai"])
                .status()
                .context("running npm install -g opencode-ai")?;
            if status.success() && command_ok("opencode", &["--version"]) {
                log(tx, "OpenCode installed.");
                return Ok(());
            }
            log(tx, format!("npm OpenCode install exited with {status}."));
        }

        log(
            tx,
            "Warning: OpenCode was not found after install attempts. Run `opencode auth login` after installing OpenCode.",
        );
        let _ = tx.send(InstallerEvent::OpenCodeMissing);
        Ok(())
    }

    fn extract_payload_archive(install_path: &Path) -> Result<()> {
        let archive_path = install_path.join(".sinorag-payload.7z");
        fs::write(&archive_path, PAYLOAD_7Z)
            .with_context(|| format!("writing temporary payload {}", archive_path.display()))?;
        let result = sevenz_rust::decompress_file(&archive_path, install_path)
            .map_err(|error| anyhow!("failed to extract payload archive: {error}"));
        let _ = fs::remove_file(&archive_path);
        result
    }

    fn create_shortcuts(
        request: &InstallRequest,
        exe_path: &Path,
        icon_path: &Path,
        tx: &flume::Sender<InstallerEvent>,
    ) -> Result<()> {
        step(tx, "Creating shortcuts", 0.97);
        let co_init_ok = unsafe { CoInitialize(None).is_ok() };

        let result = create_shortcuts_inner(request, exe_path, icon_path, tx);

        if co_init_ok {
            unsafe { CoUninitialize(); }
        }

        result
    }

    fn create_shortcuts_inner(
        request: &InstallRequest,
        exe_path: &Path,
        icon_path: &Path,
        tx: &flume::Sender<InstallerEvent>,
    ) -> Result<()> {
        if request.desktop_shortcut {
            if let Some(path) = desktop_shortcut_path() {
                create_windows_shortcut(exe_path, "agent", &path, &request.install_path, icon_path)
                    .with_context(|| format!("creating {}", path.display()))?;
                log(tx, format!("Desktop shortcut: {}", path.display()));
            }
        }

        if request.start_menu_shortcut {
            if let Some(path) = start_menu_shortcut_path() {
                if let Some(folder) = path.parent() {
                    fs::create_dir_all(folder)
                        .with_context(|| format!("creating {}", folder.display()))?;
                }
                create_windows_shortcut(exe_path, "agent", &path, &request.install_path, icon_path)
                    .with_context(|| format!("creating {}", path.display()))?;
                log(tx, format!("Start Menu shortcut: {}", path.display()));
            }
        }
        Ok(())
    }

    fn create_windows_shortcut(
        target: &Path,
        arguments: &str,
        shortcut_path: &Path,
        working_dir: &Path,
        icon_path: &Path,
    ) -> Result<()> {
        unsafe {
            let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
                .map_err(|error| anyhow!("creating shell link: {error:?}"))?;

            let target_wide = to_wide(target);
            shell_link
                .SetPath(PCWSTR::from_raw(target_wide.as_ptr()))
                .map_err(|error| anyhow!("setting shortcut target: {error:?}"))?;

            let args_wide = to_wide_str(arguments);
            shell_link
                .SetArguments(PCWSTR::from_raw(args_wide.as_ptr()))
                .map_err(|error| anyhow!("setting shortcut arguments: {error:?}"))?;

            let working_wide = to_wide(working_dir);
            shell_link
                .SetWorkingDirectory(PCWSTR::from_raw(working_wide.as_ptr()))
                .map_err(|error| anyhow!("setting shortcut working dir: {error:?}"))?;

            let icon_wide = to_wide(icon_path);
            shell_link
                .SetIconLocation(PCWSTR::from_raw(icon_wide.as_ptr()), 0)
                .map_err(|error| anyhow!("setting shortcut icon: {error:?}"))?;

            let persist_file: IPersistFile = shell_link
                .cast()
                .map_err(|error| anyhow!("casting shortcut to IPersistFile: {error:?}"))?;

            let shortcut_wide = to_wide(shortcut_path);
            persist_file
                .Save(PCWSTR::from_raw(shortcut_wide.as_ptr()), true)
                .map_err(|error| anyhow!("saving shortcut: {error:?}"))?;
        }
        Ok(())
    }

    fn step(tx: &flume::Sender<InstallerEvent>, text: impl Into<String>, progress: f32) {
        let _ = tx.send(InstallerEvent::Step(text.into(), progress));
    }

    fn log(tx: &flume::Sender<InstallerEvent>, text: impl Into<String>) {
        let _ = tx.send(InstallerEvent::Log(text.into()));
    }

    fn command_ok(program: &str, args: &[&str]) -> bool {
        Command::new(program)
            .args(args)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn which(program: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(program);
            if std::fs::metadata(&candidate)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
            {
                return Some(candidate);
            }
        }
        None
    }

    /// Fail-fast probe: returns a summary message when every component this
    /// installer would create is already present *and* the installed binary
    /// matches the bundled version. Returns `None` (proceed with install) if any
    /// component is missing or the installed version differs (upgrade/repair).
    fn existing_install_summary(request: &InstallRequest) -> Option<String> {
        let exe_path = request.install_path.join("sinorag.exe");
        let installed_version = installed_sinorag_version(&exe_path)?;
        if installed_version != EXPECTED_SINORAG_VERSION {
            // A different build is present — let the normal flow overwrite it.
            return None;
        }

        // Corpus + lexical indexes (also runs `sinorag status` as a health gate).
        if !corpus_ready(&exe_path, &request.install_path.join("data")) {
            return None;
        }
        if !opencode_fully_installed() {
            return None;
        }
        if request.desktop_shortcut
            && !desktop_shortcut_path().is_some_and(|p| p.is_file())
        {
            return None;
        }
        if request.start_menu_shortcut
            && !start_menu_shortcut_path().is_some_and(|p| p.is_file())
        {
            return None;
        }

        Some(format!(
            "SinoRAG {installed_version} is already fully installed at {} — corpus, \
             indexes, OpenCode, and shortcuts are all present. Nothing to do.",
            request.install_path.display()
        ))
    }

    /// Run `<exe> --version` and return the bare version (`sinorag 0.5.0` → `0.5.0`).
    fn installed_sinorag_version(exe_path: &Path) -> Option<String> {
        if !exe_path.is_file() {
            return None;
        }
        let out = Command::new(exe_path).arg("--version").output().ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .last()
            .map(str::to_string)
    }

    /// True when the corpus and both lexical indexes the installer builds are
    /// present and the installed binary can read the data root (`sinorag status`).
    fn corpus_ready(exe_path: &Path, data_root: &Path) -> bool {
        let derived = data_root.join("derived");
        let required = [
            data_root.join("passages.parquet"),
            derived.join("doc_table.bin"),
            derived.join("catalog.index"),
            derived.join("phrase.index"),
            derived.join("tfidf.index"),
        ];
        if !required.iter().all(|p| p.exists()) {
            return false;
        }
        if !parquet_has_corpus(&data_root.join("passages.parquet")) {
            return false;
        }
        Command::new(exe_path)
            .arg("status")
            .arg("--data")
            .arg(data_root)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    /// True if the parquet store holds at least one `source_corpus=` partition.
    fn parquet_has_corpus(parquet_root: &Path) -> bool {
        fs::read_dir(parquet_root)
            .map(|read| {
                read.flatten().any(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("source_corpus=")
                })
            })
            .unwrap_or(false)
    }

    /// True when OpenCode is completely installed: the binary resolves on PATH
    /// and `opencode --version` exits 0 with a non-empty version string.
    fn opencode_fully_installed() -> bool {
        let Some(path) = which("opencode")
            .or_else(|| which("opencode.exe"))
            .or_else(|| which("opencode.cmd"))
        else {
            return false;
        };
        Command::new(&path)
            .arg("--version")
            .output()
            .map(|out| out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty())
            .unwrap_or(false)
    }


    fn validate_install_path(value: &str) -> Result<PathBuf, String> {
        if value.trim().is_empty() {
            return Err("install path is empty".to_string());
        }
        let path = PathBuf::from(value);
        let parent = path
            .parent()
            .ok_or_else(|| "install path has no parent directory".to_string())?;
        if parent.exists() {
            let probe_dir = parent.join(format!(".sinorag-write-test-{}", std::process::id()));
            fs::create_dir(&probe_dir)
                .map_err(|error| format!("parent directory is not writable: {error}"))?;
            fs::remove_dir(&probe_dir)
                .map_err(|error| format!("failed to clean write test directory: {error}"))?;
        } else {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create parent directory: {error}"))?;
        }
        Ok(path)
    }

    fn open_install_folder(path: &str) {
        let _ = Command::new("explorer").arg(path).spawn();
    }

    fn default_install_path_str() -> String {
        default_install_path().display().to_string()
    }
}

#[cfg(target_os = "windows")]
fn main() {
    app::run();
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("SinoRAG installer is Windows-only.");
}
