use crate::api::engine::local::Db;
use crate::api::engine::local::ReDb;
use crate::api::err::Error;
use crate::api::opt::Endpoint;
use crate::api::opt::IntoEndpoint;
use crate::api::opt::Strict;
use crate::api::Result;
use std::path::Path;
use crate::dbs::Level;
use url::Url;

impl IntoEndpoint<ReDb> for &str {
	type Client = Db;

	fn into_endpoint(self) -> Result<Endpoint> {
		let url = format!("redb://{self}");
		Ok(Endpoint {
			endpoint: Url::parse(&url).map_err(|_| Error::InvalidUrl(url))?,
			strict: false,
			#[cfg(any(feature = "native-tls", feature = "rustls"))]
			tls_config: None,
			auth: Level::No,
			username: String::new(),
			password: String::new(),
		})
	}
}

impl IntoEndpoint<ReDb> for &Path {
	type Client = Db;

	fn into_endpoint(self) -> Result<Endpoint> {
		let path = self.display().to_string();
		IntoEndpoint::<ReDb>::into_endpoint(path.as_str())
	}
}

impl<T> IntoEndpoint<ReDb> for (T, Strict)
where
	T: AsRef<Path>,
{
	type Client = Db;

	fn into_endpoint(self) -> Result<Endpoint> {
		let (path, _) = self;
		let mut endpoint = IntoEndpoint::<ReDb>::into_endpoint(path.as_ref())?;
		endpoint.strict = true;
		Ok(endpoint)
	}
}