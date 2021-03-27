#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_underscore_drop,
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::shadow_unrelated,
	clippy::similar_names,
	clippy::too_many_lines,
	clippy::type_complexity,
)]

use anyhow::Context;

#[macro_export]
macro_rules! run {
	($($name:literal => $f:expr ,)*) => {
		fn main() -> $crate::_reexports::anyhow::Result<()> {
			use $crate::_reexports::anyhow::Context;

			let runtime =
				$crate::_reexports::tokio::runtime::Builder::new_current_thread()
				.enable_io()
				.enable_time()
				.build()?;
			let local_set = $crate::_reexports::tokio::task::LocalSet::new();

			let () = local_set.block_on(&runtime, async {
				let (
					misc_logger,
					settings,
					incoming,
				) = $crate::_init().await?;
				$crate::_reexports::futures_util::pin_mut!(incoming);

				let (
					azure_subscription_id,
					azure_auth,
					azure_log_analytics_workspace_resource_group_name,
					azure_log_analytics_workspace_name,
					settings,
				) = $crate::_parse_settings(&settings)?;

				let log_sender =
					$crate::_log_sender(
						&misc_logger,
						&azure_subscription_id,
						&azure_log_analytics_workspace_resource_group_name,
						&azure_log_analytics_workspace_name,
						&azure_auth,
					).await?;

				let mut pending_requests = $crate::_reexports::futures_util::stream::FuturesUnordered::new();

				while let Some(stream) = $crate::_next_stream(&mut incoming, &mut pending_requests, &misc_logger).await? {
					pending_requests.push(async {
						let mut stream = stream;

						let mut buf = [std::mem::MaybeUninit::uninit(); 8192];
						let mut buf = $crate::_reexports::tokio::io::ReadBuf::uninit(&mut buf);
						let (method, path, logger) = loop {
							if let Some(req) = $crate::_parse_request(&mut stream, &mut buf).await? {
								break req;
							}
						};

						$crate::_handle_request(method, path, &logger, &log_sender, &mut stream, async {
							match path {
								$(
									$name => match $f(&azure_subscription_id, &azure_auth, &settings, &logger).await {
										Ok(message) => $crate::_Response::Ok(message),
										Err(err) => $crate::_Response::Error(format!("{:?}", err)),
									},
								)*
								_ => $crate::_Response::UnknownFunction,
							}
						}).await?;

						Ok(())
					});
				}

				Ok::<_, $crate::_reexports::anyhow::Error>(())
			})?;

			Ok(())
		}
	};
}

#[doc(hidden)]
pub mod _reexports {
	pub use anyhow;
	pub use futures_util;
	pub use tokio;
}

#[doc(hidden)]
pub async fn _init() -> anyhow::Result<(log2::Logger, String, impl futures_util::stream::Stream<Item = anyhow::Result<tokio::net::TcpStream>>)> {
	{
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

		let logger = GlobalLogger;
		log::set_logger(Box::leak(Box::new(logger))).expect("could not set global logger");
		log::set_max_level(log::LevelFilter::Info);
	}

	let misc_logger = log2::Logger::new(None, false);

	let settings = std::env::var("SECRET_SETTINGS").context("could not read SECRET_SETTINGS env var")?;

	let port = match std::env::var("FUNCTIONS_CUSTOMHANDLER_PORT") {
		Ok(value) => value.parse().with_context(|| format!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {:?}", value))?,
		Err(std::env::VarError::NotPresent) => 8080,
		Err(std::env::VarError::NotUnicode(value)) =>
			return Err(anyhow::anyhow!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {:?}", value)),
	};

	let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 1), port)).await?;
	let incoming = futures_util::stream::try_unfold(listener, |listener| async {
		let (stream, _) = listener.accept().await.context("could not accept connection")?;
		Ok(Some((stream, listener)))
	});

	Ok((
		misc_logger,
		settings,
		incoming,
	))
}

