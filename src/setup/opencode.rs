//! `sinorag setup opencode` — verify opencode is installed and configured.

use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;

pub struct SetupOpencodeArgs {
    /// Explicit path to opencode (overrides PATH / $OPENCODE_BIN lookup).
    pub opencode: Option<PathBuf>,
}

pub fn run(args: SetupOpencodeArgs) -> Result<()> {
    println!("sinorag setup opencode\n");

    // 1. Locate opencode.
    let opencode =
        crate::which::resolve_binary(args.opencode.as_deref(), "OPENCODE_BIN", "opencode");
    match &opencode {
        Some(path) => {
            println!("[ok] opencode binary: {}", path.display());
        }
        None => {
            println!("[missing] opencode binary not found.");
            print_install_hint();
            println!("\nAfter installing, re-run `sinorag setup opencode`.");
            return Ok(());
        }
    }

    // 2. Verify it runs.
    let path = opencode.as_ref().unwrap();
    match Command::new(path).arg("--version").output() {
        Ok(out) if out.status.success() => {
            let version = String::from_utf8_lossy(&out.stdout);
            println!("[ok] opencode --version: {}", version.trim());
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            println!(
                "[warn] opencode --version exited {} — stderr: {}",
                out.status,
                stderr.trim()
            );
        }
        Err(e) => {
            println!("[warn] could not run opencode --version: {e}");
        }
    }

    // 3. Provider reminder. We don't try to introspect opencode's config
    //    (its shape is internal and may change); instead point the user
    //    at the one command they need to run.
    println!();
    println!("Next steps:");
    println!("  1. Configure an LLM provider for opencode (one-time):  opencode auth login");
    println!("     Anthropic Claude is recommended; OpenAI and others also work.");
    println!("  2. Build (or copy) a SinoRAG corpus pack — see `sinorag status`.");
    println!("  3. Launch the wrapped session:  sinorag agent");
    println!();
    println!("`sinorag agent` regenerates `<workdir>/.opencode/opencode.json` and the");
    println!("sinorag-managed slice of `<workdir>/AGENTS.md`, then execs opencode.");

    Ok(())
}

fn print_install_hint() {
    println!();
    println!("Install opencode:");
    #[cfg(windows)]
    {
        println!("  npm install -g opencode-ai");
        println!("  # or download a release from https://opencode.ai");
    }
    #[cfg(not(windows))]
    {
        println!("  curl -fsSL https://opencode.ai/install | bash");
        println!("  # or:  npm install -g opencode-ai");
    }
    println!("Once installed, ensure it is on PATH or set $OPENCODE_BIN to the full path.");
}
