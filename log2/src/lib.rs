pub fn report_error(err: &anyhow::Error) {
	report_inner(Report::Error { err });
}

pub fn report_message(message: &str) {
	report_inner(Report::Message { message });
}

pub async fn report_operation<F>(object_type: &str, object_id: &str, operation: ScopedObjectOperation<'_>, f: F) -> F::Output
where
	F: std::future::Future,
	F::Output: std::fmt::Debug,
{
	match operation {
		ScopedObjectOperation::Create { value } => {
			report_inner(Report::ObjectOperation { r#type: object_type, id: object_id, operation: ObjectOperation::CreateStart { value } });
			let result = f.await;
			report_inner(Report::ObjectOperation { r#type: object_type, id: object_id, operation: ObjectOperation::CreateEnd });
			result
		},

		ScopedObjectOperation::Delete => {
			report_inner(Report::ObjectOperation { r#type: object_type, id: object_id, operation: ObjectOperation::DeleteStart });
			let result = f.await;
			report_inner(Report::ObjectOperation { r#type: object_type, id: object_id, operation: ObjectOperation::DeleteEnd });
			result
		},

		ScopedObjectOperation::Get => {
			report_inner(Report::ObjectOperation { r#type: object_type, id: object_id, operation: ObjectOperation::GetStart });
			let result = f.await;
			report_inner(Report::ObjectOperation { r#type: object_type, id: object_id, operation: ObjectOperation::GetEnd { value: &format!("{:?}", result) } });
			result
		},
	}
}

pub fn report_state(r#type: &str, id: &str, state: &str) {
	report_inner(Report::ObjectState { r#type, id, state });
}

#[derive(Clone, Copy)]
pub enum ScopedObjectOperation<'a> {
	Create { value: &'a str },
	Delete,
	Get,
}

pub async fn with_task_local_logger<LS, LSF, F>(
	function_invocation_id: Option<hyper::header::HeaderValue>,
	log_sender: LS,
	f: F,
) -> F::Output
where
	LS: FnOnce(&'static tokio::task::LocalKey<TaskLocalLogger>, tokio::sync::oneshot::Receiver<()>) -> LSF,
	LSF: std::future::Future<Output = anyhow::Result<()>>,
	F: std::future::Future,
{
	futures_util::pin_mut!(f);

	let function_invocation_id =
		function_invocation_id.as_ref()
		.and_then(|function_invocation_id| function_invocation_id.to_str().ok())
		.map(|function_invocation_id| function_invocation_id.into());

	let logger = TaskLocalLogger {
		inner: std::sync::Mutex::new(TaskLocalLoggerInner {
			function_invocation_id,
			sequence_number: 0,
			records: vec![],
		}),
	};

	let result = LOGGER.scope(logger, async move {
		let (stop_log_sender_tx, stop_log_sender_rx) = tokio::sync::oneshot::channel();

		let log_sender = log_sender(&LOGGER, stop_log_sender_rx);
		futures_util::pin_mut!(log_sender);

		match futures_util::future::select(f, log_sender).await {
			futures_util::future::Either::Left((f, log_sender)) => {
				let _ = stop_log_sender_tx.send(());

				match log_sender.await {
					Ok(()) => ScopeResult::Ok(f),
					Err(err) => ScopeResult::ErrReady(f, err),
				}
			},

			futures_util::future::Either::Right((Ok(()), _)) =>
				unreachable!("log sender completed before scoped future"),

			futures_util::future::Either::Right((Err(err), f)) =>
				ScopeResult::ErrPending(f, err),
		}
	}).await;

	match result {
		ScopeResult::Ok(f) => f,
		ScopeResult::ErrPending(f, err) => {
			log::error!("{:?}", err.context("log sender failed"));
			// Run the rest of the future without the logger
			f.await
		},
		ScopeResult::ErrReady(f, err) => {
			log::error!("{:?}", err.context("log sender failed"));
			f
		},
	}
}

pub struct TaskLocalLogger {
	inner: std::sync::Mutex<TaskLocalLoggerInner>,
}

impl TaskLocalLogger {
	pub fn take_records(&self) -> Vec<u8> {
		let mut inner = self.inner.lock().expect("logger mutex poisoned");
		std::mem::take(&mut inner.records)
	}
}

pub static TIME_GENERATED_FIELD: once_cell::sync::Lazy<hyper::header::HeaderValue> =
	once_cell::sync::Lazy::new(|| hyper::header::HeaderValue::from_static("TimeCollected"));

#[derive(serde::Deserialize)]
pub struct Secret<T>(pub T);

impl<T> std::fmt::Debug for Secret<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("******")
	}
}

tokio::task_local! {
	static LOGGER: TaskLocalLogger;
}

struct TaskLocalLoggerInner {
	function_invocation_id: Option<std::sync::Arc<str>>,
	sequence_number: usize,
	records: Vec<u8>,
}

enum ScopeResult<T, F> {
	Ok(T),
	ErrPending(F, anyhow::Error),
	ErrReady(T, anyhow::Error),
}

fn report_inner(report: Report<'_>) {
	let timestamp = chrono::Utc::now();

	let _: Result<(), _> = LOGGER.try_with(|TaskLocalLogger { inner }| {
		let mut inner = inner.lock().expect("logger mutex poisoned");
		let TaskLocalLoggerInner { function_invocation_id, sequence_number, records } = &mut *inner;

		*sequence_number += 1;
		if !records.is_empty() {
			records.push(b',');
		}

		let record = serde_json::to_vec(&Record {
			timestamp,
			sequence_number: *sequence_number,
			function_invocation_id: function_invocation_id.as_deref(),
			report,
		}).expect("could not serialize log record");
		records.extend_from_slice(&record);
	});

	log::log!(
		if matches!(report, Report::Error { .. }) { log::Level::Error } else { log::Level::Info },
		"[{}] {:?}",
		timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
		report,
	);
}

struct Record<'a> {
	pub timestamp: chrono::DateTime<chrono::Utc>,
	pub sequence_number: usize,
	pub function_invocation_id: Option<&'a str>,
	pub report: Report<'a>,
}

#[derive(Clone, Copy, Debug)]
enum Report<'a> {
	Error {
		err: &'a anyhow::Error,
	},

	Message {
		message: &'a str,
	},

	ObjectOperation {
		r#type: &'a str,
		id: &'a str,
		operation: ObjectOperation<'a>,
	},

	ObjectState {
		r#type: &'a str,
		id: &'a str,
		state: &'a str,
	},
}

#[derive(Clone, Copy, Debug)]
enum ObjectOperation<'a> {
	CreateStart { value: &'a str },
	CreateEnd,

	DeleteStart,
	DeleteEnd,

	GetStart,
	GetEnd { value: &'a str },
}

impl serde::Serialize for Record<'_> {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		use serde::ser::SerializeStruct;

		let Record {
			timestamp,
			sequence_number,
			function_invocation_id,
			report,
		} = self;

		let mut serializer = serializer.serialize_struct("Record", 1)?;

		serializer.serialize_field("TimeCollected", &SerializeWith(timestamp))?;
		if let Some(function_invocation_id) = function_invocation_id {
			serializer.serialize_field("FunctionInvocationId", function_invocation_id)?;
		}
		serializer.serialize_field("SequenceNumber", sequence_number)?;

		match report {
			Report::Error { err } => {
				serializer.serialize_field("Level", &SerializeWith(log::Level::Error))?;
				serializer.serialize_field("Exception", &format!("{:?}", err))?;
			},

			Report::Message { message } => {
				serializer.serialize_field("Level", &SerializeWith(log::Level::Info))?;
				serializer.serialize_field("Message", message)?;
			},

			Report::ObjectOperation { r#type, id, operation } => {
				serializer.serialize_field("Level", &SerializeWith(log::Level::Info))?;
				serializer.serialize_field("ObjectType", r#type)?;
				serializer.serialize_field("ObjectId", id)?;
				match operation {
					ObjectOperation::CreateStart { value } => {
						serializer.serialize_field("ObjectOperation", "CreateStart")?;
						serializer.serialize_field("ObjectValue", value)?;
					},

					ObjectOperation::CreateEnd => {
						serializer.serialize_field("ObjectOperation", "CreateEnd")?;
					},

					ObjectOperation::DeleteStart => {
						serializer.serialize_field("ObjectOperation", "DeleteStart")?;
					},

					ObjectOperation::DeleteEnd => {
						serializer.serialize_field("ObjectOperation", "DeleteEnd")?;
					},

					ObjectOperation::GetStart => {
						serializer.serialize_field("ObjectOperation", "GetStart")?;
					},

					ObjectOperation::GetEnd { value } => {
						serializer.serialize_field("ObjectOperation", "GetEnd")?;
						serializer.serialize_field("ObjectValue", value)?;
					},
				}
			},

			Report::ObjectState { r#type, id, state } => {
				serializer.serialize_field("Level", &SerializeWith(log::Level::Info))?;
				serializer.serialize_field("ObjectType", r#type)?;
				serializer.serialize_field("ObjectId", id)?;
				serializer.serialize_field("ObjectState", state)?;
			},
		}

		serializer.end()
	}
}

struct SerializeWith<T>(T);

impl serde::Serialize for SerializeWith<&'_ chrono::DateTime<chrono::Utc>> {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		serializer.serialize_str(&self.0.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
	}
}

impl serde::Serialize for SerializeWith<log::Level> {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		match self.0 {
			log::Level::Debug => serializer.serialize_str("Debug"),
			log::Level::Error => serializer.serialize_str("Error"),
			log::Level::Info => serializer.serialize_str("Information"),
			log::Level::Trace => serializer.serialize_str("Trace"),
			log::Level::Warn => serializer.serialize_str("Warning"),
		}
	}
}
