mod server;

use crate::server::config::ServerConfig;


fn main() {
	println!("Hello, world!");
	let mut config = ServerConfig::default();
	println!("config: {:?}", config);
	// config.build_tls();
	println!("config: {:?}", config);
	std::thread::sleep(std::time::Duration::from_secs(10));
}
