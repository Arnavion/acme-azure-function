use anyhow::Context;

pub trait Handler {
	type Settings<'a>: serde::Deserialize<'a>;

	fn handle<'this>(
		&'this self,
		path: &'this str,
		azure_subscription_id: &'this str,
		azure_auth: &'this azure::Auth,
		settings: &'this Self::Settings<'_>,
		logger: &'this log2::Logger,
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>> + 'this>>;
}

impl<H> Handler for &H where H: Handler {
	type Settings<'a> = <H as Handler>::Settings<'a>;

	fn handle<'this>(
		&'this self,
		path: &'this str,
		azure_subscription_id: &'this str,
		azure_auth: &'this azure::Auth,
		settings: &'this Self::Settings<'_>,
		logger: &'this log2::Logger,
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>> + 'this>> {
		<H as Handler>::handle(*self, path, azure_subscription_id, azure_auth, settings, logger)
	}
}

pub fn run(handler: impl Handler) -> anyhow::Result<()> {
	let runtime =
		tokio::runtime::Builder::new_current_thread()
		.enable_io()
		.enable_time()
		.build()?;
	let local_set = tokio::task::LocalSet::new();

	local_set.block_on(&runtime, run_inner(handler))?;

	Ok(())
}

async fn run_inner(handler: impl Handler) -> anyhow::Result<()> {
	let (
		misc_logger,
		settings,
		listener,
	) = init().await?;

	let LoggerSettings {
		azure_subscription_id,
		azure_auth,
		azure_log_analytics_workspace_resource_group_name,
		azure_log_analytics_workspace_name,
		rest: settings,
	} = serde_json::from_str(&settings).context("could not read SECRET_SETTINGS env var")?;

	let log_sender =
		azure::management::Client::new(
			&azure_subscription_id,
			&azure_log_analytics_workspace_resource_group_name,
			&azure_auth,
			concat!("github.com/Arnavion/acme-azure-function ", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"))
				.parse().expect("hard-coded user agent is valid HeaderValue"),
			&misc_logger,
		).context("could not initialize Azure Management API client")?
		.log_analytics_log_sender(&azure_log_analytics_workspace_name)
		.await.context("could not create LogAnalytics log sender")?;

	let mut pending_requests = futures_util::stream::FuturesUnordered::new();

	loop {
		let stream = next_stream(&listener, &mut pending_requests, &misc_logger).await;
		pending_requests.push(handle_request(stream, &handler, &azure_subscription_id, &azure_auth, &settings, &log_sender));
	}
}

async fn init() -> anyhow::Result<(log2::Logger, String, tokio::net::TcpListener)> {
	log::set_logger(Box::leak(Box::new(GlobalLogger))).expect("could not set global logger");
	log::set_max_level(log::LevelFilter::Info);

	let misc_logger = log2::Logger::new(None, false);

	let settings = std::env::var("SECRET_SETTINGS").context("could not read SECRET_SETTINGS env var")?;

	let port = match std::env::var("FUNCTIONS_CUSTOMHANDLER_PORT") {
		Ok(value) => value.parse().with_context(|| format!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {value:?}"))?,
		Err(std::env::VarError::NotPresent) => 8080,
		Err(std::env::VarError::NotUnicode(value)) =>
			return Err(anyhow::anyhow!("could not parse FUNCTIONS_CUSTOMHANDLER_PORT value {value:?}")),
	};

	let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], port))).await?;

	Ok((
		misc_logger,
		settings,
		listener,
	))
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

		let timestamp = time::OffsetDateTime::now_utc();
		let level = record.level();

		eprintln!(
			"[{}] {level:5} {}",
			timestamp.format(time2::RFC3339_MILLISECONDS).expect("could not format time"),
			record.args(),
		);
	}

	fn flush(&self) {}
}

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

async fn next_stream(
	listener: &tokio::net::TcpListener,
	pending_requests: &mut (impl futures_util::stream::Stream<Item = anyhow::Result<()>> + Unpin),
	misc_logger: &log2::Logger,
) -> tokio::net::TcpStream {
	loop {
		let stream = std::pin::pin!(listener.accept());
		let next = futures_util::future::select(stream, futures_util::TryStreamExt::try_collect(&mut *pending_requests));
		let stream = match next.await {
			futures_util::future::Either::Left((stream, _)) => stream.context("could not accept connection"),

			// `pending_requests` is empty.
			futures_util::future::Either::Right((Ok(()), stream)) => stream.await.context("could not accept connection"),

			futures_util::future::Either::Right((Err(err), _)) => Err(err),
		};
		match stream {
			Ok((stream, _)) => break stream,
			Err(err) => misc_logger.report_error(&err),
		}
	}
}

