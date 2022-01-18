#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::let_unit_value,
	clippy::missing_errors_doc,
	clippy::must_use_candidate,
	clippy::shadow_unrelated,
)]

mod object_id;
pub use object_id::ObjectId;

pub struct Logger {
	inner: std::cell::RefCell<LoggerInner>,
}

impl Logger {
	pub fn new(function_invocation_id: Option<String>, persist_records: bool) -> Self {
		Logger {
			inner: std::cell::RefCell::new(LoggerInner {
				function_invocation_id,
				sequence_number: 0,
				records: persist_records.then(Vec::new),
			}),
		}
	}

	pub fn report_error(&self, err: &anyhow::Error) {
		self.report_inner::<&str, &str>(Report::Error { err });
	}

	pub fn report_message<D>(&self, message: D) where D: Copy + std::fmt::Display + serde::Serialize {
		self.report_inner::<&str, _>(Report::Message { message });
	}

	pub async fn report_operation<IID, D, F, ID, T>(&self, object_type: &str, object_id: IID, operation: ScopedObjectOperation<D>, f: F) -> F::Output
	where
		IID: Into<ObjectId<ID>>,
		ObjectId<ID>: Copy + std::fmt::Display,
		D: Copy + std::fmt::Display + serde::Serialize,
		F: std::future::Future<Output = anyhow::Result<T>>,
		T: std::fmt::Debug,
	{
		let object_id = object_id.into();

		match operation {
			ScopedObjectOperation::Create { value } => {
				self.report_inner::<_, D>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::CreateStart { value } });
				let result = f.await?;
				self.report_inner::<_, &str>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::CreateEnd });
				Ok(result)
			},

			ScopedObjectOperation::Delete => {
				self.report_inner::<_, &str>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::DeleteStart });
				let result = f.await?;
				self.report_inner::<_, &str>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::DeleteEnd });
				Ok(result)
			},

			ScopedObjectOperation::Get => {
				self.report_inner::<_, &str>(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::GetStart });
				let result = f.await?;
				self.report_inner(Report::ObjectOperation { object_type, object_id, operation: ObjectOperation::GetEnd { value: format_args!("{result:?}") } });
				Ok(result)
			},
		}
	}

	pub fn report_state<IID, D, ID>(&self, object_type: &str, object_id: IID, state: D)
	where
		IID: Into<ObjectId<ID>>,
		ObjectId<ID>: Copy + std::fmt::Display,
		D: Copy + std::fmt::Display + serde::Serialize,
	{
		self.report_inner(Report::ObjectState { object_type, object_id: object_id.into(), state });
	}

	pub fn take_records(&self) -> Vec<u8> {
		let mut inner = self.inner.borrow_mut();
		inner.records.as_mut().map(std::mem::take).unwrap_or_default()
	}

	fn report_inner<ID, D>(&self, report: Report<'_, ID, D>)
	where
		for<'a> Record<'a, ID, D>: serde::Serialize,
		for<'a> Report<'a, ID, D>: Copy + std::fmt::Debug,
	{
		let timestamp = chrono::Utc::now();

		let mut inner = self.inner.borrow_mut();
		let LoggerInner { function_invocation_id, sequence_number, records } = &mut *inner;

		*sequence_number += 1;

		if let Some(records) = records {
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
		}

		log::log!(
			if matches!(report, Report::Error { .. }) { log::Level::Error } else { log::Level::Info },
			"[{}] {report:?}",
			timestamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
		);
	}
}

#[derive(Clone, Copy)]
pub enum ScopedObjectOperation<D = &'static str> {
	Create { value: D },
	Delete,
	Get,
}

#[allow(clippy::declare_interior_mutable_const)] // Clippy doesn't like const http::HeaderValue
pub const TIME_GENERATED_FIELD: http::HeaderValue = http::HeaderValue::from_static("TimeCollected");

#[derive(serde::Deserialize)]
pub struct Secret<T>(pub T);

impl<T> std::fmt::Debug for Secret<T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("******")
	}
}

struct LoggerInner {
	function_invocation_id: Option<String>,
	sequence_number: usize,
	records: Option<Vec<u8>>,
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
				serializer.serialize_field("Exception", &format_args!("{err:?}"))?;
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

					ObjectOperation::CreateEnd =>
						serializer.serialize_field("ObjectOperation", "CreateEnd")?,

					ObjectOperation::DeleteStart =>
						serializer.serialize_field("ObjectOperation", "DeleteStart")?,

					ObjectOperation::DeleteEnd =>
						serializer.serialize_field("ObjectOperation", "DeleteEnd")?,

					ObjectOperation::GetStart =>
						serializer.serialize_field("ObjectOperation", "GetStart")?,

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
				.field("message", &format_args!("{message}"))
				.finish(),

			Report::ObjectOperation { object_type, object_id, operation } =>
				f.debug_struct("ObjectOperation")
				.field("object_type", &format_args!("{object_type}"))
				.field("object_id", &format_args!("{object_id}"))
				.field("operation", operation)
				.finish(),

			Report::ObjectState { object_type, object_id, state } =>
				f.debug_struct("ObjectState")
				.field("object_type", &format_args!("{object_type}"))
				.field("object_id", &format_args!("{object_id}"))
				.field("state", &format_args!("{state}"))
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
				.field("value", &format_args!("{value}"))
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
				.field("value", &format_args!("{value}"))
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
