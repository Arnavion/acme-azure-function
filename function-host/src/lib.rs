#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::type_complexity,
)]

use anyhow::Context;

pub type Function<TSettings> = fn(std::sync::Arc<TSettings>) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send>>;

pub async fn run<TSettings>(functions: std::collections::BTreeMap<&'static str, Function<TSettings>>) -> anyhow::Result<()>
where
	TSettings: serde::de::DeserializeOwned + 'static,
	std::sync::Arc<TSettings>: Send,
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

	let incoming =
		hyper::server::conn::AddrIncoming::bind(&std::net::SocketAddr::V4(std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(127, 0, 0, 1), port)))
		.context("could not bind HTTP server")?;

	let server =
		hyper::Server::builder(incoming)
		.serve(hyper::service::make_service_fn(move |_| std::future::ready(Ok::<_, hyper::Error>(Service {
			functions: functions.clone(),
			settings: settings.clone(),
		}))));
	let () = server.await.context("HTTP server failed")?;
	Ok(())
}

struct Service<TSettings> {
	functions: std::collections::BTreeMap<&'static str, Function<TSettings>>,
	settings: std::sync::Arc<TSettings>,
}

static OK_RESPONSE_BODY: once_cell::sync::Lazy<hyper::body::Bytes> =
	once_cell::sync::Lazy::new(|| hyper::body::Bytes::from_static(br#"{"Outputs":{"":""},"Logs":null,"ReturnValue":""}"#));

impl<TSettings> hyper::service::Service<hyper::Request<hyper::Body>> for Service<TSettings>
where
	TSettings: 'static,
	std::sync::Arc<TSettings>: Send,
{
	type Response = hyper::Response<hyper::Body>;
	type Error = anyhow::Error;
	type Future = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

	fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
		std::task::Poll::Ready(Ok(()))
	}

	fn call(&mut self, req: hyper::Request<hyper::Body>) -> Self::Future {
		let function = self.functions.get(req.uri().path()).copied();
		let settings = self.settings.clone();

		Box::pin(async move {
			eprintln!("{:?}", req);

			let function =
				if let Some(function) = function {
					function
				}
				else {
					let mut response = hyper::Response::new(Default::default());
					*response.status_mut() = hyper::StatusCode::NOT_FOUND;
					return Ok(response);
				};

			if req.method() != hyper::Method::POST {
				let mut response = hyper::Response::new(Default::default());
				*response.status_mut() = hyper::StatusCode::METHOD_NOT_ALLOWED;
				return Ok(response);
			}

			if let Err(err) = function(settings).await.context("function failed") {
				eprintln!("{:?}", err);
				let mut response = hyper::Response::new(format!("{:?}", err).into());
				*response.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
				return Ok(response);
			}

			let mut response = hyper::Response::new(OK_RESPONSE_BODY.clone().into());
			*response.status_mut() = hyper::StatusCode::OK;
			response.headers_mut().insert(hyper::header::CONTENT_TYPE, http_common::APPLICATION_JSON.clone());
			eprintln!("{:?}", response);
			Ok(response)
		})
	}
}
