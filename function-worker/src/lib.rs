#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_underscore_drop,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::shadow_unrelated,
	clippy::too_many_lines,
	clippy::type_complexity,
)]

use anyhow::Context;

#[macro_export]
macro_rules! run {
	($($name:literal => $f:expr ,)*) => {
		fn main() -> anyhow::Result<()> {
			let runtime =
				$crate::_reexports::tokio::runtime::Builder::new_current_thread()
				.enable_io()
				.enable_time()
				.build()?;
			let local_set = $crate::_reexports::tokio::task::LocalSet::new();
			let () = local_set.block_on(&runtime, $crate::_run(|req, settings| async move {
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
			}))?;
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
	TOutput: std::future::Future<Output = anyhow::Result<Option<()>>> + 'static,
{
	{
		let logger = GlobalLogger;
		log::set_logger(Box::leak(Box::new(logger))).expect("could not set global logger");
		log::set_max_level(log::LevelFilter::Info);
	}

	let (log_sender, settings) = {
		let settings = std::env::var("SECRET_SETTINGS").context("could not read SECRET_SETTINGS env var")?;
		let LoggerSettings::<TSettings> {
			azure_log_analytics_workspace_id,
			azure_log_analytics_workspace_signer,
			rest,
		} = serde_json::from_str(&settings).context("could not read SECRET_SETTINGS env var")?;

		let log_sender =
			azure::LogAnalyticsLogSender::new(
				azure_log_analytics_workspace_id,
				azure_log_analytics_workspace_signer,
				concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")),
			).context("could not create LogAnalytics log sender")?;

		(std::sync::Arc::new(log_sender), std::sync::Arc::new(rest))
	};

	let port = match std::env::var("FUNCTIONS_CUSTOMHANDLER_PORT") {
		Ok(value) => value.parse().with_context(|| format!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {:?}", value))?,
		Err(std::env::VarError::NotPresent) => 8080,
		Err(std::env::VarError::NotUnicode(value)) => return Err(anyhow::anyhow!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {:?}", value)),
	};

	let server =
		hyper::Server::try_bind(&([127, 0, 0, 1], port).into())?
		.executor(LocalSetExecutor)
		.serve(hyper::service::make_service_fn(|_| {
			let log_sender = log_sender.clone();
			let settings = settings.clone();

			std::future::ready(Ok::<_, std::convert::Infallible>(hyper::service::service_fn(move |mut req| {
				let log_sender = log_sender.clone();
				let log_sender = move |logger: &'static tokio::task::LocalKey<_>, mut stop_log_sender_rx| async move {
					let mut push_timer = tokio::time::interval(std::time::Duration::from_secs(1));

					loop {
						let push_timer_tick = push_timer.tick();
						futures_util::pin_mut!(push_timer_tick);

						let r = futures_util::future::select(push_timer_tick, stop_log_sender_rx).await;

						let records = logger.with(log2::TaskLocalLogger::take_records);

						if !records.is_empty() {
							static LOG_TYPE: once_cell2::race::LazyBox<hyper::header::HeaderValue> =
								once_cell2::race::LazyBox::new(|| hyper::header::HeaderValue::from_static("FunctionAppLogs"));

							log_sender.send_logs(LOG_TYPE.clone(), records).await?;
						}

						match r {
							futures_util::future::Either::Left((_, stop_log_sender_rx_)) => {
								stop_log_sender_rx = stop_log_sender_rx_;
							},

							futures_util::future::Either::Right(_) => break Ok(()),
						}
					}
				};

				let settings = settings.clone();

				async move {
					static X_AZURE_FUNCTIONS_INVOCATIONID: once_cell2::race::LazyBox<hyper::header::HeaderName> =
						once_cell2::race::LazyBox::new(|| hyper::header::HeaderName::from_static("x-azure-functions-invocationid"));

					let function_invocation_id = req.headers_mut().remove(&*X_AZURE_FUNCTIONS_INVOCATIONID);

					let res = log2::with_task_local_logger(function_invocation_id, log_sender, async move {
						log2::report_state("function_invocation", "", format_args!("{:?}", req));

						let res: hyper::Response<hyper::Body> =
							if req.method() == hyper::Method::POST {
								let output = run_function(req, settings).await;
								match output {
									Ok(Some(())) => {
										let mut res = hyper::Response::new(
											// Workaround for https://github.com/Azure/azure-functions-host/issues/6717
											br#"{"Outputs":{"":""},"Logs":null,"ReturnValue":""}"#[..].into(),
										);
										*res.status_mut() = hyper::StatusCode::OK;
										res.headers_mut().insert(hyper::header::CONTENT_TYPE, http_common::APPLICATION_JSON.clone());
										res
									},

									Ok(None) => {
										let mut res = hyper::Response::new(Default::default());
										*res.status_mut() = hyper::StatusCode::NOT_FOUND;
										res
									},

									Err(err) => {
										log2::report_error(&err);
										let mut res = hyper::Response::new(format!("{:?}", err).into());
										*res.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
										res
									},
								}
							}
							else {
								static ALLOW_POST: once_cell2::race::LazyBox<hyper::header::HeaderValue> =
									once_cell2::race::LazyBox::new(|| hyper::header::HeaderValue::from_static("POST"));

								let mut res = hyper::Response::new(Default::default());
								*res.status_mut() = hyper::StatusCode::METHOD_NOT_ALLOWED;
								res.headers_mut().insert(hyper::header::ALLOW, ALLOW_POST.clone());
								res
							};

						log2::report_state("function_invocation", "", format_args!("{:?}", res));

						res
					}).await;
					Ok::<_, std::convert::Infallible>(res)
				}
			})))
		}));

	let () = server.await.context("HTTP server failed")?;
	Ok(())
}

#[derive(serde::Deserialize)]
struct LoggerSettings<TSettings> {
	/// The Azure Log Analytics workspace's customer ID
	azure_log_analytics_workspace_id: String,

	/// The Azure Log Analytics workspace's shared key
	#[serde(deserialize_with = "deserialize_signer")]
	#[serde(rename = "azure_log_analytics_workspace_key")]
	azure_log_analytics_workspace_signer: hmac::Hmac<sha2::Sha256>,

	#[serde(flatten)]
	rest: TSettings,
}

fn deserialize_signer<'de, D>(deserializer: D) -> Result<hmac::Hmac<sha2::Sha256>, D::Error> where D: serde::Deserializer<'de> {
	struct Visitor;

	impl serde::de::Visitor<'_> for Visitor {
		type Value = hmac::Hmac<sha2::Sha256>;

		fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			f.write_str("base64-encoded string")
		}

		fn visit_str<E>(self, s: &str) -> Result<Self::Value, E> where E: serde::de::Error {
			let key = base64::decode(s).map_err(serde::de::Error::custom)?;
			let signer = hmac::NewMac::new_varkey(&key).expect("cannot fail to create hmac::Hmac<sha2::Sha256>");
			Ok(signer)
		}
	}

	deserializer.deserialize_str(Visitor)
}

struct GlobalLogger;

impl log::Log for GlobalLogger {
	fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
		metadata.level() <= log::Level::Info
	}

	fn log(&self, record: &log::Record<'_>) {
		if !self.enabled(record.metadata()) {
			return;
		}

		let timestamp = chrono::Utc::now();
		let level = record.level();

		eprintln!("[{}] {:5} {}", timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), level, record.args());
	}

	fn flush(&self) {
	}
}

#[derive(Clone, Copy)]
struct LocalSetExecutor;

impl<Fut> hyper::rt::Executor<Fut> for LocalSetExecutor where Fut: std::future::Future + 'static {
	fn execute(&self, fut: Fut) {
		let _ = tokio::task::spawn_local(fut);
	}
}
