// mod clipboard;
mod command;
pub mod create;
mod events;
mod handler;
mod setup;
mod tx_wrapper;
mod ui_commands;

use std::sync::Arc;

use nvim::UiAttachOptions;
use tracing::{error, info};

use crate::{running_tracker::*, settings::*, ConnectionMode, Opts};

pub use command::create_nvim_command;
pub use events::*;
use handler::NeovimHandler;
use setup::setup_neovide_specific_state;
pub use tx_wrapper::{TxWrapper, WrapTx};
pub use ui_commands::{
    start_ui_command_handler, MouseAction, MouseButton, ParallelCommand, SerialCommand, UiCommand,
};

pub async fn open(opts: Opts) {
    let handler = NeovimHandler::new();
    let (nvim, io_handler) = match opts.connection_mode() {
        ConnectionMode::Child => {
            create::new_child_cmd(&mut create_nvim_command(&opts), handler).await
        }
        ConnectionMode::RemoteTcp(address) => create::new_tcp(address, handler).await,
    }
    .expect("Could not locate or start neovim process");

    // Check the neovim version to ensure its high enough
    match nvim.command_output("echo has('nvim-0.6')").await.as_deref() {
        Ok("1") => {} // This is just a guard
        _ => {
            error!("Neovide requires nvim version 0.6 or higher. Download the latest version here https://github.com/neovim/neovim/wiki/Installing-Neovim");
            std::process::exit(0);
        }
    }

    let mut is_remote = false;
    #[cfg(windows)]
    {
        is_remote = opts.wsl;
    }

    if let ConnectionMode::RemoteTcp(_) = opts.connection_mode() {
        is_remote = true;
    }
    setup_neovide_specific_state(&nvim, is_remote).await;

    let mut options = UiAttachOptions::new();
    options
        .set_rgb(true)
        .set_hlstate_external(true)
        // .set_messages_external(true)
        .set_linegrid_external(true)
        .set_multigrid_external(true);

    let (cols, rows) = opts.size.unwrap();
    // Triggers loading the user's config
    nvim.ui_attach(cols, rows, &options)
        .await
        .expect("Could not attach ui to neovim process");

    info!("Neovim process attached");

    let nvim = Arc::new(nvim);

    start_ui_command_handler(nvim.clone());
    SETTINGS.read_initial_values(&nvim).await;
    SETTINGS.setup_changed_listeners(&nvim).await;

    let running_tracker = RUNNING_TRACKER.clone();
    tokio::select! {
        r = io_handler => {
            match r {
                Err(join_error) => error!("Error joining IO loop: '{}'", join_error),
                Ok(Err(error)) => {
                    if !error.is_channel_closed() {
                        error!("Error: '{}'", error);
                    }
                }
                Ok(Ok(())) => {}
            }
            running_tracker.quit("neovim processed failed");
        },
        _ = running_tracker.wait_quit() => {
            info!("io-handler quit.");
        }
    }
}