#[doc(hidden)]
pub fn _parse_settings<'a, TSettings>(settings: &'a str) -> anyhow::Result<(
	std::borrow::Cow<'a, str>,
	azure::Auth,
	std::borrow::Cow<'a, str>,
	std::borrow::Cow<'a, str>,
	TSettings,
)> where TSettings: serde::Deserialize<'a> {
	#[derive(serde::Deserialize)]
	struct LoggerSettings<'a, TSettings> {
		/// The Azure subscription ID.
		#[serde(borrow)]
		azure_subscription_id: std::borrow::Cow<'a, str>,

		/// The Azure authentication credentials.
		///
		/// Defaults to parsing `azure::Auth::ManagedIdentity` from the environment.
		/// If not found, then debug builds fall back to parsing a service principal from this JSON object's
		/// `{ azure_client_id: String, azure_client_secret: String, azure_tenant_id: String }` properties.
		#[serde(flatten)]
		azure_auth: azure::Auth,

		/// The name of the Azure resource group that contains the Azure Log Analytics workspace.
		#[serde(borrow)]
		azure_log_analytics_workspace_resource_group_name: std::borrow::Cow<'a, str>,

		/// The name of the Azure Log Analytics workspace.
		#[serde(borrow)]
		azure_log_analytics_workspace_name: std::borrow::Cow<'a, str>,

		#[serde(flatten)]
		rest: TSettings,
	}

	let LoggerSettings {
		azure_subscription_id,
		azure_auth,
		azure_log_analytics_workspace_resource_group_name,
		azure_log_analytics_workspace_name,
		rest: settings,
	} = serde_json::from_str(&settings).context("could not read SECRET_SETTINGS env var")?;
	Ok((
		azure_subscription_id,
		azure_auth,
		azure_log_analytics_workspace_resource_group_name,
		azure_log_analytics_workspace_name,
		settings,
	))
}

#[doc(hidden)]
pub async fn _log_sender<'a>(
	misc_logger: &'a log2::Logger,
	azure_subscription_id: &'a str,
	azure_log_analytics_workspace_resource_group_name: &'a str,
	azure_log_analytics_workspace_name: &'a str,
	azure_auth: &'a azure::Auth,
) -> anyhow::Result<azure::management::log_analytics::LogSender<'a>> {
	let log_sender =
		azure::management::Client::new(
			azure_subscription_id,
			azure_log_analytics_workspace_resource_group_name,
			azure_auth,
			concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
				.parse().expect("hard-coded user agent is valid HeaderValue"),
			misc_logger,
		).context("could not initialize Azure Management API client")?
		.log_analytics_log_sender(azure_log_analytics_workspace_name)
		.await.context("could not create LogAnalytics log sender")?;
	Ok(log_sender)
}

#[doc(hidden)]
pub async fn _next_stream(
	incoming: &mut (impl futures_util::stream::Stream<Item = anyhow::Result<tokio::net::TcpStream>> + Unpin),
	pending_requests: &mut futures_util::stream::FuturesUnordered<impl std::future::Future<Output = anyhow::Result<()>>>,
	misc_logger: &log2::Logger,
) -> anyhow::Result<Option<tokio::net::TcpStream>> {
	loop {
		if pending_requests.is_empty() {
			let stream = futures_util::TryStreamExt::try_next(incoming).await?;
			break Ok(stream);
		}

		let next = futures_util::future::try_select(futures_util::TryStreamExt::try_next(incoming), futures_util::TryStreamExt::try_next(pending_requests));
		match next.await {
			Ok(futures_util::future::Either::Left((stream, _))) => break Ok(stream),
			Ok(futures_util::future::Either::Right(_)) => (),
			Err(err) => {
				let (err, _) = err.factor_first();
				misc_logger.report_error(&err);
			},
		}
	}
}

