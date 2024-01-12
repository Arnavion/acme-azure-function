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
	) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<bool>> + 'this>> {
		Box::pin(async move {
			if path == "renew-cert" {
				function_renew_cert::main(
					azure_subscription_id,
					azure_auth,
					settings,
					logger,
				).await?;
				return Ok(true);
			}

			Ok(false)
		})
	}
}
