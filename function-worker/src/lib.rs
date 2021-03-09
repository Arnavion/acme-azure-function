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
			let () = local_set.block_on(&runtime, $crate::_run(|req, azure_subscription_id, azure_auth, settings| async move {
				let path = req.uri().path();
				if let Some(path) = path.strip_prefix('/') {
					Ok(match path {
						$($name => Some($f(azure_subscription_id, azure_auth, settings).await?) ,)*
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
	run_function: fn(
		req: hyper::Request<hyper::Body>,
		azure_subscription_id: std::rc::Rc<str>,
		azure_auth: std::rc::Rc<azure::Auth>,
		settings: std::rc::Rc<TSettings>,
	) -> TOutput,
) -> anyhow::Result<()>
where
	TSettings: serde::de::DeserializeOwned + 'static,
	TOutput: std::future::Future<Output = anyhow::Result<Option<()>>> + 'static,
{
	{
		let logger = GlobalLogger;
		log::set_logger(Box::leak(Box::new(logger))).expect("could not set global logger");
		log::set_max_level(log::LevelFilter::Info);
	}

	let (log_sender, azure_subscription_id, azure_auth, settings) = {
		let settings = std::env::var("SECRET_SETTINGS").context("could not read SECRET_SETTINGS env var")?;
		let LoggerSettings::<TSettings> {
			azure_subscription_id,
			azure_auth,
			azure_log_analytics_workspace_resource_group_name,
			azure_log_analytics_workspace_name,
			rest,
		} = serde_json::from_str(&settings).context("could not read SECRET_SETTINGS env var")?;

		let log_sender =
			azure::management::Client::new(
				&azure_subscription_id,
				&azure_log_analytics_workspace_resource_group_name,
				&azure_auth,
				concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
					.parse().expect("hard-coded user agent is valid HeaderValue"),
			).context("could not initialize Azure Management API client")?
			.log_analytics_log_sender(&azure_log_analytics_workspace_name)
			.await.context("could not create LogAnalytics log sender")?;

		(
			std::rc::Rc::new(log_sender),
			azure_subscription_id,
			azure_auth,
			rest,
		)
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
			let azure_subscription_id = azure_subscription_id.clone();
			let azure_auth = azure_auth.clone();
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

				let azure_subscription_id = azure_subscription_id.clone();
				let azure_auth = azure_auth.clone();
				let settings = settings.clone();

				async move {
					static X_AZURE_FUNCTIONS_INVOCATIONID: once_cell2::race::LazyBox<hyper::header::HeaderName> =
						once_cell2::race::LazyBox::new(|| hyper::header::HeaderName::from_static("x-azure-functions-invocationid"));

					let function_invocation_id = req.headers_mut().remove(&*X_AZURE_FUNCTIONS_INVOCATIONID);

					let res = log2::with_task_local_logger(function_invocation_id, log_sender, async move {
						log2::report_state("function_invocation", "", format_args!("{:?}", req));

						let res: hyper::Response<hyper::Body> =
							if req.method() == hyper::Method::POST {
								let output = run_function(req, azure_subscription_id, azure_auth, settings).await;
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
	/// The Azure subscription ID.
	azure_subscription_id: std::rc::Rc<str>,

	/// The Azure authentication credentials.
	///
	/// Defaults to parsing `azure::Auth::ManagedIdentity` from the environment.
	/// If not found, then debug builds fall back to parsing a service principal from this JSON object's
	/// `{ azure_client_id: String, azure_client_secret: String, azure_tenant_id: String }` properties.
	#[serde(flatten)]
	azure_auth: std::rc::Rc<azure::Auth>,

	/// The name of the Azure resource group that contains the Azure Log Analytics workspace.
	azure_log_analytics_workspace_resource_group_name: String,

	/// The name of the Azure Log Analytics workspace.
	azure_log_analytics_workspace_name: String,

	#[serde(flatten)]
	rest: std::rc::Rc<TSettings>,
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
