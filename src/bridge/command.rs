use std::{
    path::Path,
    process::{Command as StdCommand, Stdio},
};

use log::{error, info, warn};
use tokio::process::Command as TokioCommand;

#[cfg(target_os = "windows")]
use crate::settings::*;
use crate::Opts;

pub fn create_nvim_command(opts: &Opts) -> TokioCommand {
    let mut cmd = build_nvim_cmd(opts);

    info!("Starting neovim with: {:?}", cmd);

    #[cfg(not(debug_assertions))]
    cmd.stderr(Stdio::piped());

    #[cfg(debug_assertions)]
    cmd.stderr(Stdio::inherit());

    #[cfg(windows)]
    set_windows_creation_flags(&mut cmd);

    cmd
}

#[cfg(target_os = "windows")]
fn set_windows_creation_flags(cmd: &mut TokioCommand) {
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

fn build_nvim_cmd(opts: &Opts) -> TokioCommand {
    let mut args = opts.nvim_args.to_vec();
    args.extend_from_slice(&opts.files);
    if let Some(ref path) = opts.nvim_path {
        if platform_exists(path) {
            return build_nvim_cmd_with_args(path, &args);
        } else {
            warn!("NVIM is invalid falling back to first bin in PATH");
        }
    }
    if let Some(path) = platform_which("nvim") {
        build_nvim_cmd_with_args(&path, &args)
    } else {
        error!("nvim not found!");
        std::process::exit(1);
    }
}

// Creates a shell command if needed on this platform (wsl or macos)
fn create_platform_shell_command(_command: String) -> Option<StdCommand> {
    #[cfg(target_os = "windows")]
    if SETTINGS.get::<CmdLineSettings>().wsl {
        let mut result = StdCommand::new("wsl");
        result.args(&["$SHELL", "-lic"]);
        result.arg(command);

        Some(result)
    } else {
        None
    }

    #[cfg(target_os = "macos")]
    {
        let shell = std::env::var("SHELL").unwrap();
        let mut result = StdCommand::new(shell);
        result.args(&["-lic"]);
        result.arg(command);
        Some(result)
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    None
}

fn platform_exists(bin: &str) -> bool {
    if let Some(mut exists_command) = create_platform_shell_command(format!("exists -x {}", bin)) {
        if let Ok(output) = exists_command.output() {
            output.status.success()
        } else {
            error!("Exists failed");
            std::process::exit(1);
        }
    } else {
        Path::new(&bin).exists()
    }
}

fn platform_which(bin: &str) -> Option<String> {
    if let Some(mut which_command) = create_platform_shell_command(format!("which {}", bin)) {
        if let Ok(output) = which_command.output() {
            if output.status.success() {
                let nvim_path = String::from_utf8(output.stdout).unwrap();
                return Some(nvim_path.trim().to_owned());
            } else {
                return None;
            }
        }
    }

    // Platform command failed, fallback to which crate
    if let Ok(path) = which::which(bin) {
        path.into_os_string().into_string().ok()
    } else {
        None
    }
}

fn build_nvim_cmd_with_args(bin: &str, nvimargs: &[String]) -> TokioCommand {
    let mut args = vec!["--embed".to_string()];
    args.extend(nvimargs.iter().cloned());

    #[cfg(target_os = "windows")]
    if SETTINGS.get::<CmdLineSettings>().wsl {
        let args_str = args.join(" ");
        let mut cmd = TokioCommand::new("wsl");
        cmd.args(&["$SHELL", "-lc", &format!("{} {}", bin, args_str)]);
        cmd
    } else {
        let mut cmd = TokioCommand::new(bin);
        cmd.args(args);
        cmd
    }

    #[cfg(target_os = "macos")]
    {
        let shell = std::env::var("SHELL").unwrap();
        let args_str = args.join(" ");
        let mut cmd = TokioCommand::new(shell);
        cmd.args(&["-lc", &format!("{} {}", bin, args_str)]);
        cmd
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let mut cmd = TokioCommand::new(bin);
        cmd.args(args);
        cmd
    }
}