#[allow(clippy::needless_lifetimes)] // TODO: https://github.com/rust-lang/rust-clippy/issues/5787
#[doc(hidden)]
pub async fn _parse_request<'a>(
	stream: &mut tokio::net::TcpStream,
	mut buf: &'a mut tokio::io::ReadBuf<'_>,
) -> anyhow::Result<Option<(&'a str, &'a str, log2::Logger)>> {
	if buf.remaining() == 0 {
		return Err(anyhow::anyhow!("request headers too large"));
	}

	{
		let previous_filled = buf.filled().len();
		let () =
			futures_util::future::poll_fn(|cx|
				tokio::io::AsyncRead::poll_read(std::pin::Pin::new(stream), cx, &mut buf),
			).await.context("could not read request")?;
		let new_filled = buf.filled().len();
		if previous_filled == new_filled {
			return Err(anyhow::anyhow!("malformed request: EOF"));
		}
	}

	let mut headers = [httparse::EMPTY_HEADER; 16];
	let mut req = httparse::Request::new(&mut headers);
	let body_start = match req.parse(buf.filled()).context("malformed request")? {
		httparse::Status::Complete(body_start) => body_start,
		httparse::Status::Partial => return Ok(None),
	};

	let method = req.method.context("malformed request: no method")?;

	let path =
		req.path
		.and_then(|path| path.strip_prefix('/'))
		.context("malformed request: no path")?;

	if req.version != Some(1) {
		return Err(anyhow::anyhow!("malformed request: not HTTP/1.1"));
	}

	let mut function_invocation_id = None;

	for &httparse::Header { name, value } in &*req.headers {
		const X_AZURE_FUNCTIONS_INVOCATIONID: &str = "x-azure-functions-invocationid";

		if name.eq_ignore_ascii_case("content-length") {
			// We're able to send a response and close the connection without reading the request body,
			// but FunctionHost doesn't like it and fails the function invocation because it wasn't able to write the request body
			// in its entirety. So we need to drain the request body.

			let content_length: usize =
				std::str::from_utf8(value).context("malformed request: malformed content-length header")?
				.parse().context("malformed request: malformed content-length header")?;
			let mut remaining = content_length - (buf.filled().len() - body_start);

			let mut buf = [std::mem::MaybeUninit::uninit(); 8192];
			let mut buf = tokio::io::ReadBuf::uninit(&mut buf);

			while remaining > 0 {
				buf.clear();

				let () =
					futures_util::future::poll_fn(|cx|
						tokio::io::AsyncRead::poll_read(std::pin::Pin::new(stream), cx, &mut buf),
					).await.context("could not read request body")?;
				let read = buf.filled().len();
				if read == 0 {
					return Err(anyhow::anyhow!("malformed request: EOF"));
				}
				remaining = remaining.checked_sub(read).unwrap_or_default();
			}
		}
		else if name.eq_ignore_ascii_case(X_AZURE_FUNCTIONS_INVOCATIONID) {
			function_invocation_id = std::str::from_utf8(value).ok().map(ToOwned::to_owned);
		}
	}

	let logger = log2::Logger::new(function_invocation_id, true);

	Ok(Some((
		method,
		path,
		logger,
	)))
}

