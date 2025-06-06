use crate::lib::canister_info::CanisterInfo;
use crate::lib::deps::get_pull_canisters_in_config;
use crate::lib::environment::Environment;
use crate::lib::error::DfxResult;
use crate::lib::operations::canister::install_canister::install_canister;
use crate::lib::root_key::fetch_root_key_if_needed;
use crate::util::clap::argument_from_cli::ArgumentFromCliLongOpt;
use crate::util::clap::install_mode::{InstallModeHint, InstallModeOpt};
use crate::util::{ask_for_consent, blob_from_arguments};
use dfx_core::canister::{
    install_canister_wasm, install_mode_to_past_tense, install_mode_to_present_tense,
};
use dfx_core::identity::CallSender;

use crate::lib::operations::canister::skip_remote_canister;
use anyhow::bail;
use candid::Principal;
use clap::Parser;
use slog::info;
use std::path::PathBuf;

/// Installs compiled code in a canister.
#[derive(Parser, Clone)]
pub struct CanisterInstallOpts {
    /// Specifies the canister to deploy. You must specify either canister name/id or the --all option.
    canister: Option<String>,

    /// Deploys all canisters configured in the project dfx.json files.
    #[arg(
        long,
        required_unless_present("canister"),
        conflicts_with("argument"),
        conflicts_with("argument_file")
    )]
    all: bool,

    /// Specifies not to wait for the result of the call to be returned by polling the replica. Instead return a response ID.
    #[arg(long)]
    async_call: bool,

    #[command(flatten)]
    install_mode: InstallModeOpt,

    /// Upgrade the canister even if the .wasm did not change.
    #[arg(long)]
    upgrade_unchanged: bool,

    #[command(flatten)]
    argument_from_cli: ArgumentFromCliLongOpt,

    /// Specifies a particular Wasm file to install, bypassing the dfx.json project settings.
    #[arg(long, conflicts_with("all"))]
    wasm: Option<PathBuf>,

    /// Output environment variables to a file in dotenv format (without overwriting any user-defined variables, if the file already exists).
    output_env_file: Option<PathBuf>,

    /// Skips yes/no checks by answering 'yes'. Such checks usually result in data loss,
    /// so this is not recommended outside of CI.
    #[arg(long, short)]
    yes: bool,

    /// Skips upgrading the asset canister, to only install the assets themselves.
    #[arg(long)]
    no_asset_upgrade: bool,

    /// Always use Candid assist when the argument types are all optional.
    #[arg(
        long,
        conflicts_with("argument"),
        conflicts_with("argument_file"),
        conflicts_with("yes")
    )]
    always_assist: bool,
}

