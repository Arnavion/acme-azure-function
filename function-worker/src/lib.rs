#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
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
						&azure_subscription_id,
						&azure_log_analytics_workspace_resource_group_name,
						&azure_log_analytics_workspace_name,
						&azure_auth,
						&misc_logger,
					).await?;

				let mut pending_requests = $crate::_reexports::futures_util::stream::FuturesUnordered::new();

				while let Some(stream) = $crate::_next_stream(&mut incoming, &mut pending_requests, &misc_logger).await {
					pending_requests.push(async {
						let mut stream = stream;
						let (mut read, mut write) = stream.split();

						let mut buf = [std::mem::MaybeUninit::uninit(); 8192];
						let mut buf = $crate::_reexports::tokio::io::ReadBuf::uninit(&mut buf);
						let (method, path, logger) = loop {
							if let Some(req) = $crate::_parse_request(&mut read, &mut buf).await? {
								break req;
							}
						};

						$crate::_handle_request(method, path, &logger, &log_sender, &mut write, async {
							Ok(match path {
								$(
									$name => Some($f(&azure_subscription_id, &azure_auth, &settings, &logger).await?),
								)*
								_ => None,
							})
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

				let timestamp = time::OffsetDateTime::now_utc();
				let level = record.level();

				eprintln!(
					"[{}] {level:5} {}",
					timestamp.format(time2::RFC3339_MILLISECONDS).expect("could not format time"),
					record.args(),
				);
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
		Ok(value) => value.parse().with_context(|| format!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {value:?}"))?,
		Err(std::env::VarError::NotPresent) => 8080,
		Err(std::env::VarError::NotUnicode(value)) =>
			return Err(anyhow::anyhow!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {value:?}")),
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
	} = serde_json::from_str(settings).context("could not read SECRET_SETTINGS env var")?;
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
	azure_subscription_id: &'a str,
	azure_log_analytics_workspace_resource_group_name: &'a str,
	azure_log_analytics_workspace_name: &'a str,
	azure_auth: &'a azure::Auth,
	misc_logger: &'a log2::Logger,
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
	pending_requests: &mut (impl futures_util::stream::Stream<Item = anyhow::Result<()>> + Unpin),
	misc_logger: &log2::Logger,
) -> Option<tokio::net::TcpStream> {
	// FuturesUnordered repeatedly yields Poll::Ready(None) when it's empty, but we want to treat it like it yields Poll::Pending.
	// So chain(stream::pending()) to it.
	let mut pending_requests = futures_util::StreamExt::chain(pending_requests, futures_util::stream::pending());
	loop {
		let next = futures_util::future::try_select(futures_util::TryStreamExt::try_next(incoming), futures_util::TryStreamExt::try_next(&mut pending_requests));
		match next.await {
			Ok(futures_util::future::Either::Left((stream, _))) => break stream,
			Ok(futures_util::future::Either::Right(_)) => (),
			Err(err) => {
				let (err, _) = err.factor_first();
				misc_logger.report_error(&err);
			},
		}
	}
}

#[doc(hidden)]
pub async fn _parse_request<'a>(
	stream: &mut (impl tokio::io::AsyncRead + Unpin),
	buf: &'a mut tokio::io::ReadBuf<'_>,
) -> anyhow::Result<Option<(&'a str, &'a str, log2::Logger)>> {
	if buf.remaining() == 0 {
		return Err(anyhow::anyhow!("request headers too large"));
	}

	{
		let previous_filled = buf.filled().len();
		let () =
			futures_util::future::poll_fn(|cx| tokio::io::AsyncRead::poll_read(std::pin::Pin::new(stream), cx, buf))
			.await.context("could not read request")?;
		let new_filled = buf.filled().len();
		if previous_filled == new_filled {
			return Err(anyhow::anyhow!("malformed request: EOF"));
		}
	}

	// SAFETY: TODO: Replace with `std::mem::MaybeUninit::uninit_array::<16>()` when that is stabilized.
	let mut headers = unsafe { std::mem::MaybeUninit::<[std::mem::MaybeUninit<httparse::Header<'_>>; 16]>::uninit().assume_init() };
	let mut req = httparse::Request::new(&mut []);
	let body_start = match req.parse_with_uninit_headers(buf.filled(), &mut headers).context("malformed request")? {
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
					futures_util::future::poll_fn(|cx| tokio::io::AsyncRead::poll_read(std::pin::Pin::new(stream), cx, &mut buf))
					.await.context("could not read request body")?;
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
pub async fn _handle_request(
	method: &str,
	path: &str,
	logger: &log2::Logger,
	log_sender: &azure::management::log_analytics::LogSender<'_>,
	stream: &mut (impl tokio::io::AsyncWrite + Unpin),
	res_f: impl std::future::Future<Output = anyhow::Result<Option<std::borrow::Cow<'static, str>>>>,
) -> anyhow::Result<()> {
	fn make_log_sender<'a>(
		logger: &'a log2::Logger,
		log_sender: &'a azure::management::log_analytics::LogSender<'_>,
	) -> (
		tokio::sync::oneshot::Sender<()>,
		impl std::future::Future<Output = anyhow::Result<()>> + 'a,
	) {
		let (stop_log_sender_tx, mut stop_log_sender_rx) = tokio::sync::oneshot::channel();

		let log_sender_f = async move {
			let mut push_timer = tokio::time::interval(std::time::Duration::from_secs(1));
			push_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

			loop {
				let push_timer_tick = push_timer.tick();
				futures_util::pin_mut!(push_timer_tick);

				let r = futures_util::future::select(push_timer_tick, stop_log_sender_rx).await;

				let records = logger.take_records();

				if !records.is_empty() {
					#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderValue
					const LOG_TYPE: http::HeaderValue = http::HeaderValue::from_static("FunctionAppLogs");

					log_sender.send_logs(LOG_TYPE, records).await?;
				}

				match r {
					futures_util::future::Either::Left((_, stop_log_sender_rx_)) => stop_log_sender_rx = stop_log_sender_rx_,
					futures_util::future::Either::Right(_) => break Ok::<_, anyhow::Error>(()),
				}
			}
		};

		(stop_log_sender_tx, log_sender_f)
	}

	#[derive(Debug)]
	enum Response {
		Ok(std::borrow::Cow<'static, str>),
		UnknownFunction,
		MethodNotAllowed,
		Error(String),
	}

	async fn write_response(stream: &mut (impl tokio::io::AsyncWrite + Unpin), res: &Response) -> anyhow::Result<()> {
		let status = match res {
			Response::Ok(_) => http::StatusCode::OK,
			Response::UnknownFunction => http::StatusCode::NOT_FOUND,
			Response::MethodNotAllowed => http::StatusCode::METHOD_NOT_ALLOWED,
			Response::Error(_) => http::StatusCode::INTERNAL_SERVER_ERROR,
		};

		let mut io_slices = [
			std::io::IoSlice::new(b"HTTP/1.1 "),
			std::io::IoSlice::new(status.as_str().as_bytes()),
			std::io::IoSlice::new(b" \r\n"),
			std::io::IoSlice::new(b""), // headers
			std::io::IoSlice::new(b"\r\n"),
			std::io::IoSlice::new(b""), // body
		];
		match res {
			Response::Ok(_) => {
				io_slices[3] = std::io::IoSlice::new(b"content-type:application/json\r\n");
				io_slices[5] = std::io::IoSlice::new(br#"{"Outputs":{"":""},"Logs":null,"ReturnValue":""}"#);
			},

			Response::UnknownFunction => (),

			Response::MethodNotAllowed =>
				io_slices[3] = std::io::IoSlice::new(b"allow:POST\r\n"),

			Response::Error(err) => {
				io_slices[3] = std::io::IoSlice::new(b"content-type:text/plain\r\n");
				io_slices[5] = std::io::IoSlice::new(err.as_bytes());
			},
		}

		let to_write: usize = io_slices.iter().map(|io_slice| io_slice.len()).sum();

		let written = tokio::io::AsyncWriteExt::write_vectored(stream, &io_slices).await.context("could not write response")?;
		if written != to_write {
			// TODO:
			//
			// Our responses are short enough that writev is unlikely to do a short write, so this works in practice.
			// But when `std::io::IoSlice::advance()` [1] becomes stable and tokio adds `AsyncWriteExt::write_all_vectored` [2],
			// switch this to use that.
			//
			// [1]: https://github.com/rust-lang/rust/issues/62726
			// [2]: https://github.com/tokio-rs/tokio/issues/3679
			return Err(anyhow::anyhow!("could not write response: short write from writev ({written}/{to_write})"));
		}

		let () = tokio::io::AsyncWriteExt::flush(stream).await.context("could not write response")?;

		Ok(())
	}

	let res_f = async {
		logger.report_state("function_invocation", "", format_args!("Request {{ method: {method:?}, path: {path:?} }}"));

		let res =
			if method == "POST" {
				match res_f.await {
					Ok(Some(message)) => Response::Ok(message),
					Ok(None) => Response::UnknownFunction,
					Err(err) => Response::Error(format!("{err:?}")),
				}
			}
			else {
				Response::MethodNotAllowed
			};

		logger.report_state("function_invocation", "", format_args!("Response {{ {res:?} }}"));
		res
	};
	futures_util::pin_mut!(res_f);

	let (stop_log_sender_tx, log_sender_f) = make_log_sender(logger, log_sender);
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

	write_response(stream, &res).await?;
	Ok(())
}
