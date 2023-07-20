use crate::api::engine::local::Db;
use crate::api::engine::local::ReDb;
use crate::api::err::Error;
use crate::api::opt::Endpoint;
use crate::api::opt::IntoEndpoint;
use crate::api::opt::Strict;
use crate::api::Result;
use std::path::Path;
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
		})
	}
}

impl IntoEndpoint<ReDb> for &Path {
	type Client = Db;

	fn into_endpoint(self) -> Result<Endpoint> {
		let url = format!("redb://{}", self.display());
		Ok(Endpoint {
			endpoint: Url::parse(&url).map_err(|_| Error::InvalidUrl(url))?,
			strict: false,
			#[cfg(any(feature = "native-tls", feature = "rustls"))]
			tls_config: None,
		})
	}
}

impl<T> IntoEndpoint<ReDb> for (T, Strict)
where
	T: AsRef<Path>,
{
	type Client = Db;

	fn into_endpoint(self) -> Result<Endpoint> {
		let url = format!("redb://{}", self.0.as_ref().display());
		Ok(Endpoint {
			endpoint: Url::parse(&url).map_err(|_| Error::InvalidUrl(url))?,
			strict: true,
			#[cfg(any(feature = "native-tls", feature = "rustls"))]
			tls_config: None,
		})
	}
}