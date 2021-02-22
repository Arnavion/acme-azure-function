#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::type_complexity,
)]

use anyhow::Context;

#[macro_export]
macro_rules! run {
	($($name:literal => $f:expr ,)*) => {
		fn main() -> anyhow::Result<()> {
			let () =
				$crate::_reexports::tokio::runtime::Builder::new_current_thread()
				.enable_io()
				.enable_time()
				.build()?
				.block_on(async {
					$crate::_run(|req, settings| async move {
						let path = req.uri().path();
						if let Some(path) = path.strip_prefix('/') {
							Ok(match path {
								$($name => Some($f(settings).await?) ,)*
								_ => None,
							})
						}
						else {
							Ok(None)
						}
					}).await
				})?;
			Ok(())
		}
	};
}

#[doc(hidden)]
pub mod _reexports {
	pub use tokio;
}

#[doc(hidden)]
pub async fn _run<TSettings, TOutput>(
	run_function: fn(req: hyper::Request<hyper::Body>, settings: std::sync::Arc<TSettings>) -> TOutput,
) -> anyhow::Result<()>
where
	TSettings: serde::de::DeserializeOwned + 'static,
	std::sync::Arc<TSettings>: Send,
	TOutput: std::future::Future<Output = anyhow::Result<Option<()>>> + Send + 'static,
{
	let settings = {
		let settings = std::env::var("SECRET_SETTINGS").context("could not read SECRET_SETTINGS env var")?;
		let settings: TSettings = serde_json::from_str(&settings).context("could not read SECRET_SETTINGS env var")?;
		std::sync::Arc::new(settings)
	};

	let port = match std::env::var("FUNCTIONS_CUSTOMHANDLER_PORT") {
		Ok(value) => value.parse().with_context(|| format!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {:?}", value))?,
		Err(std::env::VarError::NotPresent) => 8080,
		Err(std::env::VarError::NotUnicode(value)) => return Err(anyhow::anyhow!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {:?}", value)),
	};

	let server =
		hyper::Server::try_bind(&([127, 0, 0, 1], port).into())?
		.serve(hyper::service::make_service_fn(|_| {
			let settings = settings.clone();

			async move {
				Ok::<_, std::convert::Infallible>(hyper::service::service_fn(move |req| {
					let settings = settings.clone();

					async move {
						eprintln!("{:?}", req);

						if req.method() != hyper::Method::POST {
							let mut response = hyper::Response::new(Default::default());
							*response.status_mut() = hyper::StatusCode::METHOD_NOT_ALLOWED;
							return Ok(response);
						}

						let output = run_function(req, settings).await;
						let response = match output {
							Ok(Some(())) => {
								let mut response = hyper::Response::new(hyper::Body::from(
									hyper::body::Bytes::from_static(br#"{"Outputs":{"":""},"Logs":null,"ReturnValue":""}"#),
								));
								*response.status_mut() = hyper::StatusCode::OK;
								response.headers_mut().insert(
									hyper::header::CONTENT_TYPE,
									http_common::APPLICATION_JSON.clone(),
								);
								response
							},

							Ok(None) => {
								let mut response = hyper::Response::new(Default::default());
								*response.status_mut() = hyper::StatusCode::NOT_FOUND;
								response
							},

							Err(err) => {
								eprintln!("{:?}", err);
								let mut response = hyper::Response::new(format!("{:?}", err).into());
								*response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
								response
							},
						};
						eprintln!("{:?}", response);
						Ok::<_, std::convert::Infallible>(response)
					}
				}))
			}
		}));

	let () = server.await.context("HTTP server failed")?;
	Ok(())
}
