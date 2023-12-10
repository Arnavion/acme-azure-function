#![deny(rust_2018_idioms, warnings)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
	clippy::default_trait_access,
	clippy::let_and_return,
	clippy::let_unit_value,
	clippy::too_many_lines,
)]

fn main() -> anyhow::Result<()> {
	function_worker::run(Handler)
}

struct Handler;

impl function_worker::Handler for Handler {
	type Settings<'a> = function_renew_cert::Settings<'a>;

	fn handle<'this>(
		&'this self,
		path: &'this str,
		azure_subscription_id: &'this str,
		azure_auth: &'this azure::Auth,
		settings: &'this function_renew_cert::Settings<'_>,
		logger: &'this log2::Logger,
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<Option<std::borrow::Cow<'static, str>>>> + 'this>> {
		Box::pin(async move {
			if path == "renew-cert" {
				let result = function_renew_cert::main(
					azure_subscription_id,
					azure_auth,
					settings,
					logger,
				).await?;
				return Ok(Some(result));
			}

			Ok(None)
		})
	}
}
