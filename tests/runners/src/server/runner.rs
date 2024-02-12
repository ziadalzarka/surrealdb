use rand::{thread_rng, Rng};
use std::error::Error;
use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::{env, fs};
use tokio::time;
use tracing::{debug, error, info};

use crate::server::config::ServerConfig;


pub struct Runner {
    pub inner: Option<std::process::Child>,
    pub config: ServerConfig,
    pub stdout_path: String,
	pub stderr_path: String
}


impl Runner {
	/// Send some thing to the child's stdin
	pub fn input(mut self, input: &str) -> Self {
		let stdin = self.inner.as_mut().unwrap().stdin.as_mut().unwrap();
		use std::io::Write;
		stdin.write_all(input.as_bytes()).unwrap();
		self
	}

	pub fn kill(mut self) -> Self {
		self.inner.as_mut().unwrap().kill().unwrap();
		self
	}

	pub fn send_signal(&self, signal: nix::sys::signal::Signal) -> nix::Result<()> {
		nix::sys::signal::kill(
			nix::unistd::Pid::from_raw(self.inner.as_ref().unwrap().id() as i32),
			signal,
		)
	}

	pub fn status(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
		self.inner.as_mut().unwrap().try_wait()
	}

	pub fn stdout(&self) -> String {
		std::fs::read_to_string(&self.stdout_path).expect("Failed to read the stdout file")
	}

	pub fn stderr(&self) -> String {
		std::fs::read_to_string(&self.stderr_path).expect("Failed to read the stderr file")
	}

	/// Read the child's stdout concatenated with its stderr. Returns Ok if the child
	/// returns successfully, Err otherwise.
	pub fn output(mut self) -> Result<String, String> {
		let status = self.inner.take().unwrap().wait().unwrap();

		let mut buf = self.stdout();
		buf.push_str(&self.stderr());

		// Cleanup files after reading them
		std::fs::remove_file(self.stdout_path.as_str()).unwrap();
		std::fs::remove_file(self.stderr_path.as_str()).unwrap();

		if status.success() {
			Ok(buf)
		} else {
			Err(buf)
		}
	}
}

impl Drop for Runner {
	fn drop(&mut self) {
		if let Some(inner) = self.inner.as_mut() {
			let _ = inner.kill();
		}
	}
}
