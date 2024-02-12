//! Defines the configuration around running a server.
use tokio::time;
use rand::{thread_rng, Rng};
use std::path::Path;
use std::fs;
use cargo_metadata::MetadataCommand;
use std::process::Stdio;
use std::fs::File;


#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub user: String,
    pub password: String,
    pub namespace: String,
    pub database: String,
    pub auth: bool,
    pub tls: bool,
    pub wait_is_ready: bool,
    pub enable_auth_level: bool,
    pub tick_interval: time::Duration,
    pub args: String,
    pub port: u16,
    pub address: String,
}


impl Default for ServerConfig {
    fn default() -> Self {
        let mut rng = thread_rng();
        let port: u16 = rng.gen_range(13000..14000);
	    let addr = format!("127.0.0.1:{port}");
        Self {
            user: "root".to_string(),
            password: "root".to_string(),
            namespace: "testns".to_string(),
            database: "testdb".to_string(),
            auth: true,
            tls: false,
            wait_is_ready: true,
            enable_auth_level: false,
            tick_interval: time::Duration::from_secs(1),
            args: "--allow-all".to_string(),
            port: port,
            address: addr,
        }
    }
}


impl ServerConfig {

    pub fn tmp_file_path(name: &str) -> String {
        let metadata = MetadataCommand::new().exec().unwrap();
        let target_directory = metadata.target_directory;
        let path = Path::new(&target_directory).join(format!("{}-{}", rand::random::<u32>(), name));
        path.to_string_lossy().into_owned()
    }

    pub fn generate_std_io_paths(&self) -> (Stdio, Stdio, String, String) {
        // Use local files instead of pipes to avoid deadlocks. See https://github.com/rust-lang/rust/issues/45572
        let stdout_path: String = Self::tmp_file_path("server-stdout.log");
        let stderr_path: String = Self::tmp_file_path("server-stderr.log");
        let stdout: Stdio = Stdio::from(File::create(&stdout_path).unwrap());
        let stderr: Stdio = Stdio::from(File::create(&stderr_path).unwrap());
        (stdout, stderr, stdout_path, stderr_path)
    }

    pub fn build_tls(args: &mut String) {
        let crt_path = Self::tmp_file_path("crt.crt");
		let key_path = Self::tmp_file_path("key.pem");
        let cert = rcgen::generate_simple_self_signed(Vec::new()).unwrap();
        fs::write(&crt_path, cert.serialize_pem().unwrap()).unwrap();
		fs::write(&key_path, cert.serialize_private_key_pem().into_bytes()).unwrap();

		args.push_str(format!(" --web-crt {crt_path} --web-key {key_path}").as_str());
    }

    pub fn generate_start_args(&self) -> String {

        let mut args_buffer = self.args.clone();

        if self.tls {
            Self::build_tls(&mut args_buffer);
        }

        if self.auth {
            args_buffer.push_str(" --auth");
        }
    
        if self.enable_auth_level {
            args_buffer.push_str(" --auth-level-enabled");
        }
    
        if !self.tick_interval.is_zero() {
            let sec = self.tick_interval.as_secs();
            args_buffer.push_str(format!(" --tick-interval {sec}s").as_str());
        }
        format!(
            "start --bind {} memory --no-banner --log trace --user {} --pass {} {}",
            self.address, self.user, self.password, args_buffer
        )
    }

}


#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_default() {
        let config = ServerConfig::default();
        assert_eq!(config.user, "root");
        assert_eq!(config.password, "root");
        assert_eq!(config.namespace, "testns");
        assert_eq!(config.database, "testdb");
        assert_eq!(config.auth, true);
        assert_eq!(config.tls, false);
        assert_eq!(config.wait_is_ready, true);
        assert_eq!(config.enable_auth_level, false);
        assert_eq!(config.tick_interval, time::Duration::from_secs(1));
        assert_eq!(config.args, "--allow-all");
        assert!(config.port >= 13000 && config.port <= 14000);
        assert_eq!(config.address, format!("127.0.0.1:{}", config.port));
    }

    #[test]
    fn test_partial_default() {
        let config = ServerConfig {
            user: "admin".to_string(),
            ..ServerConfig::default()
        };
        assert_eq!(config.user, "admin");
        assert_eq!(config.password, "root");
        assert_eq!(config.namespace, "testns");
        assert_eq!(config.database, "testdb");
        assert_eq!(config.auth, true);
        assert_eq!(config.tls, false);
        assert_eq!(config.wait_is_ready, true);
        assert_eq!(config.enable_auth_level, false);
        assert_eq!(config.tick_interval, time::Duration::from_secs(1));
        assert_eq!(config.args, "--allow-all");
        assert!(config.port >= 13000 && config.port <= 14000);
        assert_eq!(config.address, format!("127.0.0.1:{}", config.port));
    }

    #[test]
    fn test_generate_default_args() {
        let config = ServerConfig::default();
        let args = config.generate_start_args();
        let expected = format!("start --bind {} memory --no-banner --log trace --user root --pass root --allow-all --auth --tick-interval 1s", config.address);
        assert_eq!(args, expected);
    }

}

