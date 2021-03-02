#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
	clippy::shadow_unrelated,
)]

mod object_id;
pub use object_id::ObjectId;

pub fn report_error(err: &anyhow::Error) {
	report_inner::<&str, &str>(Report::Error { err });
}

pub fn report_message<D>(message: D) where D: Copy + std::fmt::Display + serde::Serialize {
	report_inner::<&str, _>(Report::Message { message });
}

pub async fn report_operation<IID, D, F, ID>(object_type: &str, object_id: IID, operation: ScopedObjectOperation<D>, f: F) -> F::Output
where
	IID: Into<ObjectId<ID>>,
	ObjectId<ID>: Copy + std::fmt::Display,
	D: Copy + std::fmt::Display + serde::Serialize,
	F: std::future::Future,
	F::Output: std::fmt::Debug,
{
	let object_id = object_id.into();

	match operation {
		ScopedObjectOperation::Create { value } => {
			report_inner::<_, D>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::CreateStart { value } });
			let result = f.await;
			report_inner::<_, &str>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::CreateEnd });
			result
		},

		ScopedObjectOperation::Delete => {
			report_inner::<_, &str>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::DeleteStart });
			let result = f.await;
			report_inner::<_, &str>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::DeleteEnd });
			result
		},

		ScopedObjectOperation::Get => {
			report_inner::<_, &str>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::GetStart });
			let result = f.await;
			report_inner(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::GetEnd { value: format_args!("{:?}", result) } });
			result
		},
	}
}

pub fn report_state<IID, D, ID>(object_type: &str, object_id: IID, state: D)
where
	IID: Into<ObjectId<ID>>,
	ObjectId<ID>: Copy + std::fmt::Display,
	D: Copy + std::fmt::Display + serde::Serialize,
{
	report_inner(Report::ObjectState { object_type, object_id: object_id.into(), state });
}

#[derive(Clone, Copy)]
pub enum ScopedObjectOperation<D = &'static str> {
	Create { value: D },
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

pub static TIME_GENERATED_FIELD: once_cell2::race::LazyBox<hyper::header::HeaderValue> =
	once_cell2::race::LazyBox::new(|| hyper::header::HeaderValue::from_static("TimeCollected"));

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

fn report_inner<ID, D>(report: Report<'_, ID, D>)
where
	for<'a> Record<'a, ID, D>: serde::Serialize,
	for<'a> Report<'a, ID, D>: Copy + std::fmt::Debug,
{
	let timestamp = chrono::Utc::now();

	let _: Result<(), _> = LOGGER.try_with(|TaskLocalLogger { inner }| {
		let mut inner = inner.lock().expect("logger mutex poisoned");
		let TaskLocalLoggerInner { function_invocation_id, sequence_number, records } = &mut *inner;

		*sequence_number += 1;
		if !records.is_empty() {
			records.push(b',');
		}

		let mut serializer = serde_json::Serializer::new(records);
		let () =
			serde::Serialize::serialize(
				&Record {
					timestamp,
					function_invocation_id: function_invocation_id.as_deref(),
					sequence_number: *sequence_number,
					report,
				},
				&mut serializer,
			).expect("could not serialize log record");
	});

	log::log!(
		if matches!(report, Report::Error { .. }) { log::Level::Error } else { log::Level::Info },
		"[{}] {:?}",
		timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
		report,
	);
}

struct Record<'a, ID, D> {
	pub timestamp: chrono::DateTime<chrono::Utc>,
	pub function_invocation_id: Option<&'a str>,
	pub sequence_number: usize,
	pub report: Report<'a, ID, D>,
}

impl<ID, D> serde::Serialize for Record<'_, ID, D>
where
	ObjectId<ID>: serde::Serialize,
	D: std::fmt::Display + serde::Serialize,
{
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: serde::Serializer {
		use serde::ser::SerializeStruct;

		let Record {
			timestamp,
			function_invocation_id,
			sequence_number,
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
				serializer.serialize_field("Exception", &format_args!("{:?}", err))?;
			},

			Report::Message { message } => {
				serializer.serialize_field("Level", &SerializeWith(log::Level::Info))?;
				serializer.serialize_field("Message", message)?;
			},

			Report::ObjectOperation { object_type, object_id, operation } => {
				serializer.serialize_field("Level", &SerializeWith(log::Level::Info))?;
				serializer.serialize_field("ObjectType", object_type)?;
				serializer.serialize_field("ObjectId", object_id)?;
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

			Report::ObjectState { object_type, object_id, state } => {
				serializer.serialize_field("Level", &SerializeWith(log::Level::Info))?;
				serializer.serialize_field("ObjectType", object_type)?;
				serializer.serialize_field("ObjectId", object_id)?;
				serializer.serialize_field("ObjectState", state)?;
			},
		}

		serializer.end()
	}
}

enum Report<'a, ID, D> {
	Error {
		err: &'a anyhow::Error,
	},

	Message {
		message: D,
	},

	ObjectOperation {
		object_type: &'a str,
		object_id: ObjectId<ID>,
		operation: ObjectOperation<D>,
	},

	ObjectState {
		object_type: &'a str,
		object_id: ObjectId<ID>,
		state: D,
	},
}

impl<ID, D> Clone for Report<'_, ID, D> where Self: Copy {
	fn clone(&self) -> Self {
		*self
	}
}

impl<ID, D> Copy for Report<'_, ID, D>
where
	ObjectId<ID>: Copy,
	D: Copy,
{
}

impl<ID, D> std::fmt::Debug for Report<'_, ID, D>
where
	ObjectId<ID>: std::fmt::Display,
	D: std::fmt::Display,
{
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Report::Error { err } =>
				f.debug_struct("Error")
				.field("err", err)
				.finish(),

			Report::Message { message } =>
				f.debug_struct("Message")
				.field("message", &format_args!("{}", message))
				.finish(),

			Report::ObjectOperation { object_type, object_id, operation } =>
				f.debug_struct("ObjectOperation")
				.field("object_type", &format_args!("{}", object_type))
				.field("object_id", &format_args!("{}", object_id))
				.field("operation", operation)
				.finish(),

			Report::ObjectState { object_type, object_id, state } =>
				f.debug_struct("ObjectState")
				.field("object_type", &format_args!("{}", object_type))
				.field("object_id", &format_args!("{}", object_id))
				.field("state", &format_args!("{}", state))
				.finish(),
		}
	}
}

#[derive(Clone, Copy)]
enum ObjectOperation<D> {
	CreateStart { value: D },
	CreateEnd,

	DeleteStart,
	DeleteEnd,

	GetStart,
	GetEnd { value: D },
}

impl<D> std::fmt::Debug for ObjectOperation<D> where D: std::fmt::Display {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ObjectOperation::CreateStart { value } =>
				f.debug_struct("CreateStart")
				.field("value", &format_args!("{}", value))
				.finish(),

			ObjectOperation::CreateEnd =>
				f.debug_struct("CreateEnd")
				.finish(),

			ObjectOperation::DeleteStart =>
				f.debug_struct("DeleteStart")
				.finish(),

			ObjectOperation::DeleteEnd =>
				f.debug_struct("DeleteEnd")
				.finish(),

			ObjectOperation::GetStart =>
				f.debug_struct("GetStart")
				.finish(),

			ObjectOperation::GetEnd { value } =>
				f.debug_struct("GetEnd")
				.field("value", &format_args!("{}", value))
				.finish(),
		}
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