#[doc(hidden)]
#[derive(Debug)]
pub enum _Response {
	Ok(&'static str),
	UnknownFunction,
	MethodNotAllowed,
	Error(String),
}

#[doc(hidden)]
pub async fn _handle_request(
	method: &str,
	path: &str,
	logger: &log2::Logger,
	log_sender: &azure::management::log_analytics::LogSender<'_>,
	stream: &mut tokio::net::TcpStream,
	res_f: impl std::future::Future<Output = _Response>,
) -> anyhow::Result<()> {
	let res_f = async {
		logger.report_state("function_invocation", "", format_args!("Request {{ method: {:?}, path: {:?} }}", method, path));

		let res =
			if method == "POST" {
				res_f.await
			}
			else {
				_Response::MethodNotAllowed
			};

		logger.report_state("function_invocation", "", format_args!("Response {{ {:?} }}", res));
		res
	};
	futures_util::pin_mut!(res_f);

	let (stop_log_sender_tx, mut stop_log_sender_rx) = tokio::sync::oneshot::channel();

	let log_sender_f = async {
		let mut push_timer = tokio::time::interval(std::time::Duration::from_secs(1));

		loop {
			let push_timer_tick = push_timer.tick();
			futures_util::pin_mut!(push_timer_tick);

			let r = futures_util::future::select(push_timer_tick, stop_log_sender_rx).await;

			let records = logger.take_records();

			if !records.is_empty() {
				static LOG_TYPE: once_cell2::race::LazyBox<http::HeaderValue> =
					once_cell2::race::LazyBox::new(|| http::HeaderValue::from_static("FunctionAppLogs"));

				log_sender.send_logs(LOG_TYPE.clone(), records).await?;
			}

			match r {
				futures_util::future::Either::Left((_, stop_log_sender_rx_)) => stop_log_sender_rx = stop_log_sender_rx_,
				futures_util::future::Either::Right(_) => break Ok::<_, anyhow::Error>(()),
			}
		}
	};
	futures_util::pin_mut!(log_sender_f);

	let res = match futures_util::future::select(res_f, log_sender_f).await {
		futures_util::future::Either::Left((res, log_sender_f)) => {
			let _ = stop_log_sender_tx.send(());

			if let Err(err) = log_sender_f.await {
				log::error!("{:?}", err.context("log sender failed"));
			}

			res
		},

		futures_util::future::Either::Right((Ok(()), _)) =>
			unreachable!("log sender completed before scoped future"),

		futures_util::future::Either::Right((Err(err), res_f)) => {
			log::error!("{:?}", err.context("log sender failed"));
			res_f.await
		},
	};

	let status = match &res {
		_Response::Ok(_) => http::StatusCode::OK,
		_Response::UnknownFunction => http::StatusCode::NOT_FOUND,
		_Response::MethodNotAllowed => http::StatusCode::METHOD_NOT_ALLOWED,
		_Response::Error(_) => http::StatusCode::INTERNAL_SERVER_ERROR,
	};

	let mut io_slices = [
		std::io::IoSlice::new(b"HTTP/1.1 "),
		std::io::IoSlice::new(status.as_str().as_bytes()),
		std::io::IoSlice::new(b" \r\n"),
		std::io::IoSlice::new(b""), // headers
		std::io::IoSlice::new(b"\r\n"),
		std::io::IoSlice::new(b""), // body
	];
	match &res {
		_Response::Ok(_) => {
			io_slices[3] = std::io::IoSlice::new(b"content-type:application/json\r\n");
			io_slices[5] = std::io::IoSlice::new(br#"{"Outputs":{"":""},"Logs":null,"ReturnValue":""}"#);
		},

		_Response::UnknownFunction => (),

		_Response::MethodNotAllowed =>
			io_slices[3] = std::io::IoSlice::new(b"allow:POST\r\n"),

		_Response::Error(err) => {
			io_slices[3] = std::io::IoSlice::new(b"content-type:text/plain\r\n");
			io_slices[5] = std::io::IoSlice::new(err.as_bytes());
		},
	}

	let to_write: usize = io_slices.iter().map(|io_slice| io_slice.len()).sum();

	let written =
		futures_util::future::poll_fn(|cx|
			tokio::io::AsyncWrite::poll_write_vectored(std::pin::Pin::new(stream), cx, &io_slices),
		).await.context("could not write response")?;
	if written != to_write {
		// TODO:
		//
		// Our responses are short enough that writev is unlikely to do a short write, so this works in practice.
		// But when `std::io::IoSlice::advance()` [1] becomes stable, make this a loop that calls that.
		//
		// [1]: https://github.com/rust-lang/rust/issues/62726
		return Err(anyhow::anyhow!("could not write response: short write from writev ({}/{})", written, to_write));
	}

	futures_util::future::poll_fn(|cx|
		tokio::io::AsyncWrite::poll_flush(std::pin::Pin::new(stream), cx),
	).await.context("could not write response")?;

	Ok(())
}