pub async fn exec(
    env: &dyn Environment,
    opts: CanisterInstallOpts,
    call_sender: &CallSender,
) -> DfxResult {
    fetch_root_key_if_needed(env).await?;

    let mode_hint = opts.install_mode.mode_for_canister_install()?;
    let canister_id_store = env.get_canister_id_store()?;
    let network = env.get_network_descriptor();

    if mode_hint == InstallModeHint::Reinstall && (opts.canister.is_none() || opts.all) {
        bail!("The --mode=reinstall is only valid when specifying a single canister, because reinstallation destroys all data in the canister.");
    }

    if let Some(canister) = opts.canister.as_deref() {
        let (argument_from_cli, argument_type) = opts.argument_from_cli.get_argument_and_type()?;
        // `opts.canister` is a Principal (canister ID)
        if let Ok(canister_id) = Principal::from_text(canister) {
            if let Some(wasm_path) = &opts.wasm {
                let args = blob_from_arguments(
                    Some(env),
                    argument_from_cli.as_deref(),
                    None,
                    argument_type.as_deref(),
                    &None,
                    true,
                    opts.always_assist,
                )?;
                let wasm_module = dfx_core::fs::read(wasm_path)?;
                let mode = mode_hint.to_install_mode_with_wasm_path()?;
                let spinner = env.new_spinner(
                    format!(
                        "{} code for canister {}",
                        install_mode_to_present_tense(&mode),
                        canister_id,
                    )
                    .into(),
                );

                install_canister_wasm(
                    env.get_agent(),
                    canister_id,
                    None,
                    &args,
                    mode,
                    call_sender,
                    wasm_module,
                    |message| {
                        if opts.yes {
                            Ok(())
                        } else {
                            ask_for_consent(env, message)
                        }
                    },
                )
                .await?;
                spinner.finish_and_clear();
                info!(
                    env.get_logger(),
                    "{} code for canister {}",
                    install_mode_to_past_tense(&mode),
                    canister_id
                );
                Ok(())
            } else {
                bail!("When installing a canister by its ID, you must specify `--wasm` option.")
            }
        } else {
            // `opts.canister` is not a canister ID, but a canister name
            let config = env.get_config_or_anyhow()?;
            let config_interface = config.get_config();
            let env_file = config.get_output_env_file(opts.output_env_file)?;
            let pull_canisters_in_config = get_pull_canisters_in_config(env)?;
            if pull_canisters_in_config.contains_key(canister) {
                bail!(
                    "{0} is a pull dependency. Please deploy it using `dfx deps deploy {0}`",
                    canister
                );
            }
            if config_interface.is_remote_canister(canister, &network.name)? {
                bail!("Canister '{}' is a remote canister on network '{}', and cannot be installed from here.", canister, &network.name)
            }

            let canister_id = canister_id_store.get(canister)?;
            let canister_info = CanisterInfo::load(&config, canister, Some(canister_id))?;
            if let Some(wasm_path) = opts.wasm {
                // streamlined version, we can ignore most of the environment
                install_canister(
                    env,
                    canister_id_store,
                    canister_id,
                    &canister_info,
                    Some(&wasm_path),
                    argument_from_cli.as_deref(),
                    argument_type.as_deref(),
                    &mode_hint,
                    call_sender,
                    opts.upgrade_unchanged,
                    None,
                    opts.yes,
                    None,
                    opts.no_asset_upgrade,
                    opts.always_assist,
                )
                .await
                .map_err(Into::into)
            } else {
                install_canister(
                    env,
                    canister_id_store,
                    canister_id,
                    &canister_info,
                    None,
                    argument_from_cli.as_deref(),
                    argument_type.as_deref(),
                    &mode_hint,
                    call_sender,
                    opts.upgrade_unchanged,
                    None,
                    opts.yes,
                    env_file.as_deref(),
                    opts.no_asset_upgrade,
                    opts.always_assist,
                )
                .await
                .map_err(Into::into)
            }
        }
    } else if opts.all {
        // Install all canisters.
        let config = env.get_config_or_anyhow()?;
        let env_file = config.get_output_env_file(opts.output_env_file)?;
        let pull_canisters_in_config = get_pull_canisters_in_config(env)?;
        if let Some(canisters) = &config.get_config().canisters {
            for canister in canisters.keys() {
                if pull_canisters_in_config.contains_key(canister) {
                    continue;
                }
                if skip_remote_canister(env, canister)? {
                    continue;
                }

                let canister_id = canister_id_store.get(canister)?;
                let canister_info = CanisterInfo::load(&config, canister, Some(canister_id))?;
                install_canister(
                    env,
                    canister_id_store,
                    canister_id,
                    &canister_info,
                    None,
                    None,
                    None,
                    &mode_hint,
                    call_sender,
                    opts.upgrade_unchanged,
                    None,
                    opts.yes,
                    env_file.as_deref(),
                    opts.no_asset_upgrade,
                    opts.always_assist,
                )
                .await?;
            }
        }
        if !pull_canisters_in_config.is_empty() {
            info!(env.get_logger(), "There are pull dependencies defined in dfx.json. Please deploy them using `dfx deps deploy`.");
        }
        Ok(())
    } else {
        unreachable!()
    }
}
