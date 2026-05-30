/// sinorag-uninstaller — console-mode uninstaller for SinoRAG.
///
/// Double-click (or run from Settings → Apps) to launch. Prompts once for
/// confirmation, then removes shortcuts, the Apps & features registry entry,
/// and schedules the install directory for deletion after exit.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("sinorag-uninstaller is Windows-only.");
}

#[cfg(target_os = "windows")]
fn main() {
    use sinorag_installer::win_utils::{
        delete_uninstall_entry, desktop_shortcut_path, read_install_location,
        start_menu_shortcut_path,
    };
    use std::io::{self, Write};
    use std::path::PathBuf;

    // Locate the install directory: prefer the registry entry, fall back to the
    // directory that contains this executable.
    let install_dir = read_install_location().unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from(r"C:\SinoRAG"))
    });

    println!("SinoRAG Uninstaller");
    println!("-------------------");
    println!("Install location: {}", install_dir.display());
    println!();
    print!("Uninstall SinoRAG from this location? [y/N] ");
    let _ = io::stdout().flush();

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        eprintln!("Failed to read input. Aborting.");
        std::process::exit(1);
    }

    let answer = answer.trim().to_lowercase();
    if answer != "y" && answer != "yes" {
        println!("Aborted.");
        return;
    }

    // Remove shortcuts (best-effort; missing shortcuts are not an error).
    if let Some(path) = desktop_shortcut_path() {
        if path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("Warning: could not remove desktop shortcut: {e}");
            } else {
                println!("Removed: {}", path.display());
            }
        }
    }

    if let Some(path) = start_menu_shortcut_path() {
        if path.exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("Warning: could not remove Start Menu shortcut: {e}");
            } else {
                println!("Removed: {}", path.display());
            }
        }
        // Remove the SinoRAG Start Menu folder if it is now empty.
        if let Some(folder) = path.parent() {
            if folder.exists() {
                let _ = std::fs::remove_dir(folder); // fails silently if non-empty
            }
        }
    }

    // Remove the Apps & features / Uninstall registry key.
    delete_uninstall_entry();
    println!("Removed uninstall registry entry.");

    // The install directory contains this running executable, so we can't
    // delete it while it is in use. Spawn a detached cmd that waits for this
    // process to exit, then removes the directory tree. DETACHED_PROCESS frees
    // the child from this console so it survives after the window closes; `ping`
    // is a console-independent delay (unlike `timeout`, which needs console
    // stdin and errors out when redirected).
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    println!("Scheduling install directory removal...");
    let remove_cmd = format!(
        "ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{}\"",
        install_dir.display()
    );
    match std::process::Command::new("cmd")
        .args(["/c", &remove_cmd])
        .creation_flags(DETACHED_PROCESS)
        .spawn()
    {
        Ok(_) => println!("SinoRAG has been uninstalled."),
        Err(e) => {
            eprintln!(
                "Could not schedule directory removal: {e}\n\
                 Please delete {} manually.",
                install_dir.display()
            );
        }
    }
}
