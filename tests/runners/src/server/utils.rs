use rand::{thread_rng, Rng};
use std::error::Error;
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::{env, fs};
use tokio::time;
use tracing::{debug, error, info};
use crate::server::config::ServerConfig;


pub fn check_server(address: &str) -> Result<String, String> {
	let mut path = std::env::current_exe().unwrap();
	assert!(path.pop());
	if path.ends_with("deps") {
		assert!(path.pop());
	}

	path.push(format!("{}{}", env!("CARGO_PKG_NAME"), std::env::consts::EXE_SUFFIX));

	let stdout_path = ServerConfig::tmp_file_path("server-stdout.log");
	let stderr_path = ServerConfig::tmp_file_path("server-stderr.log");
	let stdout = Stdio::from(File::create(&stdout_path).unwrap());
	let stderr = Stdio::from(File::create(&stderr_path).unwrap());

    let command = &format!("isready --conn http://{}", address);

    let mut cmd = Command::new(path);
	cmd.env_clear();
	cmd.stdin(Stdio::piped());
	cmd.stdout(stdout);
	cmd.stderr(stderr);
	cmd.args(command.split_ascii_whitespace());

    let mut running_process = cmd.spawn().unwrap();
    let status = running_process.wait().unwrap();

    let mut buf = std::fs::read_to_string(&stdout_path).expect("Failed to read the stdout file");
    buf.push_str(&std::fs::read_to_string(&stderr_path).expect("Failed to read the stderr file"));

    // Cleanup files after reading them
    std::fs::remove_file(stdout_path.as_str()).unwrap();
    std::fs::remove_file(stderr_path.as_str()).unwrap();

    if status.success() {
        Ok(buf)
    } else {
        Err(buf)
    }
}