async fn handle_request<H>(
	mut stream: tokio::net::TcpStream,
	handler: &H,
	azure_subscription_id: &str,
	azure_auth: &azure::Auth,
	settings: &H::Settings<'_>,
	log_sender: &azure::management::log_analytics::LogSender<'_>,
) -> anyhow::Result<()>
where
	H: Handler,
{
	let (mut read, mut write) = stream.split();

	let mut buf = [std::mem::MaybeUninit::uninit(); 8192];
	let mut buf = tokio::io::ReadBuf::uninit(&mut buf);
	let (method, path, logger) = loop {
		if let Some(req) = parse_request(&mut read, &mut buf).await? {
			break req;
		}
	};

	let res_f = std::pin::pin!(logger.report_operation("function_invocation/request", (method, path), <log2::ScopedObjectOperation>::Get, async {
		if method == "POST" {
			let result = handler.handle(path, azure_subscription_id, azure_auth, settings, &logger).await;
			match result {
				Ok(true) => Response::Ok,
				Ok(false) => Response::UnknownFunction,
				Err(err) => Response::Error(format!("{err:?}")),
			}
		}
		else {
			Response::MethodNotAllowed
		}
	}));

	let (stop_log_sender_tx, log_sender_f) = make_log_sender(&logger, log_sender);
	let log_sender_f = std::pin::pin!(log_sender_f);

	let res = match futures_util::future::select(res_f, log_sender_f).await {
		futures_util::future::Either::Left((res, log_sender_f)) => {
			_ = stop_log_sender_tx.send(());

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

	res.write_to(&mut write).await?;

	Ok(())
}

async fn parse_request<'a>(
	stream: &mut (impl tokio::io::AsyncRead + Unpin),
	buf: &'a mut tokio::io::ReadBuf<'_>,
) -> anyhow::Result<Option<(&'a str, &'a str, log2::Logger)>> {
	if buf.remaining() == 0 {
		return Err(anyhow::anyhow!("request headers too large"));
	}

	{
		let read = tokio::io::AsyncReadExt::read_buf(stream, buf).await.context("could not read request")?;
		if read == 0 {
			return Err(anyhow::anyhow!("malformed request: EOF"));
		}
	}

	let mut headers = [std::mem::MaybeUninit::<httparse::Header<'_>>::uninit(); 16];
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
				str::from_utf8(value).context("malformed request: malformed content-length header")?
				.parse().context("malformed request: malformed content-length header")?;
			let mut remaining = content_length - (buf.filled().len() - body_start);

			let mut buf = [std::mem::MaybeUninit::uninit(); 8192];
			let mut buf = tokio::io::ReadBuf::uninit(&mut buf);

			while remaining > 0 {
				buf.clear();

				let read = tokio::io::AsyncReadExt::read_buf(stream, &mut buf).await.context("could not read request body")?;
				if read == 0 {
					return Err(anyhow::anyhow!("malformed request: EOF"));
				}
				remaining = remaining.saturating_sub(read);
			}
		}
		else if name.eq_ignore_ascii_case(X_AZURE_FUNCTIONS_INVOCATIONID) {
			function_invocation_id = str::from_utf8(value).ok().map(ToOwned::to_owned);
		}
	}

	let logger = log2::Logger::new(function_invocation_id, true);

	Ok(Some((
		method,
		path,
		logger,
	)))
}

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
			let push_timer_tick = std::pin::pin!(push_timer.tick());

			let r = futures_util::future::select(push_timer_tick, &mut stop_log_sender_rx).await;

			let records = logger.take_records();

			if !records.is_empty() {
				#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderValue
				const LOG_TYPE: http::HeaderValue = http::HeaderValue::from_static("FunctionAppLogs");

				log_sender.send_logs(LOG_TYPE, records).await?;
			}

			match r {
				futures_util::future::Either::Left(_) => (),
				futures_util::future::Either::Right(_) => break Ok(()),
			}
		}
	};

	(stop_log_sender_tx, log_sender_f)
}

#[derive(Debug)]
enum Response {
	Ok,
	UnknownFunction,
	MethodNotAllowed,
	Error(String),
}

impl Response {
	async fn write_to(&self, stream: &mut (impl tokio::io::AsyncWrite + Unpin)) -> anyhow::Result<()> {
		let status = match self {
			Response::Ok => http::StatusCode::OK,
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
		match self {
			Response::Ok => {
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

		tokio::io::AsyncWriteExt::flush(stream).await.context("could not write response")?;

		Ok(())
	}
}
