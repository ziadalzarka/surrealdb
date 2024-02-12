use rand::{thread_rng, Rng};
use std::error::Error;
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::{env, fs};
use tokio::time;
use tracing::{debug, error, info};

use crate::server::runner::Runner;
use crate::server::config::ServerConfig;
use crate::server::utils::check_server;


pub async fn start_server_without_auth() -> Result<(String, Runner), Box<dyn Error>> {
    let config = ServerConfig {
        auth: false,
        ..Default::default()
    };
    start_runner::<String>(config, None).await
}


pub async fn start_server_with_auth_level() -> Result<(String, Runner), Box<dyn Error>> {
    let config = ServerConfig {
        auth: true,
        ..Default::default()
    };
    start_runner::<String>(config, None).await
}


pub async fn start_server_with_defaults() -> Result<(String, Runner), Box<dyn Error>> {
    let config = ServerConfig {
        ..Default::default()
    };
	start_runner::<String>(config, None).await
}


pub async fn start_runner<P: AsRef<Path>>(config: ServerConfig, current_dir: Option<P>) 
    -> Result<(String, Runner), Box<dyn Error>> {
    
	let mut path = std::env::current_exe().unwrap();
	assert!(path.pop());
	if path.ends_with("deps") {
		assert!(path.pop());
	}

	// Note: Cargo automatically builds this binary for integration tests.
	path.push(format!("{}{}", env!("CARGO_PKG_NAME"), std::env::consts::EXE_SUFFIX));

	let mut cmd = Command::new(path);
	if let Some(dir) = current_dir {
		cmd.current_dir(&dir);
	}

    let start_args = config.generate_start_args();

    let (stdout, stderr, stdout_path, stderr_path) = config.generate_std_io_paths();
	debug!("Redirecting output. args=`{start_args}` stdout={stdout_path} stderr={stderr_path})");

	cmd.env_clear();
	cmd.stdin(Stdio::piped());
	cmd.stdout(stdout);
	cmd.stderr(stderr);
	cmd.args(start_args.split_ascii_whitespace());

	let runner = Runner {
		inner: Some(cmd.spawn().unwrap()),
        config, 
		stdout_path,
		stderr_path,
	};

    let server_address = runner.config.address.clone();

    if !runner.config.wait_is_ready {
		return Ok((server_address, runner));
	}
    // Wait 5 seconds for the server to start
	let mut interval = time::interval(time::Duration::from_millis(1000));
	info!("Waiting for server to start...");
	for _i in 0..10 {
		interval.tick().await;

		if check_server(&server_address).is_ok() {
			info!("Server ready!");
			return Ok((server_address, runner));
		}
	}

	let server_out = runner.kill().output().err().unwrap();
	error!("server output: {server_out}");
	Err("server failed to start".into())
}
